//! Explicit schema-only startup used by one-off imports, without bootstrapping
//! users, granting permissions, or starting an HTTP listener.
#[cfg(feature = "postgres")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use std::str::FromStr;
    let url =
        std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL is required"))?;
    let mut options = sqlx::postgres::PgConnectOptions::from_str(&url)
        .map_err(|_| anyhow::anyhow!("invalid PostgreSQL URL"))?;
    if let Ok(password) = std::env::var("DATABASE_PASSWORD") {
        options = options.password(&password);
    }
    akunkeuangan_service::db::postgres_impl::PostgresRepo::connect_with_options(options, true)
        .await?;
    println!(
        "PostgreSQL schema migrations and validation ready; no bootstrap or HTTP listener started"
    );
    Ok(())
}
#[cfg(not(feature = "postgres"))]
fn main() {
    panic!("requires the postgres feature");
}
