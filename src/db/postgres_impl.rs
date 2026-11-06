//! Native Rust PostgreSQL repository. SQL migrations are embedded in the binary
//! and run before the listener starts; Oracle never uses this migration path.
use super::json_shape::{self, Lifecycle};
use super::postgres_lifecycle::{self as lc_sql, Fields};
use super::Repository;
use crate::models::*;
use anyhow::{anyhow, ensure, Context, Result};
use serde_json::{json, Value};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgRow};
use sqlx::{ConnectOptions, PgConnection, PgPool, Row};
use std::{collections::{HashMap, HashSet}, future::Future, pin::Pin, str::FromStr, time::Duration};

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/postgres");

pub struct PostgresRepo {
    pub(super) pool: PgPool,
}

impl PostgresRepo {
    pub async fn from_env() -> Result<Self> {
        let native_url = std::env::var("DATABASE_URL")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let url = native_url
            .clone()
            .or_else(|| std::env::var("SPRING_DATASOURCE_JDBC_URL").ok())
            .context(
                "PostgreSQL requires DATABASE_URL (postgres://user:password@host:5432/database)",
            )?;
        let url = url.strip_prefix("jdbc:").unwrap_or(&url);
        ensure!(
            url.starts_with("postgres://") || url.starts_with("postgresql://"),
            "PostgreSQL requires a postgres:// or postgresql:// URL"
        );
        let mut options = PgConnectOptions::from_str(url).context("invalid PostgreSQL URL")?;
        if native_url.is_none() {
            if let Ok(user) = std::env::var("SPRING_DATASOURCE_USERNAME") {
                options = options.username(&user);
            }
            if let Ok(password) = std::env::var("SPRING_DATASOURCE_PASSWORD") {
                options = options.password(&password);
            }
        }
        // Optional separate password avoids URL encoding secrets in Compose.
        if let Ok(password) = std::env::var("DATABASE_PASSWORD") {
            options = options.password(&password);
        }
        let migrate = match std::env::var("POSTGRES_AUTO_MIGRATE").as_deref() {
            Err(_) | Ok("true") => true,
            Ok("false") => false,
            _ => return Err(anyhow!("POSTGRES_AUTO_MIGRATE must be true or false")),
        };
        let repo = Self::connect_with_options(options, migrate).await?;
        if let Ok(seed) = std::env::var("POSTGRES_BOOTSTRAP_USER") {
            if !seed.is_empty() {
                let (email, password) = seed
                    .split_once('|')
                    .context("POSTGRES_BOOTSTRAP_USER must be email|password")?;
                ensure!(
                    !email.trim().is_empty() && !password.is_empty(),
                    "bootstrap email and password must not be empty"
                );
                if repo.bootstrap_user(email, password).await? {
                    tracing::info!("PostgreSQL bootstrap user created");
                }
            }
        }
        if let Ok(email)=std::env::var("POSTGRES_BOOTSTRAP_ADMIN_EMAIL") {
            if !email.trim().is_empty() {
                repo.grant_security_admin(&email).await?;
                tracing::info!("explicit PostgreSQL security administrator assignment ready");
            }
        }
        Ok(repo)
    }

    pub async fn connect(url: &str, auto_migrate: bool) -> Result<Self> {
        Self::connect_with_options(PgConnectOptions::from_str(url)?, auto_migrate).await
    }

    pub async fn connect_with_options(
        options: PgConnectOptions,
        auto_migrate: bool,
    ) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .min_connections(1)
            .max_connections(15)
            .acquire_timeout(Duration::from_secs(30))
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET TIME ZONE 'UTC'").execute(conn).await?;
                    Ok(())
                })
            })
            // Do not log credentials, SQL binds or request payloads.
            .connect_with(
                options
                    .application_name("akunkeuangan-rs")
                    .disable_statement_logging(),
            )
            .await
            .context("connecting PostgreSQL repository")?;
        if auto_migrate {
            tracing::info!("checking PostgreSQL schema migrations");
            MIGRATOR.run(&pool).await.context(
                "applying PostgreSQL migrations; server will not start with a failed migration",
            )?;
            tracing::info!("PostgreSQL schema migrations ready");
        }
        // Also reject a missing/incompatible schema when migrations are disabled.
        // CREATE TABLE IF NOT EXISTS never repairs an incompatible existing table.
        for (table, cols) in [
            ("USERS", format!("{USER_COLS},{LC_COLS}")),
            ("AKUNKEUANGAN", format!("{AKUN_COLS},{LC_COLS}")),
            ("CATEGORY", format!("{CATEGORY_COLS},ID_AKUNTRANSAKSI")),
            ("TRANSAKSIKEUANGAN", format!("{TK_COLS},CATEGORY_ID")),
            ("SYSTEMPROP", "PROPNAME,PROPVALUE".into()),
            ("USERROLE", "ID,SHORTNAME,NAME,ACTIVATION_STATUS,INTERNAL".into()),
            ("PERMISSIONPROFILE", "ID,NAME,USER_ID,DEACTIVATION_DATE".into()),
            ("USERACCOUNTPERMISSIONPROFILE", "USER_ID,PROFILE_ID".into()),
            ("PERMISSIONPROFILE_USERROLE", "PROFILE_ID,ROLE_ID".into()),
            ("USERROLEFUNCTION", "ID,ROLE_ID,FUNCTION_ID,ACCESSLEVEL_ID".into()),
            ("SECURITY_FUNCTION", "ID,FUNCTION_NAME,AUTHLEVEL_ID,DESCRIPTION".into()),
            ("ACCESSLEVEL", "ID,NAME".into()),
            ("AUTHLEVEL", "ID,NAME".into()),
        ] {
            sqlx::query(&format!("SELECT {cols} FROM {table} LIMIT 0"))
                .execute(&pool)
                .await
                .with_context(|| {
                    format!("incompatible PostgreSQL table {table}; check migrations")
                })?;
        }
        Ok(Self { pool })
    }

    /// Explicit first-user bootstrap. Repeated startup never changes credentials.
    pub async fn bootstrap_user(&self, email: &str, password: &str) -> Result<bool> {
        ensure!(
            !email.trim().is_empty() && !password.is_empty(),
            "bootstrap email and password must not be empty"
        );
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(724891345012::bigint)")
            .execute(&mut *tx)
            .await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM USERS WHERE EMAIL=$1 AND IDENT_MASTER IS NULL)",
        )
        .bind(email)
        .fetch_one(&mut *tx)
        .await?;
        if exists {
            tx.commit().await?;
            return Ok(false);
        }
        let user=save_user(
            &mut tx,
            UsersRequest {
                name: Some("Admin".into()),
                email: Some(email.into()),
                username: Some(email.into()),
                password: Some(password.into()),
                lifecycle_entity: Some(json!({"activationType":{"identActivationType":1}})),
                ..Default::default()
            },
            true,
        )
        .await?;
        super::postgres_security::grant_admin(&mut tx,user["id"].as_i64().context("bootstrap user id")?).await?;
        tx.commit().await?;
        Ok(true)
    }
}

