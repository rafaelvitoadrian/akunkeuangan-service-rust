//! akunkeuangan_service — server binary.
//! Konfigurasi & bootstrap saja; seluruh logika ada di lib.rs.

use akunkeuangan_service::{auth, db, router, AppState, DynRepo};
use anyhow::{bail, Context, Result};
use std::sync::Arc;

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let mut env_file = std::env::var_os("APP_ENV_FILE").map(std::path::PathBuf::from);
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            println!("akunkeuangan_service [--env-file PATH]\nLoads .env in the current directory by default. Process environment overrides file values.\nOracle: cargo build --locked --features oracle\nPostgreSQL with startup migrations: cargo build --locked --features postgres");
            return Ok(());
        } else if arg == "--env-file" {
            env_file = Some(args.next().context("--env-file requires a path")?.into());
        } else { bail!("unknown argument; use --help"); }
    }
    let required = env_file.is_some();
    let path = env_file.unwrap_or_else(|| ".env".into());
    let loaded = akunkeuangan_service::config::local_env::load(&path, required)?;
    akunkeuangan_service::util::logging::init()?;
    if loaded { tracing::info!(path = %path.display(), "local configuration loaded"); }
    akunkeuangan_service::config::init();
    let result = tokio::runtime::Runtime::new()?.block_on(run());
    if let Err(error) = &result {
        tracing::error!(error = %format!("{error:#}"), "server stopped");
    }
    result
}

async fn run() -> Result<()> {
    // Railway's PORT takes precedence over local SERVER_PORT.
    let port = akunkeuangan_service::config::server_port()?;

    let repo: DynRepo = build_repo().await?;

    // is.test dari tabel SYSTEMPROP (AkunConfigurationCache lama)
    let is_test = repo.get_system_prop_bool("is.test").await.unwrap_or(false);
    if is_test {
        tracing::warn!("SYSTEMPROP is.test=true → JWT key statis 1234567890123456");
    }

    let state = Arc::new(AppState {
        repo,
        security_key: auth::SecurityKey::new(is_test),
    });

    let app = router(state);
    let app = if let Some(cors) = akunkeuangan_service::config::cors::from_env()? {
        tracing::info!("frontend CORS origin allowlist enabled");
        app.layer(cors)
    } else { app };
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Pilih backend repository.
/// - `REPO_BACKEND=oracle` + build `--features oracle` → Oracle asli (butuh Instant Client)
/// - `REPO_BACKEND=postgres` + `--features postgres` → PostgreSQL + startup migrations
/// - default → PostgreSQL when compiled in; memory must be explicit for fixtures
async fn build_repo() -> Result<DynRepo> {
    let backend = std::env::var("REPO_BACKEND").unwrap_or_else(|_| {
        if cfg!(feature = "postgres") { "postgres" } else { "memory" }.to_string()
    });
    tracing::info!(%backend, "initializing repository");

    #[cfg(feature = "postgres")]
    if backend == "postgres" {
        return Ok(Arc::new(db::postgres_impl::PostgresRepo::from_env().await?));
    }
    #[cfg(not(feature = "postgres"))]
    if backend == "postgres" {
        bail!("REPO_BACKEND=postgres requires a build with --features postgres");
    }

    #[cfg(feature = "oracle")]
    if backend == "oracle" {
        let required = |name: &str| -> Result<String> {
            std::env::var(name).ok().filter(|s| !s.trim().is_empty())
                .with_context(|| format!("missing Oracle configuration: {name}"))
        };
        let main_url = required("SPRING_DATASOURCE_JDBC_URL")?;
        let main_user = required("SPRING_DATASOURCE_USERNAME")?;
        let main_pass = required("SPRING_DATASOURCE_PASSWORD")?;
        let audit_url = std::env::var("AUDIT_DATASOURCE_JDBC_URL").unwrap_or_else(|_| main_url.clone());
        let audit_user = std::env::var("AUDIT_DATASOURCE_USERNAME").unwrap_or_else(|_| main_user.clone());
        let audit_pass = std::env::var("AUDIT_DATASOURCE_PASSWORD").unwrap_or_else(|_| main_pass.clone());
        let client_key = if cfg!(windows) { "ORACLE_CLIENT_LIB_DIR_WINDOWS" }
            else { "ORACLE_CLIENT_LIB_DIR_LINUX" };
        if let Some(dir) = std::env::var("ORACLE_CLIENT_LIB_DIR").ok()
            .or_else(|| std::env::var(client_key).ok()).filter(|s| !s.is_empty()) {
            oracle::InitParams::new().oracle_client_lib_dir(dir)?.init()
                .context("initializing Oracle Client")?;
        }
        let repo = db::oracle_impl::OracleRepo::connect(
            &jdbc_to_connect_string(&main_url),
            &main_user,
            &main_pass,
            &jdbc_to_connect_string(&audit_url),
            &audit_user,
            &audit_pass,
        )
        .context("connecting Oracle repository")?;
        return Ok(Arc::new(repo));
    }
    #[cfg(not(feature = "oracle"))]
    if backend == "oracle" {
        bail!("REPO_BACKEND=oracle butuh build dengan --features oracle");
    }

    if backend != "memory" { bail!("REPO_BACKEND must be oracle, postgres or memory"); }

    let mem = db::memory::MemoryRepo::new();
    // SEED_USER="email|password" — untuk smoke test / golden test login
    if let Ok(seed) = std::env::var("SEED_USER") {
        let mut it = seed.split('|');
        let email = it.next().unwrap_or("admin@example.test");
        let password = it.next().unwrap_or("password");
        mem.seed_user(1, email, password, Some("Admin"));
    }
    if let Ok(is_test) = std::env::var("SYSTEMPROP_IS_TEST") {
        mem.set_prop("is.test", &is_test);
    }
    Ok(Arc::new(mem))
}

/// "jdbc:oracle:thin:@host:port/service" → "host:port/service"
/// (format connect string ODPI-C; tanpa prefix jdbc)
#[cfg(feature = "oracle")]
fn jdbc_to_connect_string(jdbc: &str) -> String {
    let connect = jdbc.strip_prefix("jdbc:oracle:thin:@").unwrap_or(jdbc);
    let Some(host) = std::env::var("ORACLE_DB_HOST").ok().filter(|h| !h.trim().is_empty()) else {
        return connect.to_string();
    };
    let (protocol, endpoint) = connect.strip_prefix("//")
        .map(|rest| ("//", rest)).unwrap_or(("", connect));
    let Some((authority, service)) = endpoint.split_once('/') else { return connect.to_string(); };
    let Some((_, port)) = authority.rsplit_once(':') else { return connect.to_string(); };
    let port = port.split(':').next().unwrap_or(port);
    if port.parse::<u16>().is_err() { return connect.to_string(); }
    format!("{protocol}{host}:{port}/{}", service)
}
