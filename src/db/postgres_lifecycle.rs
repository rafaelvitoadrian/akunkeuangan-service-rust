//! PostgreSQL persistence with the same master/revision decisions as Oracle.
//! The repository owns the transaction, so failures roll back the entire write.
use super::json_shape::Lifecycle;
use anyhow::{anyhow, ensure, Result};
use sqlx::PgConnection;

pub(super) type Fields = Vec<(&'static str, Option<String>)>;

fn expression(name: &str, bind: usize) -> String {
    let ty = match name {
        "REQUESTED_ACTIVATION_DATE" => "date",
        "REVISION_EFFECTIVE_FROM"
        | "REVISION_EFFECTIVE_TO"
        | "LASTUPDATETIME"
        | "CREATEDDATE"
        | "UPDATEDDATE"
        | "TRANSAKSI_DATE" => "timestamp",
        "SALDO" | "NOMINAL" | "JENIS" | "CATEGORY_TYPE" => "numeric",
        "IDENT_MASTER"
        | "IDENT_REVISION"
        | "IDENT_ACTIVATIONTYPE"
        | "IDENT_LASTUPDATEUSER"
        | "IDENT_REVISIONSTATE"
        | "ID_USER"
        | "ID_AKUNTRANSAKSI"
        | "CATEGORY_ID" => "bigint",
        _ => "text",
    };
    // Bind text explicitly before the cast: NUMERIC never passes through f64.
    // Oracle empty strings persist as NULL; retain that behavior on PostgreSQL.
    format!("CAST(NULLIF(${bind}::text, '') AS {ty})")
}

pub(super) async fn write_fields(
    conn: &mut PgConnection,
    table: &str,
    id: i64,
    fields: &Fields,
    insert: bool,
) -> Result<()> {
    // Table/column names only come from repository constants, never HTTP input.
    let sql = if insert {
        format!(
            "INSERT INTO {table} ({}, ID) VALUES ({}, ${})",
            fields
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(","),
            fields
                .iter()
                .enumerate()
                .map(|(i, (name, _))| expression(name, i + 1))
                .collect::<Vec<_>>()
                .join(","),
            fields.len() + 1
        )
    } else {
        format!(
            "UPDATE {table} SET {} WHERE ID=${}",
            fields
                .iter()
                .enumerate()
                .map(|(i, (name, _))| format!("{name}={}", expression(name, i + 1)))
                .collect::<Vec<_>>()
                .join(","),
            fields.len() + 1
        )
    };
    let mut query = sqlx::query(&sql);
    for (_, value) in fields {
        query = query.bind(value);
    }
    let result = query.bind(id).execute(conn).await?;
    ensure!(result.rows_affected() == 1, "entity not found");
    Ok(())
}

async fn write(
    conn: &mut PgConnection,
    table: &str,
    id: i64,
    lc: &Lifecycle,
    fields: &Fields,
    insert: bool,
) -> Result<()> {
    let mut fields = fields.clone();
    fields.extend([
        ("IDENT_MASTER", lc.ident_master.map(|v| v.to_string())),
        ("IDENT_REVISION", lc.ident_revision.map(|v| v.to_string())),
        (
            "REVISION_EFFECTIVE_FROM",
            lc.revision_effective_from.clone(),
        ),
        ("REVISION_EFFECTIVE_TO", lc.revision_effective_to.clone()),
        (
            "REQUESTED_ACTIVATION_DATE",
            lc.requested_activation_date.clone(),
        ),
        (
            "IDENT_ACTIVATIONTYPE",
            lc.activation_type.as_ref().map(|v| v.0.to_string()),
        ),
        (
            "IDENT_LASTUPDATEUSER",
            lc.last_update_user_ident.map(|v| v.to_string()),
        ),
        ("LASTUPDATETIME", lc.last_update_time.clone()),
        (
            "IDENT_REVISIONSTATE",
            lc.revision_state.as_ref().map(|v| v.0.to_string()),
        ),
    ]);
    write_fields(conn, table, id, &fields, insert).await
}

pub(super) async fn next_seq(conn: &mut PgConnection, sequence: &str) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT nextval($1::regclass)")
        .bind(sequence)
        .fetch_one(conn)
        .await?)
}

pub(super) async fn has_future(conn: &mut PgConnection, table: &str, master: i64) -> Result<bool> {
    Ok(sqlx::query_scalar(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE IDENT_MASTER=$1 AND REVISION_EFFECTIVE_FROM IS NULL AND IDENT_REVISIONSTATE IN (1,2,8))"))
        .bind(master).fetch_one(conn).await?)
}

/// Serialize revisions of a master, including writes addressed to a revision ID.
pub(super) async fn lock_master(conn: &mut PgConnection, table: &str, id: i64) -> Result<()> {
    let master: i64 = sqlx::query_scalar(&format!(
        "SELECT COALESCE(IDENT_MASTER, ID) FROM {table} WHERE ID=$1"
    ))
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(&format!("SELECT ID FROM {table} WHERE ID=$1 FOR UPDATE"))
        .bind(master)
        .fetch_one(conn)
        .await?;
    Ok(())
}