// PostgreSQL timestamps retain the legacy wall-clock storage convention. The
// HTTP serializer remains json_shape, shared with Oracle. Monetary columns are
// read as TEXT so their precision is independent of any Rust floating type.
const LC_COLS: &str = "IDENT_MASTER,IDENT_REVISION,\
    TO_CHAR(REVISION_EFFECTIVE_FROM,'YYYY-MM-DD HH24:MI:SS.US'),\
    TO_CHAR(REVISION_EFFECTIVE_TO,'YYYY-MM-DD HH24:MI:SS.US'),\
    TO_CHAR(REQUESTED_ACTIVATION_DATE,'YYYY-MM-DD'),IDENT_ACTIVATIONTYPE,IDENT_LASTUPDATEUSER,\
    TO_CHAR(LASTUPDATETIME,'YYYY-MM-DD HH24:MI:SS.US'),IDENT_REVISIONSTATE";
macro_rules! utc_ts {
    ($col:literal, $fmt:literal) => {
        concat!(
            "TO_CHAR((",
            $col,
            " AT TIME ZONE 'Asia/Jakarta') AT TIME ZONE 'UTC','",
            $fmt,
            "')"
        )
    };
}
const USER_COLS: &str = concat!(
    "ID,NAME,EMAIL,USERNAME,PWD,PASSWORD,SALT,",
    utc_ts!("CREATEDDATE", "YYYY-MM-DD HH24:MI:SS"),
    ",",
    utc_ts!("UPDATEDDATE", "YYYY-MM-DD HH24:MI:SS")
);
const AKUN_COLS: &str = concat!(
    "ID,NAME,DESCRIPTION,ID_USER,SALDO::text,ACCOUNT_NUMBER,",
    utc_ts!("CREATEDDATE", "YYYY-MM-DD HH24:MI:SS"),
    ",",
    utc_ts!("UPDATEDDATE", "YYYY-MM-DD HH24:MI:SS")
);
const CATEGORY_COLS: &str = concat!(
    "ID,CATEGORY_NAME,CATEGORY_TYPE::text,",
    utc_ts!("CREATEDDATE", "YYYY-MM-DD HH24:MI:SS.US"),
    ",",
    utc_ts!("UPDATEDDATE", "YYYY-MM-DD HH24:MI:SS.US")
);
const TK_COLS: &str = concat!(
    "ID,NAME,DESCRIPTION,ID_AKUNTRANSAKSI,JENIS::text,NOMINAL::text,",
    utc_ts!("CREATEDDATE", "YYYY-MM-DD HH24:MI:SS"),
    ",",
    utc_ts!("UPDATEDDATE", "YYYY-MM-DD HH24:MI:SS"),
    ",",
    utc_ts!("TRANSAKSI_DATE", "YYYY-MM-DD HH24:MI:SS"),
    ",EXTERNALREFERENCE,INTERNALREFERENCE"
);

fn activation_name(id: i64) -> &'static str {
    match id {
        1 => "Active",
        2 => "Deactivated",
        4 => "Suspended",
        5 => "Inactive",
        _ => "Unknown",
    }
}
fn revision_state_name(id: i64) -> &'static str {
    match id {
        1 => "Ready",
        2 => "AwaitingApproval",
        3 => "ValidationFailed",
        4 => "Rejected",
        5 => "Draft",
        6 => "Deleted",
        7 => "Error",
        8 => "Pending",
        _ => "Unknown",
    }
}
fn read_lifecycle(row: &PgRow, off: usize, strict: bool) -> Result<Lifecycle> {
    let act: Option<i64> = row.try_get(off + 5)?;
    let legacy_crash = std::env::var("LEGACY_NULL_ACTIVATION_CRASH")
        .map(|v| !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true);
    ensure!(
        !(strict && act.is_none() && legacy_crash),
        "null value was assigned to ident_activationtype (legacy behaviour: HTTP 500)"
    );
    let rev: Option<i64> = row.try_get(off + 8)?;
    Ok(Lifecycle {
        ident_master: row.try_get(off)?,
        ident_revision: row.try_get(off + 1)?,
        revision_effective_from: row.try_get(off + 2)?,
        revision_effective_to: row.try_get(off + 3)?,
        requested_activation_date: row.try_get(off + 4)?,
        activation_type: act.map(|id| (id, activation_name(id).into())),
        ident_activation_type: act.unwrap_or(0),
        last_update_user_ident: row.try_get(off + 6)?,
        last_update_time: row.try_get(off + 7)?,
        revision_state: rev.map(|id| (id, revision_state_name(id).into())),
    })
}
fn text(row: &PgRow, index: usize) -> Result<Option<String>> {
    Ok(row.try_get(index)?)
}
fn user_from_row(row: &PgRow, strict: bool) -> Result<Value> {
    Ok(json_shape::user_json(
        row.try_get(0)?,
        &read_lifecycle(row, 9, strict)?,
        text(row, 1)?.as_deref(),
        text(row, 2)?.as_deref(),
        text(row, 3)?.as_deref(),
        text(row, 4)?.as_deref(),
        text(row, 5)?.as_deref(),
        text(row, 6)?.as_deref(),
        text(row, 7)?.as_deref(),
        text(row, 8)?.as_deref(),
    ))
}

