//! Shared SQL lifecycle implementation, matching JPALifecycleEntityPersister.
//! All statements stay on the caller's connection; the caller commits once.
use super::json_shape::Lifecycle;
use anyhow::{anyhow, Result};
use oracle::{Connection, sql_type::ToSql};

pub(super) type Fields = Vec<(&'static str, Option<String>)>;

fn expression(name: &str, bind: usize) -> String {
    if name == "REQUESTED_ACTIVATION_DATE" {
        format!("TO_DATE(:{}, 'YYYY-MM-DD')", bind)
    } else if matches!(name, "REVISION_EFFECTIVE_FROM" | "REVISION_EFFECTIVE_TO" | "LASTUPDATETIME" | "CREATEDDATE" | "UPDATEDDATE" | "TRANSAKSI_DATE") {
        format!("TO_TIMESTAMP(:{}, 'YYYY-MM-DD HH24:MI:SS.FF9')", bind)
    } else { format!(":{}", bind) }
}

pub(super) fn write(conn: &Connection, table: &str, id: i64, lc: &Lifecycle, fields: &Fields, insert: bool) -> Result<()> {
    let mut fields = fields.clone();
    fields.extend([
        ("IDENT_MASTER", lc.ident_master.map(|v| v.to_string())),
        ("IDENT_REVISION", lc.ident_revision.map(|v| v.to_string())),
        ("REVISION_EFFECTIVE_FROM", lc.revision_effective_from.clone()),
        ("REVISION_EFFECTIVE_TO", lc.revision_effective_to.clone()),
        ("REQUESTED_ACTIVATION_DATE", lc.requested_activation_date.clone()),
        ("IDENT_ACTIVATIONTYPE", lc.activation_type.as_ref().map(|v| v.0.to_string())),
        ("IDENT_LASTUPDATEUSER", lc.last_update_user_ident.map(|v| v.to_string())),
        ("LASTUPDATETIME", lc.last_update_time.clone()),
        ("IDENT_REVISIONSTATE", lc.revision_state.as_ref().map(|v| v.0.to_string())),
    ]);
    write_fields(conn,table,id,&fields,insert)
}

pub(super) fn write_fields(conn:&Connection,table:&str,id:i64,fields:&Fields,insert:bool)->Result<()> {
    let mut binds: Vec<&dyn ToSql> = fields.iter().map(|(_,v)| v as &dyn ToSql).collect();
    binds.push(&id);
    let sql = if insert {
        format!("INSERT INTO {} ({}, ID) VALUES ({}, :{})", table,
            fields.iter().map(|(k,_)| *k).collect::<Vec<_>>().join(","),
            fields.iter().enumerate().map(|(i,(k,_))| expression(k,i+1)).collect::<Vec<_>>().join(","), binds.len())
    } else {
        format!("UPDATE {} SET {} WHERE ID=:{}", table,
            fields.iter().enumerate().map(|(i,(k,_))| format!("{}={}",k,expression(k,i+1))).collect::<Vec<_>>().join(","), binds.len())
    };
    conn.execute(&sql, &binds)?;
    Ok(())
}

pub(super) fn has_future(conn: &Connection, table: &str, master: i64) -> Result<bool> {
    let row=conn.query_row(&format!("SELECT COUNT(*) FROM {} WHERE IDENT_MASTER=:1 AND REVISION_EFFECTIVE_FROM IS NULL AND IDENT_REVISIONSTATE IN (1,2,8)",table),&[&master])?;
    Ok(row.get::<usize,i64>(0)?>0)
}

/// Returns the input entity ID, because the Java HTTP controller re-fetches it.
pub(super) fn persist(conn: &Connection, table: &str, sequence: &str,
    input_id: i64, mut lc: Lifecycle, fields: Fields, create: bool,
    load: impl Fn(i64) -> Result<Lifecycle>, next: impl Fn() -> Result<i64>) -> Result<i64> {
    let _ = sequence; // sequence selected by the caller, never supplied by HTTP
    let now = crate::util::app_now_string();
    if create {
        if input_id > 0 { return Err(anyhow!("createMasterAndRevision: primary key provided")); }
        lc.last_update_time = Some(now.clone());
        lc.revision_state = Some((1,"Ready".into()));
        let master_id = next()?;
        let revision_id = next()?;
        let mut revision = lc.clone();
        revision.ident_master = Some(master_id);
        revision.ident_revision = None;
        revision.revision_effective_from = Some(now.clone());
        let mut revision_fields = fields.clone();
        if table == "USERS" {
            revision_fields.iter_mut().filter(|(k,_)| *k=="CREATEDDATE")
                .for_each(|(_,v)| *v=Some(now.clone()));
        }
        // Insert the master first, as Java does, so self-referencing FKs work.
        let mut initial_master=lc.clone();
        initial_master.activation_type=Some((5,"InActive".into()));
        write(conn,table,master_id,&initial_master,&fields,true)?;
        write(conn, table, revision_id, &revision, &revision_fields, true)?;
        lc.ident_master = None;
        lc.ident_revision = Some(revision_id);
        lc.revision_effective_from = Some(now);
        write(conn, table, master_id, &lc, &fields, false)?;
        return Ok(master_id);
    }
    if input_id <= 0 { return Err(anyhow!("updateLifecycled: missing primary key")); }
    if lc.activation_type.is_none() { return Err(anyhow!("null activation type")); }
    let original = load(input_id)?;
    let master_id = original.ident_master.unwrap_or(input_id);
    let master = load(master_id)?;
    let future = has_future(conn,table,master_id)?;
    let revision_id;
    let inserting;
    if original.ident_master.is_some() {
        revision_id = input_id; inserting = false;
    } else if lc.activation_type.as_ref().map(|v|v.0)==Some(5) && future {
        // Java merges the pending revision without calling registerUpdate;
        // unlike updateRevision, this must not make it effective yet.
        revision_id = master.ident_revision.ok_or_else(|| anyhow!("missing future revision"))?;
        let mut pending=load(revision_id)?;
        pending.revision_state=lc.revision_state.clone();
        pending.last_update_user_ident=lc.last_update_user_ident;
        pending.last_update_time=lc.last_update_time.clone();
        pending.requested_activation_date=lc.requested_activation_date.clone();
        write(conn,table,revision_id,&pending,&fields,false)?;
        write(conn,table,master_id,&lc,&fields,false)?;
        return Ok(input_id);
    } else {
        if future { return Err(anyhow!("lifecycleentity.pending.future.revisions")); }
        revision_id=next()?; inserting=true;
        lc.ident_revision=None;
        lc.revision_effective_from=None;
        lc.last_update_time=Some(now.clone());
    }
    lc.ident_master=Some(master_id);
    lc.ident_revision=None;
    let becomes_effective = lc.revision_effective_from.is_none() && (table=="AKUNKEUANGAN" || lc.revision_state.as_ref().map(|v|v.0)==Some(1));
    let mut revision_fields=fields.clone();
    if inserting && table=="USERS" {
        revision_fields.iter_mut().filter(|(k,_)| *k=="CREATEDDATE").for_each(|(_,v)| *v=Some(now.clone()));
    }
    if becomes_effective { lc.revision_effective_from=Some(now.clone()); }
    write(conn,table,revision_id,&lc,&revision_fields,inserting)?;
    if becomes_effective {
        if let Some(old) = master.ident_revision.filter(|v| *v!=revision_id) {
            conn.execute(&format!("UPDATE {} SET REVISION_EFFECTIVE_TO=TO_TIMESTAMP(:1,'YYYY-MM-DD HH24:MI:SS.FF9') WHERE ID=:2",table), &[&now,&old])?;
        }
        let mut new_master=lc;
        new_master.ident_master=None;
        new_master.ident_revision=Some(revision_id);
        let mut master_fields=fields.clone();
        for (name,value) in master_fields.iter_mut() {
            if matches!(*name,"CREATEDDATE" | "UPDATEDDATE") {*value=None;}
        }
        write(conn,table,master_id,&new_master,&master_fields,false)?;
    } else if master.activation_type.as_ref().map(|v|v.0)==Some(5) {
        let mut copied=master;
        copied.last_update_time=lc.last_update_time;
        copied.last_update_user_ident=lc.last_update_user_ident;
        copied.requested_activation_date=lc.requested_activation_date;
        write(conn,table,master_id,&copied,&fields,false)?;
    }
    Ok(input_id)
}