pub(super) async fn persist(
    conn: &mut PgConnection,
    table: &str,
    sequence: &str,
    input_id: i64,
    mut lc: Lifecycle,
    fields: Fields,
    create: bool,
) -> Result<i64> {
    use super::postgres_impl::load_lifecycle;
    let now = crate::util::app_now_string();
    if create {
        ensure!(
            input_id <= 0,
            "createMasterAndRevision: primary key provided"
        );
        lc.last_update_time = Some(now.clone());
        lc.revision_state = Some((1, "Ready".into()));
        let master_id = next_seq(conn, sequence).await?;
        let revision_id = next_seq(conn, sequence).await?;
        let mut revision = lc.clone();
        revision.ident_master = Some(master_id);
        revision.ident_revision = None;
        revision.revision_effective_from = Some(now.clone());
        let mut revision_fields = fields.clone();
        if table == "USERS" {
            revision_fields
                .iter_mut()
                .filter(|(name, _)| *name == "CREATEDDATE")
                .for_each(|(_, value)| *value = Some(now.clone()));
        }
        let mut initial_master = lc.clone();
        initial_master.activation_type = Some((5, "InActive".into()));
        write(conn, table, master_id, &initial_master, &fields, true).await?;
        write(conn, table, revision_id, &revision, &revision_fields, true).await?;
        lc.ident_master = None;
        lc.ident_revision = Some(revision_id);
        lc.revision_effective_from = Some(now);
        write(conn, table, master_id, &lc, &fields, false).await?;
        return Ok(master_id);
    }
    ensure!(input_id > 0, "updateLifecycled: missing primary key");
    ensure!(lc.activation_type.is_some(), "null activation type");
    let original = load_lifecycle(conn, table, input_id).await?;
    let master_id = original.ident_master.unwrap_or(input_id);
    let master = load_lifecycle(conn, table, master_id).await?;
    let future = has_future(conn, table, master_id).await?;
    let (revision_id, inserting) = if original.ident_master.is_some() {
        (input_id, false)
    } else if lc.activation_type.as_ref().map(|v| v.0) == Some(5) && future {
        let revision_id = master
            .ident_revision
            .ok_or_else(|| anyhow!("missing future revision"))?;
        let mut pending = load_lifecycle(conn, table, revision_id).await?;
        pending.revision_state = lc.revision_state.clone();
        pending.last_update_user_ident = lc.last_update_user_ident;
        pending.last_update_time = lc.last_update_time.clone();
        pending.requested_activation_date = lc.requested_activation_date.clone();
        write(conn, table, revision_id, &pending, &fields, false).await?;
        if table=="USERS" { super::postgres_security::copy_assignments(conn,master_id,revision_id).await?; }
        write(conn, table, master_id, &lc, &fields, false).await?;
        return Ok(input_id);
    } else {
        ensure!(!future, "lifecycleentity.pending.future.revisions");
        let id = next_seq(conn, sequence).await?;
        lc.ident_revision = None;
        lc.revision_effective_from = None;
        lc.last_update_time = Some(now.clone());
        (id, true)
    };
    lc.ident_master = Some(master_id);
    lc.ident_revision = None;
    let effective = lc.revision_effective_from.is_none()
        && (table == "AKUNKEUANGAN" || lc.revision_state.as_ref().map(|v| v.0) == Some(1));
    let mut revision_fields = fields.clone();
    if inserting && table == "USERS" {
        revision_fields
            .iter_mut()
            .filter(|(name, _)| *name == "CREATEDDATE")
            .for_each(|(_, value)| *value = Some(now.clone()));
    }
    if effective {
        lc.revision_effective_from = Some(now.clone());
    }
    write(conn, table, revision_id, &lc, &revision_fields, inserting).await?;
    if table=="USERS" { super::postgres_security::copy_assignments(conn,input_id,revision_id).await?; }
    if effective {
        if let Some(old) = master.ident_revision.filter(|v| *v != revision_id) {
            sqlx::query(&format!(
                "UPDATE {table} SET REVISION_EFFECTIVE_TO=$1::text::timestamp WHERE ID=$2"
            ))
            .bind(&now)
            .bind(old)
            .execute(&mut *conn)
            .await?;
        }
        let mut new_master = lc;
        new_master.ident_master = None;
        new_master.ident_revision = Some(revision_id);
        let mut master_fields = fields.clone();
        for (name, value) in &mut master_fields {
            if table != "USERS" && matches!(*name, "CREATEDDATE" | "UPDATEDDATE") {
                *value = None;
            }
        }
        write(conn, table, master_id, &new_master, &master_fields, false).await?;
        if table=="USERS" { super::postgres_security::copy_assignments(conn,revision_id,master_id).await?; }
    } else if master.activation_type.as_ref().map(|v| v.0) == Some(5) {
        let mut copied = master;
        copied.last_update_time = lc.last_update_time;
        copied.last_update_user_ident = lc.last_update_user_ident;
        copied.requested_activation_date = lc.requested_activation_date;
        write(conn, table, master_id, &copied, &fields, false).await?;
        if table=="USERS" { super::postgres_security::copy_assignments(conn,revision_id,master_id).await?; }
    }
    Ok(input_id)
}