pub(super) async fn raw_users(conn: &mut PgConnection) -> Result<Vec<Value>> {
    sqlx::query(&format!("SELECT {USER_COLS},{LC_COLS} FROM USERS ORDER BY ID"))
        .fetch_all(conn).await?.iter().map(|row| user_from_row(row, false)).collect()
}
fn akun_from_row(row: &PgRow, strict: bool) -> Result<Value> {
    Ok(json_shape::akun_json(
        row.try_get(0)?,
        &read_lifecycle(row, 8, strict)?,
        text(row, 1)?.as_deref(),
        text(row, 2)?.as_deref(),
        row.try_get(3)?,
        text(row, 4)?.as_deref(),
        text(row, 5)?.as_deref(),
        text(row, 6)?.as_deref(),
        text(row, 7)?.as_deref(),
    ))
}
fn category_from_row(row: &PgRow, account: Option<Value>) -> Result<Value> {
    Ok(json_shape::category_json(
        row.try_get(0)?,
        text(row, 1)?.as_deref(),
        text(row, 2)?.as_deref(),
        account,
        text(row, 3)?.as_deref(),
        text(row, 4)?.as_deref(),
    ))
}
fn transaksi_from_row(row: &PgRow, category: Option<Value>) -> Result<Value> {
    Ok(json_shape::transaksi_json(
        row.try_get(0)?,
        text(row, 1)?.as_deref(),
        text(row, 2)?.as_deref(),
        row.try_get(3)?,
        category,
        text(row, 4)?.as_deref(),
        text(row, 5)?.as_deref(),
        text(row, 6)?.as_deref(),
        text(row, 7)?.as_deref(),
        text(row, 8)?.as_deref(),
        text(row, 9)?.as_deref(),
        text(row, 10)?.as_deref(),
    ))
}

pub(super) async fn load_lifecycle(
    conn: &mut PgConnection,
    table: &str,
    id: i64,
) -> Result<Lifecycle> {
    let row = sqlx::query(&format!("SELECT {LC_COLS} FROM {table} WHERE ID=$1"))
        .bind(id)
        .fetch_one(conn)
        .await?;
    read_lifecycle(&row, 0, true)
}
async fn load_akun_value(conn: &mut PgConnection, id: i64) -> Result<Option<Value>> {
    sqlx::query(&format!(
        "SELECT {AKUN_COLS},{LC_COLS} FROM AKUNKEUANGAN WHERE ID=$1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .as_ref()
    .map(|r| akun_from_row(r, true))
    .transpose()
}
async fn load_category_value(conn: &mut PgConnection, id: i64) -> Result<Option<Value>> {
    let Some(row) = sqlx::query(&format!(
        "SELECT {CATEGORY_COLS},ID_AKUNTRANSAKSI FROM CATEGORY WHERE ID=$1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let account = match row.try_get::<Option<i64>, _>(5)? {
        Some(id) => load_akun_value(conn, id).await?,
        None => None,
    };
    Ok(Some(category_from_row(&row, account)?))
}
async fn transaksi_category(conn: &mut PgConnection, id: i64) -> Result<Option<Value>> {
    let id: Option<i64> =
        sqlx::query_scalar("SELECT CATEGORY_ID FROM TRANSAKSIKEUANGAN WHERE ID=$1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    match id {
        Some(id) => load_category_value(conn, id).await,
        None => Ok(None),
    }
}

/// Request-local relation snapshot: at most three SELECTs, regardless of the
/// transaction count. A category may belong to a different account, so hydrate
/// its actual account instead of assuming it is the list's owning account.
struct TransactionList {
    rows: Vec<PgRow>,
    categories: HashMap<i64, Value>,
}

async fn load_transaction_list(conn: &mut PgConnection, account: i64, email: &str) -> Result<TransactionList> {
    let rows = sqlx::query(&format!("SELECT {TK_COLS},CATEGORY_ID FROM TRANSAKSIKEUANGAN tk WHERE tk.ID_AKUNTRANSAKSI=$1 \
        AND EXISTS(SELECT 1 FROM AKUNKEUANGAN ak,USERS u WHERE u.ID=ak.ID_USER AND ak.IDENT_MASTER IS NULL AND ak.ID=$1 AND u.EMAIL=$2) ORDER BY tk.TRANSAKSI_DATE DESC"))
        .bind(account).bind(email).fetch_all(&mut *conn).await?;
    let mut category_ids = HashSet::new();
    for row in &rows {
        if let Some(id) = row.try_get::<Option<i64>, _>(11)? { category_ids.insert(id); }
    }
    let mut categories = HashMap::new();
    if !category_ids.is_empty() {
        let category_rows = sqlx::query(&format!("SELECT {CATEGORY_COLS},ID_AKUNTRANSAKSI FROM CATEGORY WHERE ID=ANY($1)"))
            .bind(category_ids.into_iter().collect::<Vec<_>>()).fetch_all(&mut *conn).await?;
        let mut account_ids = HashSet::new();
        for row in &category_rows {
            if let Some(id) = row.try_get::<Option<i64>, _>(5)? { account_ids.insert(id); }
        }
        let mut accounts = HashMap::new();
        if !account_ids.is_empty() {
            for row in sqlx::query(&format!("SELECT {AKUN_COLS},{LC_COLS} FROM AKUNKEUANGAN WHERE ID=ANY($1)"))
                .bind(account_ids.into_iter().collect::<Vec<_>>()).fetch_all(&mut *conn).await? {
                accounts.insert(row.try_get::<i64, _>(0)?, akun_from_row(&row, true)?);
            }
        }
        for row in category_rows {
            let account = row.try_get::<Option<i64>, _>(5)?.and_then(|id| accounts.get(&id)).cloned();
            categories.insert(row.try_get::<i64, _>(0)?, category_from_row(&row, account)?);
        }
    }
    Ok(TransactionList { rows, categories })
}

impl TransactionList {
    fn into_values(self) -> Result<Vec<Value>> {
        self.rows.into_iter().map(|row| {
            let category = row.try_get::<Option<i64>, _>(11)?.and_then(|id| self.categories.get(&id)).cloned();
            transaksi_from_row(&row, category)
        }).collect()
    }

    fn into_json_bytes(self, account: i64) -> Result<Vec<u8>> {
        // Repeated nested relations are encoded once. Keep only one transaction
        // Value at a time rather than a 100k-element, deeply cloned JSON tree.
        let categories: HashMap<i64, Vec<u8>> = self.categories.into_iter()
            .map(|(id, value)| Ok((id, serde_json::to_vec(&value)?))).collect::<Result<_>>()?;
        let mut output = Vec::new();
        output.extend_from_slice(b"{\"data\":[");
        for (index, row) in self.rows.into_iter().enumerate() {
            if index != 0 { output.push(b','); }
            let category = row.try_get::<Option<i64>, _>(11)?.and_then(|id| categories.get(&id));
            let value = transaksi_from_row(&row, None)?;
            // Field names/order and scalar formatting still come exclusively
            // from json_shape::transaksi_json, shared with detail/write APIs.
            output.push(b'{');
            for (field_index, (key, value)) in value.as_object().context("transaction JSON object")?.iter().enumerate() {
                if field_index != 0 { output.push(b','); }
                serde_json::to_writer(&mut output, key)?;
                output.push(b':');
                if key == "category" {
                    output.extend_from_slice(category.map(Vec::as_slice).unwrap_or(b"null"));
                } else {
                    serde_json::to_writer(&mut output, value)?;
                }
            }
            output.push(b'}');
        }
        output.extend_from_slice(b"],\"IdAkun\":");
        serde_json::to_writer(&mut output, &account)?;
        output.push(b'}');
        Ok(output)
    }
}
async fn converted_lifecycle(
    conn: &mut PgConnection,
    table: &str,
    input: &crate::lifecycle::LifecycleRequest,
    actor: Option<i64>,
    create: bool,
) -> Result<Lifecycle> {
    ensure!(
        !(create && input.ident_entity > 0),
        "primary key supplied for create"
    );
    let mut lc = if input.ident_entity > 0 {
        lc_sql::lock_master(conn, table, input.ident_entity).await?;
        load_lifecycle(conn, table, input.ident_entity).await?
    } else {
        Lifecycle::default()
    };
    lc.activation_type = match input.activation_id() {
        Some(id) => {
            let name: String = sqlx::query_scalar(
                "SELECT TYPENAME FROM ACTIVATIONTYPE WHERE IDENT_ACTIVATIONTYPE=$1",
            )
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
            Some((id, name))
        }
        None => None,
    };
    lc.ident_activation_type = lc.activation_type.as_ref().map(|v| v.0).unwrap_or(0);
    lc.requested_activation_date = input.requested_activation_date.map(|v| v.to_string());
    lc.last_update_user_ident = actor;
    Ok(lc)
}
async fn write_entity_response(
    conn: &mut PgConnection,
    table: &str,
    id: i64,
    transient: bool,
) -> Result<Value> {
    let cols = if table == "USERS" {
        USER_COLS
    } else {
        AKUN_COLS
    };
    let row = sqlx::query(&format!("SELECT {cols},{LC_COLS} FROM {table} WHERE ID=$1"))
        .bind(id)
        .fetch_one(conn)
        .await?;
    let mut value = if table == "USERS" {
        user_from_row(&row, false)?
    } else {
        akun_from_row(&row, false)?
    };
    if transient {
        value["identActivationType"] = json!(0);
    }
    Ok(value)
}

async fn user_fields(conn: &mut PgConnection, id: i64) -> Result<Fields> {
    let row = sqlx::query("SELECT NAME,EMAIL,USERNAME,PWD,PASSWORD,SALT,TO_CHAR(CREATEDDATE,'YYYY-MM-DD HH24:MI:SS'),TO_CHAR(UPDATEDDATE,'YYYY-MM-DD HH24:MI:SS') FROM USERS WHERE ID=$1")
        .bind(id).fetch_one(conn).await?;
    [
        "NAME",
        "EMAIL",
        "USERNAME",
        "PWD",
        "PASSWORD",
        "SALT",
        "CREATEDDATE",
        "UPDATEDDATE",
    ]
    .iter()
    .enumerate()
    .map(|(i, name)| Ok((*name, text(&row, i)?)))
    .collect()
}

async fn save_user(conn: &mut PgConnection, req: UsersRequest, create: bool) -> Result<Value> {
    let input = crate::lifecycle::LifecycleRequest::from_value(&req.lifecycle_entity)?;
    let lc = converted_lifecycle(conn, "USERS", &input, req.actor_ident, create).await?;
    let creates_revision = !create
        && lc.ident_master.is_none()
        && !(lc.activation_type.as_ref().map(|v| v.0) == Some(5)
            && lc_sql::has_future(conn, "USERS", input.ident_entity).await?);
    let (salt, pwd, password) = if create {
        let password = req.password.as_deref().context("null password")?;
        let salt = crate::auth::new_password_salt();
        let hash = crate::auth::hash_password_sha256(&salt, password);
        (
            Some(salt),
            Some(hash),
            Some(crate::auth::hash_password_sha1(password)),
        )
    } else {
        let row = sqlx::query("SELECT SALT,PWD,PASSWORD FROM USERS WHERE ID=$1")
            .bind(input.ident_entity)
            .fetch_one(&mut *conn)
            .await?;
        let salt = text(&row, 0)?;
        let hash = text(&row, 1)?;
        match req.password.as_deref() {
            Some(p) if !p.is_empty() => {
                let salt = salt.context("null salt")?;
                if hash.as_deref() == Some(&crate::auth::hash_password_sha256(&salt, p)) {
                    (Some(salt), hash, text(&row,2)?)
                } else {
                    let salt = crate::auth::new_password_salt();
                    let hash = crate::auth::hash_password_sha256(&salt, p);
                    (Some(salt), Some(hash), Some(crate::auth::hash_password_sha1(p)))
                }
            }
            _ => (salt, hash, text(&row,2)?),
        }
    };
    let created_date=if create { None } else {
        sqlx::query_scalar::<_,Option<String>>("SELECT createddate::text FROM users WHERE id=$1")
            .bind(input.ident_entity).fetch_one(&mut *conn).await?
    };
    let fields = vec![
        ("NAME", req.name.clone()),
        ("EMAIL", req.email.clone()),
        ("USERNAME", req.username.clone()),
        ("PWD", pwd),
        ("PASSWORD", password),
        ("SALT", salt),
        ("CREATEDDATE", created_date),
        ("UPDATEDDATE", (!create).then(crate::util::app_now_string)),
    ];
    let assignments = super::postgres_security::prepare_assignments(conn,&req).await?;
    if !create {
        super::postgres_security::replace_assignments(conn,input.ident_entity,&assignments.ids).await?;
    }
    let id = lc_sql::persist(
        conn,
        "USERS",
        "users_sequence",
        input.ident_entity,
        lc,
        fields,
        create,
    )
    .await?;
    if create {
        super::postgres_security::replace_assignments(conn,id,&assignments.ids).await?;
        let revision: i64=sqlx::query_scalar("SELECT ident_revision FROM users WHERE id=$1")
            .bind(id).fetch_one(&mut *conn).await?;
        super::postgres_security::copy_assignments(conn,id,revision).await?;
    }
    for profile in &assignments.individual {
        sqlx::query("UPDATE permissionprofile SET user_id=$2 WHERE id=$1")
            .bind(profile).bind(id).execute(&mut *conn).await?;
    }
    let mut value = write_entity_response(conn, "USERS", id, create || creates_revision).await?;
    value["name"] = json!(req.name);
    value["email"] = json!(req.email);
    value["username"] = json!(req.username);
    Ok(value)
}

async fn save_account(
    conn: &mut PgConnection,
    req: AkunKeuanganRequest,
    email: &str,
    create: bool,
) -> Result<Value> {
    let input = crate::lifecycle::LifecycleRequest::from_value(&req.lifecycle_entity)?;
    let lc = converted_lifecycle(conn, "AKUNKEUANGAN", &input, req.actor_ident, create).await?;
    let number = if create {
        Some(crate::util::create_account_number(req.account_type).map_err(|e| anyhow!(e))?)
    } else {
        sqlx::query_scalar("SELECT ACCOUNT_NUMBER FROM AKUNKEUANGAN WHERE ID=$1")
            .bind(input.ident_entity)
            .fetch_one(&mut *conn)
            .await?
    };
    if create && req.name.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(super::ValidationError("'Name' is mandatory.".into()).into());
    }
    let actor =
        match req.actor_ident {
            Some(id) => Some(id),
            None if !email.is_empty() => sqlx::query_scalar(
                "SELECT ID FROM USERS WHERE EMAIL=$1 AND IDENT_MASTER IS NULL ORDER BY ID LIMIT 1",
            )
            .bind(email)
            .fetch_optional(&mut *conn)
            .await?,
            None => None,
        };
    let fields = vec![
        ("NAME", req.name.clone()),
        ("DESCRIPTION", req.description.clone()),
        ("ID_USER", actor.map(|v| v.to_string())),
        ("SALDO", Some("0".into())),
        ("ACCOUNT_NUMBER", number),
        ("CREATEDDATE", None),
        ("UPDATEDDATE", None),
    ];
    let id = lc_sql::persist(
        conn,
        "AKUNKEUANGAN",
        "seq_account_composite",
        input.ident_entity,
        lc,
        fields,
        create,
    )
    .await?;
    let mut value = write_entity_response(conn, "AKUNKEUANGAN", id, true).await?;
    value["name"] = json!(req.name);
    value["description"] = json!(req.description);
    Ok(value)
}

fn relation_id(value: &Option<Value>) -> Option<i64> {
    value
        .as_ref()
        .and_then(|v| v.get("id").or_else(|| v.get("entityIdent")))
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|v| v.parse().ok()))
        })
        .map(|v| v.max(0))
}
fn request_account_json(value: &Option<Value>) -> Option<Value> {
    value.as_ref().map(|input| {
        let mut output = json_shape::akun_stub(relation_id(value).unwrap_or(0));
        for key in [
            "name",
            "description",
            "idUser",
            "saldo",
            "accountNumber",
            "createdDate",
            "updatedDate",
            "identMaster",
            "identRevision",
            "revisionEffectiveFrom",
            "revisionEffectiveTo",
            "requestedActivationDate",
            "activationType",
            "revisionState",
            "identActivationType",
            "lastUpdateUserIdent",
            "lastUpdateTime",
        ] {
            if let Some(v) = input.get(key) {
                output[key] = v.clone();
            }
        }
        output
    })
}
fn request_category_json(value: &Option<Value>) -> Result<Option<Value>> {
    value
        .as_ref()
        .map(|value| {
            let cat: CategoryRequest = serde_json::from_value(value.clone())?;
            Ok(json_shape::category_json(
                cat.id,
                cat.category_name.as_deref(),
                cat.category_type.as_ref().map(|v| v.to_string()).as_deref(),
                request_account_json(&cat.akun_keuangan),
                cat.created_date
                    .map(|v| v.format("%Y-%m-%d %H:%M:%S%.9f").to_string())
                    .as_deref(),
                cat.updated_date
                    .map(|v| v.format("%Y-%m-%d %H:%M:%S%.9f").to_string())
                    .as_deref(),
            ))
        })
        .transpose()
}
fn timestamp_to_sql(value: Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    value.map(|v| {
        let offset = std::env::var("APP_UTC_OFFSET_MINUTES")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        (v.naive_utc() + chrono::Duration::minutes(offset))
            .format("%Y-%m-%d %H:%M:%S%.9f")
            .to_string()
    })
}

async fn save_category(
    conn: &mut PgConnection,
    cat: CategoryRequest,
    create: bool,
) -> Result<Value> {
    ensure!(!(create && cat.id > 0), "detached entity passed to persist");
    let id = if create {
        lc_sql::next_seq(conn, "category_sequence").await?
    } else {
        cat.id
    };
    let (previous, created, updated) = if create {
        (
            None,
            Some(crate::util::app_now_string()),
            timestamp_to_sql(cat.updated_date),
        )
    } else {
        let row = sqlx::query("SELECT ID_AKUNTRANSAKSI,TO_CHAR(CREATEDDATE,'YYYY-MM-DD HH24:MI:SS.US'),TO_CHAR(UPDATEDDATE,'YYYY-MM-DD HH24:MI:SS.US') FROM CATEGORY WHERE ID=$1 FOR UPDATE")
            .bind(id).fetch_one(&mut *conn).await?;
        (
            row.try_get::<Option<i64>, _>(0)?,
            text(&row, 1)?,
            text(&row, 2)?,
        )
    };
    let fields = vec![
        ("CATEGORY_NAME", cat.category_name.clone()),
        (
            "CATEGORY_TYPE",
            cat.category_type.as_ref().map(|v| v.to_string()),
        ),
        (
            "ID_AKUNTRANSAKSI",
            relation_id(&cat.akun_keuangan).map(|v| v.to_string()),
        ),
        ("CREATEDDATE", created),
        ("UPDATEDDATE", updated),
    ];
    lc_sql::write_fields(conn, "CATEGORY", id, &fields, create).await?;
    let row = sqlx::query(&format!("SELECT {CATEGORY_COLS} FROM CATEGORY WHERE ID=$1"))
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    let account = if create || previous != relation_id(&cat.akun_keuangan) {
        request_account_json(&cat.akun_keuangan)
    } else {
        match relation_id(&cat.akun_keuangan) {
            Some(id) => load_akun_value(conn, id).await?,
            None => None,
        }
    };
    let mut response = category_from_row(&row, account)?;
    response["category_type"] =
        json_shape::fmt_decimal(cat.category_type.as_ref().map(|v| v.to_string()).as_deref());
    Ok(response)
}
async fn save_transaksi(
    conn: &mut PgConnection,
    tk: TransaksiKeuanganRequest,
    create: bool,
) -> Result<Value> {
    ensure!(!(create && tk.id > 0), "detached entity passed to persist");
    let id = if create {
        lc_sql::next_seq(conn, "transaksikeuangan_sequence").await?
    } else {
        tk.id
    };
    let (previous, created, updated, external, internal) = if create {
        (
            None,
            Some(crate::util::app_now_string()),
            timestamp_to_sql(tk.updated_date),
            tk.externalreference.clone(),
            Some(crate::util::generate_internal_reference(
                crate::util::app_now(),
            )),
        )
    } else {
        let row = sqlx::query("SELECT CATEGORY_ID,TO_CHAR(CREATEDDATE,'YYYY-MM-DD HH24:MI:SS'),TO_CHAR(UPDATEDDATE,'YYYY-MM-DD HH24:MI:SS'),EXTERNALREFERENCE,INTERNALREFERENCE FROM TRANSAKSIKEUANGAN WHERE ID=$1 FOR UPDATE")
            .bind(id).fetch_one(&mut *conn).await?;
        (
            row.try_get::<Option<i64>, _>(0)?,
            text(&row, 1)?,
            text(&row, 2)?,
            text(&row, 3)?,
            text(&row, 4)?,
        )
    };
    let fields = vec![
        ("NAME", tk.name.clone()),
        ("DESCRIPTION", tk.description.clone()),
        (
            "ID_AKUNTRANSAKSI",
            tk.id_akun_keuangan.map(|v| v.to_string()),
        ),
        ("JENIS", tk.jenis.as_ref().map(|v| v.to_string())),
        ("NOMINAL", tk.nominal.as_ref().map(|v| v.to_string())),
        (
            "CATEGORY_ID",
            relation_id(&tk.category).map(|v| v.to_string()),
        ),
        ("CREATEDDATE", created),
        ("UPDATEDDATE", updated),
        ("TRANSAKSI_DATE", timestamp_to_sql(tk.transaction_date)),
        ("EXTERNALREFERENCE", external),
        ("INTERNALREFERENCE", internal),
    ];
    lc_sql::write_fields(conn, "TRANSAKSIKEUANGAN", id, &fields, create).await?;
    let row = sqlx::query(&format!(
        "SELECT {TK_COLS} FROM TRANSAKSIKEUANGAN WHERE ID=$1"
    ))
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    let category = if create || previous != relation_id(&tk.category) {
        request_category_json(&tk.category)?
    } else {
        transaksi_category(conn, id).await?
    };
    let mut response = transaksi_from_row(&row, category)?;
    response["name"] = json!(tk.name);
    response["description"] = json!(tk.description);
    if create {
        response["externalreference"] = json!(tk.externalreference);
    }
    response["jenis"] =
        json_shape::fmt_decimal(tk.jenis.as_ref().map(|v| v.to_string()).as_deref());
    response["nominal"] =
        json_shape::fmt_decimal(tk.nominal.as_ref().map(|v| v.to_string()).as_deref());
    Ok(response)
}

impl Repository for PostgresRepo {
    fn fetch_all_user_models(&self) -> BoxFut<'_, Vec<Value>> {
        Box::pin(async move {
            let mut conn = self.pool.acquire().await?;
            let graph = super::postgres_security_graph::SecurityGraph::load(&mut conn).await?;
            graph.master_users()
        })
    }
    fn create_user_model<'a>(&'a self,req:UsersRequest)->BoxFut<'a,Value> {
        Box::pin(async move {
            let mut tx=self.pool.begin().await?;
            let raw=save_user(&mut tx,req,true).await?;
            let model=super::postgres_security::assemble_user(&mut tx,raw).await?;
            tx.commit().await?; Ok(model)
        })
    }
    fn update_user_model<'a>(&'a self,req:UsersRequest)->BoxFut<'a,Value> {
        Box::pin(async move {
            let mut tx=self.pool.begin().await?;
            let raw=save_user(&mut tx,req,false).await?;
            let model=super::postgres_security::assemble_user(&mut tx,raw).await?;
            tx.commit().await?; Ok(model)
        })
    }
    fn user_model<'a>(&'a self,raw:Value)->BoxFut<'a,Value> {
        Box::pin(async move { let mut conn=self.pool.acquire().await?; super::postgres_security::assemble_user(&mut conn,raw).await })
    }
    fn require_function<'a>(&'a self,email:&'a str,function:&'a str,minimum:i64)->BoxFut<'a,()> {
        Box::pin(async move { let mut conn=self.pool.acquire().await?; super::postgres_security::require_function(&mut conn,email,function,minimum).await })
    }
    fn security_query<'a>(&'a self,query:crate::security::SecurityQuery,email:&'a str)->BoxFut<'a,Value> {
        Box::pin(async move { self.security_read(query,email).await })
    }
    fn security_write<'a>(&'a self,command:crate::security::SecurityWrite,email:&'a str)->BoxFut<'a,Value> {
        Box::pin(async move { self.security_mutate(command,email).await })
    }
    fn resolve_actor_ident<'a>(&'a self, email: &'a str) -> BoxFut<'a, Option<i64>> {
        Box::pin(async move {
            Ok(sqlx::query_scalar(
                "SELECT ID FROM USERS WHERE EMAIL=$1 AND IDENT_MASTER IS NULL ORDER BY ID LIMIT 1",
            )
            .bind(email)
            .fetch_optional(&self.pool)
            .await?)
        })
    }
    fn get_system_prop_bool<'a>(&'a self, name: &'a str) -> BoxFut<'a, bool> {
        Box::pin(async move {
            let value: Option<Option<String>> =
                sqlx::query_scalar("SELECT PROPVALUE FROM SYSTEMPROP WHERE PROPNAME=$1")
                    .bind(name)
                    .fetch_optional(&self.pool)
                    .await?;
            Ok(value
                .flatten()
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false))
        })
    }
    fn authenticate_user_by_email<'a>(
        &'a self,
        email: &'a str,
        password: &'a str,
    ) -> BoxFut<'a, bool> {
        Box::pin(async move {
            let stored: Option<Option<String>> = sqlx::query_scalar("SELECT PASSWORD FROM USERS WHERE EMAIL=$1 AND IDENT_MASTER IS NULL ORDER BY ID LIMIT 1")
                .bind(email).fetch_optional(&self.pool).await?;
            Ok(stored
                .flatten()
                .map(|hash| crate::auth::authenticate_user_sha1(password, &hash))
                .unwrap_or(false))
        })
    }
    fn authenticate_user_v2<'a>(&'a self, email: &'a str, password: &'a str) -> BoxFut<'a, bool> {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT PWD,SALT FROM USERS WHERE EMAIL=$1 AND IDENT_MASTER IS NULL LIMIT 2",
            )
            .bind(email)
            .fetch_all(&self.pool)
            .await?;
            ensure!(rows.len() <= 1, "useraccount.toomanyaccounts.found");
            let Some(row) = rows.first() else {
                return Ok(false);
            };
            Ok(match (text(row, 0)?, text(row, 1)?) {
                (Some(hash), Some(salt)) => {
                    crate::auth::hash_password_sha256(&salt, password) == hash
                }
                _ => false,
            })
        })
    }
    fn fetch_all_users(&self) -> BoxFut<'_, Vec<Value>> {
        Box::pin(async move {
            sqlx::query(&format!(
                "SELECT {USER_COLS},{LC_COLS} FROM USERS WHERE IDENT_MASTER IS NULL ORDER BY ID"
            ))
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|row| user_from_row(row, true))
            .collect()
        })
    }
    fn fetch_user_by_id<'a>(&'a self, id: i64) -> BoxFut<'a, Option<Value>> {
        Box::pin(async move {
            sqlx::query(&format!(
                "SELECT {USER_COLS},{LC_COLS} FROM USERS WHERE ID=$1"
            ))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .as_ref()
            .map(|row| user_from_row(row, true))
            .transpose()
        })
    }
    fn create_user<'a>(&'a self, req: UsersRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let value = save_user(&mut tx, req, true).await?;
            tx.commit().await?;
            Ok(value)
        })
    }
    fn update_user<'a>(&'a self, req: UsersRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let value = save_user(&mut tx, req, false).await?;
            tx.commit().await?;
            Ok(value)
        })
    }
    fn change_password<'a>(&'a self, id: i64, new_password: &'a str) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            lc_sql::lock_master(&mut tx, "USERS", id).await?;
            let lc = load_lifecycle(&mut tx, "USERS", id).await?;
            let mut fields = user_fields(&mut tx, id).await?;
            let creates_revision = lc.ident_master.is_none()
                && !(lc.activation_type.as_ref().map(|v| v.0) == Some(5)
                    && lc_sql::has_future(&mut tx, "USERS", id).await?);
            let salt = crate::auth::new_password_salt();
            for (key, value) in &mut fields {
                match *key {
                    "PWD" => *value = Some(crate::auth::hash_password_sha256(&salt, new_password)),
                    "SALT" => *value = Some(salt.clone()),
                    "PASSWORD" => *value = Some(crate::auth::hash_password_sha1(new_password)),
                    _ => {}
                }
            }
            let id =
                lc_sql::persist(&mut tx, "USERS", "users_sequence", id, lc, fields, false).await?;
            let value = write_entity_response(&mut tx, "USERS", id, creates_revision).await?;
            tx.commit().await?;
            Ok(value)
        })
    }
    fn delete_user<'a>(&'a self, id: i64) -> BoxFut<'a, bool> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            lc_sql::lock_master(&mut tx, "USERS", id).await?;
            let lc = load_lifecycle(&mut tx, "USERS", id).await?;
            ensure!(
                lc.revision_state.as_ref().map(|v| v.0) == Some(1)
                    && lc.revision_effective_from.is_none()
                    && lc.revision_effective_to.is_none(),
                "lifecycle.not.allowed.to.delete.revision"
            );
            sqlx::query("UPDATE USERS SET IDENT_REVISIONSTATE=6 WHERE ID=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok(true)
        })
    }
    fn fetch_akun_keuangan_by_email<'a>(&'a self, email: &'a str) -> BoxFut<'a, Vec<Value>> {
        Box::pin(async move {
            sqlx::query(&format!("SELECT {AKUN_COLS},{LC_COLS} FROM AKUNKEUANGAN ak WHERE ak.ID_USER IN \
                (SELECT u.ID FROM USERS u WHERE u.EMAIL=$1) AND ak.IDENT_MASTER IS NULL ORDER BY ak.ID"))
                .bind(email).fetch_all(&self.pool).await?.iter().map(|row| akun_from_row(row,true)).collect()
        })
    }
    fn fetch_akun_by_id<'a>(&'a self, id: i64) -> BoxFut<'a, Option<Value>> {
        Box::pin(async move { load_akun_value(&mut *self.pool.acquire().await?, id).await })
    }
    fn create_akun_keuangan<'a>(
        &'a self,
        req: AkunKeuanganRequest,
        email: &'a str,
    ) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let value = save_account(&mut tx, req, email, true).await?;
            tx.commit().await?;
            Ok(value)
        })
    }
    fn update_akun_keuangan<'a>(&'a self, req: AkunKeuanganRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let value = save_account(&mut tx, req, "", false).await?;
            tx.commit().await?;
            Ok(value)
        })
    }
    fn fetch_categories<'a>(&'a self, id_akun: i64, email: &'a str) -> BoxFut<'a, Vec<Value>> {
        Box::pin(async move {
            let mut conn = self.pool.acquire().await?;
            let rows = sqlx::query(&format!("SELECT {CATEGORY_COLS} FROM CATEGORY c WHERE c.ID_AKUNTRANSAKSI=$1 \
                AND EXISTS(SELECT 1 FROM AKUNKEUANGAN ak,USERS u WHERE u.ID=ak.ID_USER AND ak.ID=$1 AND u.EMAIL=$2) ORDER BY c.CREATEDDATE DESC"))
                .bind(id_akun).bind(email).fetch_all(&mut *conn).await?;
            let account = if rows.is_empty() {
                None
            } else {
                load_akun_value(&mut conn, id_akun).await?
            };
            rows.iter()
                .map(|row| category_from_row(row, account.clone()))
                .collect()
        })
    }
    fn fetch_category_detail<'a>(
        &'a self,
        id_akun: i64,
        id_c: i64,
        email: &'a str,
    ) -> BoxFut<'a, Option<Value>> {
        Box::pin(async move {
            let mut conn = self.pool.acquire().await?;
            let Some(row) = sqlx::query(&format!("SELECT {CATEGORY_COLS} FROM CATEGORY c WHERE c.ID=$1 AND c.ID_AKUNTRANSAKSI=$2 \
                AND EXISTS(SELECT 1 FROM AKUNKEUANGAN ak,USERS u WHERE u.ID=ak.ID_USER AND ak.ID=$2 AND u.EMAIL=$3)"))
                .bind(id_c).bind(id_akun).bind(email).fetch_optional(&mut *conn).await? else { return Ok(None); };
            Ok(Some(category_from_row(
                &row,
                load_akun_value(&mut conn, id_akun).await?,
            )?))
        })
    }
    fn create_category<'a>(&'a self, req: CategoryRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let value = save_category(&mut tx, req, true).await?;
            tx.commit().await?;
            Ok(value)
        })
    }
    fn update_category<'a>(&'a self, req: CategoryRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let value = save_category(&mut tx, req, false).await?;
            tx.commit().await?;
            Ok(value)
        })
    }
    fn fetch_transaksi_by_akun<'a>(
        &'a self,
        id_akun: i64,
        email: &'a str,
    ) -> BoxFut<'a, Vec<Value>> {
        Box::pin(async move {
            let mut conn = self.pool.acquire().await?;
            let list = load_transaction_list(&mut conn, id_akun, email).await?;
            drop(conn);
            tokio::task::spawn_blocking(move || list.into_values()).await?
        })
    }
    fn fetch_transaksi_list_json<'a>(&'a self, id_akun: i64, email: &'a str) -> BoxFut<'a, Vec<u8>> {
        Box::pin(async move {
            let mut conn = self.pool.acquire().await?;
            let list = load_transaction_list(&mut conn, id_akun, email).await?;
            drop(conn); // CPU encoding must not hold a DB connection.
            tokio::task::spawn_blocking(move || list.into_json_bytes(id_akun)).await?
        })
    }
    fn fetch_transaksi_detail<'a>(
        &'a self,
        id_akun: i64,
        id_tk: i64,
        email: &'a str,
    ) -> BoxFut<'a, Option<Value>> {
        Box::pin(async move {
            let mut conn = self.pool.acquire().await?;
            let Some(row) = sqlx::query(&format!("SELECT {TK_COLS} FROM TRANSAKSIKEUANGAN tk WHERE tk.ID=$1 AND tk.ID_AKUNTRANSAKSI=$2 \
                AND EXISTS(SELECT 1 FROM AKUNKEUANGAN ak,USERS u WHERE u.ID=ak.ID_USER AND ak.IDENT_MASTER IS NULL AND ak.ID=$2 AND u.EMAIL=$3)"))
                .bind(id_tk).bind(id_akun).bind(email).fetch_optional(&mut *conn).await? else { return Ok(None); };
            Ok(Some(transaksi_from_row(
                &row,
                transaksi_category(&mut conn, id_tk).await?,
            )?))
        })
    }
    fn create_transaksi<'a>(&'a self, req: TransaksiKeuanganRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let value = save_transaksi(&mut tx, req, true).await?;
            tx.commit().await?;
            Ok(value)
        })
    }
    fn update_transaksi<'a>(&'a self, req: TransaksiKeuanganRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let value = save_transaksi(&mut tx, req, false).await?;
            tx.commit().await?;
            Ok(value)
        })
    }
    fn fetch_transaksi_not_in_settlement_engine(&self) -> BoxFut<'_, Vec<Value>> {
        Box::pin(async move {
            let ids: Vec<i64> = sqlx::query_scalar("SELECT ID FROM TRANSAKSIKEUANGAN WHERE INTERNALREFERENCE IS NOT NULL AND INTERNALREFERENCE NOT IN (SELECT INSTRUCTIONREFERENCE FROM CSV_SETTLEMENTINSTRUCTION_AKUNKEUANGAN)")
                .fetch_all(&self.pool).await?;
            Ok(ids.into_iter().map(|id| json!({"id":id})).collect())
        })
    }
    fn fetch_event_entries<'a>(&'a self, event_id: &'a str) -> BoxFut<'a, Vec<Value>> {
        Box::pin(async move {
            let rows = sqlx::query("SELECT IDENT_AUDITEVENTENTRY,ENTRYNAME,TO_CHAR(ENTRYTIMESTAMP,'YYYY-MM-DD HH24:MI:SS'),NANOSECONDSONTIMESTAMP,BUSINESSIDENTIFIER,ENTRYDATA,COMPRESSEDENTRYDATA,ENTITYNAME,ENTITYID,MASTERID,REVISIONID FROM AUDITEVENTENTRY WHERE EVENTID=$1 ORDER BY IDENT_AUDITEVENTENTRY")
                .bind(event_id).fetch_all(&self.pool).await?;
            rows.iter()
                .map(|row| {
                    super::audit_gson::entry_json(
                        row.try_get(0)?,
                        event_id,
                        text(row, 1)?,
                        text(row, 2)?,
                        row.try_get(3)?,
                        text(row, 4)?,
                        text(row, 5)?,
                        row.try_get(6)?,
                        text(row, 7)?,
                        row.try_get(8)?,
                        row.try_get(9)?,
                        row.try_get(10)?,
                    )
                })
                .collect()
        })
    }
}
