//! Implementasi Oracle Repository — feature `oracle`.
//!
//! Memakai crate `oracle` (ODPI-C) yang memerlukan Oracle Instant Client.
//! Query diterjemahkan dari JPQL/native SQL di persister Java.
//!
//! Semantik lifecycle `createMasterAndRevision` **diverifikasi empiris** dengan
//! membandingkan baris DB sebelum/sesudah POST ke sistem lama (2026-10-18):
//!
//! master  ID=630038  IDENT_MASTER=null  IDENT_REVISION=630039
//!         REVISION_EFFECTIVE_FROM=now2  EFFECTIVE_TO=null
//!         IDENT_ACTIVATIONTYPE=null (disalin dari request, bukan dipaksa Active)
//!         IDENT_REVISIONSTATE=1  LASTUPDATETIME=now1
//! revision ID=630039 IDENT_MASTER=630038 IDENT_REVISION=null
//!         REVISION_EFFECTIVE_FROM=now2 (sama dengan master)
//!         IDENT_ACTIVATIONTYPE=null  IDENT_REVISIONSTATE=1  LASTUPDATETIME=now1
//!
//! Catatan: `now1` (lastUpdateTime) dan `now2` (revisionEffectiveFrom) adalah dua
//! pemanggilan LocalDateTime.now() berbeda, sehingga selisih beberapa ms.
//!
//! Semua akses blocking; dibungkus `spawn_blocking`.

use crate::db::json_shape::{self, Lifecycle};
use crate::db::Repository;
use crate::models::*;
use anyhow::{anyhow, Result};
use oracle::pool::{Pool, PoolBuilder};
use oracle::Connection;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

pub struct OracleRepo {
    main: Pool,
    audit: Pool,
}

impl OracleRepo {
    pub fn connect(
        main_connect: &str,
        main_user: &str,
        main_pass: &str,
        audit_connect: &str,
        audit_user: &str,
        audit_pass: &str,
    ) -> Result<Self> {
        // Hikari lama: minimum-idle 3, maximum-pool-size 15
        let main = PoolBuilder::new(main_user, main_pass, main_connect)
            .min_connections(3)
            .max_connections(15)
            .build()?;
        let audit = PoolBuilder::new(audit_user, audit_pass, audit_connect)
            .min_connections(1)
            .max_connections(5)
            .build()?;
        Ok(Self { main, audit })
    }

    fn conn(&self) -> Result<Connection> {
        let c = self.main.get()?;
        // DATE/TIMESTAMP dikonversi ODPI-C memakai zona sesi. JVM lama berjalan di
        // UTC, jadi sesi dipaksa UTC agar nilai terbaca identik.
        c.execute("ALTER SESSION SET TIME_ZONE = 'UTC'", &[]).ok();
        Ok(c)
    }

    fn audit_conn(&self) -> Result<Connection> {
        let c = self.audit.get()?;
        c.execute("ALTER SESSION SET TIME_ZONE = 'UTC'", &[]).ok();
        Ok(c)
    }
}

async fn blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f).await?
}

// ---------------------------------------------------------------------------
// Kolom & pemetaan
// ---------------------------------------------------------------------------

/// Kolom lifecycle. Timestamp dibaca sebagai string via TO_CHAR agar konversi
/// zona waktu tidak bergantung pada OCI/ODPI-C (nilai tersimpan dibaca apa adanya,
/// sama seperti JVM lama yang berjalan di UTC).
/// Nilai DATE/TIMESTAMP di DB ditulis oleh JVM yang berjalan di zona Asia/Jakarta
/// sebagai wall-clock polos (tanpa konversi). Sistem lama membacanya sebagai instant
/// lalu memformat dalam UTC — sehingga output API = nilai tersimpan − 7 jam.
/// Konversi berikut mereplikasi perilaku itu secara eksplisit di SQL.
macro_rules! utc_ts {
    ($col:expr, $fmt:expr) => {
        concat!(
            "TO_CHAR(FROM_TZ(CAST(",
            $col,
            " AS TIMESTAMP), 'Asia/Jakarta') AT TIME ZONE 'UTC', '",
            $fmt,
            "')"
        )
    };
}

const LC_COLS: &str = "IDENT_MASTER, IDENT_REVISION, \
     TO_CHAR(REVISION_EFFECTIVE_FROM, 'YYYY-MM-DD HH24:MI:SS.FF6'), \
     TO_CHAR(REVISION_EFFECTIVE_TO, 'YYYY-MM-DD HH24:MI:SS.FF6'), \
     TO_CHAR(REQUESTED_ACTIVATION_DATE, 'YYYY-MM-DD'), \
     IDENT_ACTIVATIONTYPE, IDENT_LASTUPDATEUSER, \
     TO_CHAR(LASTUPDATETIME, 'YYYY-MM-DD HH24:MI:SS.FF6'), \
     IDENT_REVISIONSTATE";

const USER_COLS: &str = concat!(
    "ID, NAME, EMAIL, USERNAME, PWD, PASSWORD, SALT, ",
    utc_ts!("CREATEDDATE", "YYYY-MM-DD HH24:MI:SS"),
    ", ",
    utc_ts!("UPDATEDDATE", "YYYY-MM-DD HH24:MI:SS")
);
const AKUN_COLS: &str = concat!(
    "ID, NAME, DESCRIPTION, ID_USER, SALDO, ACCOUNT_NUMBER, ",
    utc_ts!("CREATEDDATE", "YYYY-MM-DD HH24:MI:SS"),
    ", ",
    utc_ts!("UPDATEDDATE", "YYYY-MM-DD HH24:MI:SS")
);
const CATEGORY_COLS: &str = concat!(
    "ID, CATEGORY_NAME, CATEGORY_TYPE, ",
    utc_ts!("CREATEDDATE", "YYYY-MM-DD HH24:MI:SS.FF6"),
    ", ",
    utc_ts!("UPDATEDDATE", "YYYY-MM-DD HH24:MI:SS.FF6")
);
const TK_COLS: &str = concat!(
    "ID, NAME, DESCRIPTION, ID_AKUNTRANSAKSI, JENIS, NOMINAL, ",
    utc_ts!("CREATEDDATE", "YYYY-MM-DD HH24:MI:SS"),
    ", ",
    utc_ts!("UPDATEDDATE", "YYYY-MM-DD HH24:MI:SS"),
    ", ",
    utc_ts!("TRANSAKSI_DATE", "YYYY-MM-DD HH24:MI:SS"),
    ", EXTERNALREFERENCE, INTERNALREFERENCE"
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

/// Baca kolom lifecycle.
///
/// `strict_activation` mereplikasi BUG sistem lama:
/// `LifecycleEntityPO.identActivationType` dideklarasikan `private long`
/// (primitif) sementara kolom `IDENT_ACTIVATIONTYPE` nullable. Hibernate
/// melempar saat menghidrasi nilai NULL ke field primitif, sehingga endpoint
/// mengembalikan **500**.
///
/// Dibuktikan pada 2026-10-18: akun `LifecycleProbe` (IDENT_ACTIVATIONTYPE NULL)
/// membuat `GET /api/v1/akunkeuangan` dan `/akunkeuangan/{id}` menjadi 500 di
/// sistem lama, sementara baris lain tetap normal.
///
/// CATATAN: pemicunya tidak sepenuhnya konsisten — pada satu pengamatan endpoint
/// masih 200 walau baris NULL sudah ada, pada pengamatan lain 500. Karena itu
/// replikasi crash dibuat **opt-in** (`LEGACY_NULL_ACTIVATION_CRASH=true`).
/// Default: nilai NULL diperlakukan sebagai 0 (endpoint tetap berfungsi).
/// Lihat docs/FINDINGS.md #F1 dan docs/OPEN-QUESTIONS.md #1.
/// Replikasi BUG F1 (lihat docs/FINDINGS.md):
/// `LifecycleEntityPO.identActivationType` bertipe **primitif** `long` sementara
/// kolom DB nullable. Hibernate melempar saat menghidrasi NULL ke field primitif,
/// sehingga endpoint mengembalikan 500.
///
/// Apakah bug F1 direplikasi?
///
/// `LifecycleEntityPO.identActivationType` di Java bertipe **primitif** `long`
/// sementara kolom `IDENT_ACTIVATIONTYPE` nullable. Hibernate gagal menghidrasi
/// NULL ke field primitif → request 500.
///
/// Dibuktikan 2026-10-18: akun `LifecycleProbe` (activationtype NULL) membuat
/// `GET /api/v1/akunkeuangan` milik `admin1` menjadi 500; setelah baris itu
/// dihapus endpoint kembali 200.
///
/// Default: **replikasi aktif** (kompatibilitas 100%).
/// Matikan dengan `LEGACY_NULL_ACTIVATION_CRASH=false` bila ingin endpoint tetap
/// berfungsi meski ada data lama yang tidak lengkap.
fn legacy_null_activation_crash() -> bool {
    std::env::var("LEGACY_NULL_ACTIVATION_CRASH")
        .map(|v| !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true)
}

fn read_lifecycle(row: &oracle::Row, off: usize) -> Result<Lifecycle> {
    read_lifecycle_impl(row, off, true)
}

fn read_lifecycle_impl(row: &oracle::Row, off: usize, strict_activation: bool) -> Result<Lifecycle> {
    let ident_master: Option<i64> = row.get(off)?;
    let ident_revision: Option<i64> = row.get(off + 1)?;
    let eff_from: Option<String> = row.get(off + 2)?;
    let eff_to: Option<String> = row.get(off + 3)?;
    let req_act: Option<String> = row.get(off + 4)?;
    let act_type: Option<i64> = row.get(off + 5)?;
    if strict_activation && act_type.is_none() && legacy_null_activation_crash() {
        return Err(anyhow!(
            "could not read column value [ident_activationtype]: null value was assigned \
             to a property of primitive type (legacy behaviour: HTTP 500)"
        ));
    }
    let last_user: Option<i64> = row.get(off + 6)?;
    let last_time: Option<String> = row.get(off + 7)?;
    let rev_state: Option<i64> = row.get(off + 8)?;

    Ok(Lifecycle {
        ident_master,
        ident_revision,
        revision_effective_from: eff_from,
        revision_effective_to: eff_to,
        requested_activation_date: req_act,
        activation_type: act_type.map(|id| (id, activation_name(id).to_string())),
        ident_activation_type: act_type.unwrap_or(0),
        last_update_user_ident: last_user,
        last_update_time: last_time,
        revision_state: rev_state.map(|id| (id, revision_state_name(id).to_string())),
    })
}

fn user_from_row(row: &oracle::Row) -> Result<Value> {
    user_from_row_impl(row,true)
}

fn user_from_row_impl(row: &oracle::Row, strict: bool) -> Result<Value> {
    let id: i64 = row.get(0)?;
    let lc = read_lifecycle_impl(row, 9, strict)?;
    Ok(json_shape::user_json(
        id,
        &lc,
        row.get::<usize, Option<String>>(1)?.as_deref(),
        row.get::<usize, Option<String>>(2)?.as_deref(),
        row.get::<usize, Option<String>>(3)?.as_deref(),
        row.get::<usize, Option<String>>(4)?.as_deref(),
        row.get::<usize, Option<String>>(5)?.as_deref(),
        row.get::<usize, Option<String>>(6)?.as_deref(),
        (row.get::<usize, Option<String>>(7)?).as_deref(),
        (row.get::<usize, Option<String>>(8)?).as_deref(),
    ))
}

fn akun_from_row(row: &oracle::Row) -> Result<Value> {
    akun_from_row_impl(row,true)
}

fn akun_from_row_impl(row: &oracle::Row, strict: bool) -> Result<Value> {
    let id: i64 = row.get(0)?;
    let lc = read_lifecycle_impl(row, 8, strict)?;
    Ok(json_shape::akun_json(
        id,
        &lc,
        row.get::<usize, Option<String>>(1)?.as_deref(),
        row.get::<usize, Option<String>>(2)?.as_deref(),
        row.get::<usize, Option<i64>>(3)?,
        row.get::<usize, Option<String>>(4)?.as_deref(),
        row.get::<usize, Option<String>>(5)?.as_deref(),
        (row.get::<usize, Option<String>>(6)?).as_deref(),
        (row.get::<usize, Option<String>>(7)?).as_deref(),
    ))
}

fn category_from_row(row: &oracle::Row, akun: Option<Value>) -> Result<Value> {
    let id: i64 = row.get(0)?;
    Ok(json_shape::category_json(
        id,
        row.get::<usize, Option<String>>(1)?.as_deref(),
        row.get::<usize, Option<String>>(2)?.as_deref(),
        akun,
        (row.get::<usize, Option<String>>(3)?).as_deref(),
        (row.get::<usize, Option<String>>(4)?).as_deref(),
    ))
}

/// Muat satu kategori lengkap (termasuk akunKeuangan nested), dipakai sebagai
/// nilai `category` pada transaksi. Java menyerialisasi relasi LAZY ini penuh.
fn load_category_value(conn: &Connection, id: i64) -> Result<Option<Value>> {
    let sql = format!("SELECT {} FROM CATEGORY WHERE ID = :1", CATEGORY_COLS);
    let row = match conn.query(&sql, &[&id])?.next() {
        Some(Ok(r)) => r,
        _ => return Ok(None),
    };
    let akun_id: Option<i64> = match conn
        .query("SELECT ID_AKUNTRANSAKSI FROM CATEGORY WHERE ID = :1", &[&id])?
        .next()
    {
        Some(Ok(r)) => r.get(0)?,
        _ => None,
    };
    let akun = match akun_id {
        Some(a) => load_akun_value(conn, a)?,
        None => None,
    };
    Ok(Some(category_from_row(&row, akun)?))
}

/// Ambil `category_id` sebuah transaksi lalu muat kategorinya.
fn transaksi_category(conn: &Connection, transaksi_id: i64) -> Result<Option<Value>> {
    let cat_id: Option<i64> = match conn
        .query(
            "SELECT CATEGORY_ID FROM TRANSAKSIKEUANGAN WHERE ID = :1",
            &[&transaksi_id],
        )?
        .next()
    {
        Some(Ok(r)) => r.get(0)?,
        _ => None,
    };
    match cat_id {
        Some(c) => load_category_value(conn, c),
        None => Ok(None),
    }
}

fn transaksi_from_row(row: &oracle::Row, category: Option<Value>) -> Result<Value> {
    let id: i64 = row.get(0)?;
    Ok(json_shape::transaksi_json(
        id,
        row.get::<usize, Option<String>>(1)?.as_deref(),
        row.get::<usize, Option<String>>(2)?.as_deref(),
        row.get::<usize, Option<i64>>(3)?,
        category,
        row.get::<usize, Option<String>>(4)?.as_deref(),
        row.get::<usize, Option<String>>(5)?.as_deref(),
        (row.get::<usize, Option<String>>(6)?).as_deref(),
        (row.get::<usize, Option<String>>(7)?).as_deref(),
        (row.get::<usize, Option<String>>(8)?).as_deref(),
        row.get::<usize, Option<String>>(9)?.as_deref(),
        row.get::<usize, Option<String>>(10)?.as_deref(),
    ))
}

// ---------------------------------------------------------------------------
// Helper SQL
// ---------------------------------------------------------------------------

fn next_seq(conn: &Connection, seq: &str) -> Result<i64> {
    let sql = format!("SELECT {}.NEXTVAL FROM DUAL", seq);
    let row = conn.query(&sql, &[])?.next().ok_or_else(|| anyhow!("no row"))??;
    Ok(row.get(0)?)
}


fn load_akun_value(conn: &Connection, id: i64) -> Result<Option<Value>> {
    let sql = format!("SELECT {}, {} FROM AKUNKEUANGAN WHERE ID = :1", AKUN_COLS, LC_COLS);
    match conn.query(&sql, &[&id])?.next() {
        Some(Ok(r)) => Ok(Some(akun_from_row(&r)?)),
        _ => Ok(None),
    }
}

/// Set identRevision + effectiveFrom pada master, dan effectiveFrom pada revision.
// ---------------------------------------------------------------------------
// Repository impl
// ---------------------------------------------------------------------------

fn converted_lifecycle(conn: &Connection, table: &str, input: &crate::lifecycle::LifecycleRequest,
    actor: Option<i64>, create: bool) -> Result<Lifecycle> {
    let mut lc=if input.ident_entity>0 { load_lifecycle(conn,table,input.ident_entity)? } else { Lifecycle::default() };
    if create && input.ident_entity>0 { return Err(anyhow!("primary key supplied for create")); }
    lc.activation_type=match input.activation_id() {
        Some(id)=> {
            let row=conn.query_row("SELECT TYPENAME FROM ACTIVATIONTYPE WHERE IDENT_ACTIVATIONTYPE=:1",&[&id])?;
            Some((id,row.get(0)?))
        }, None=>None,
    };
    lc.ident_activation_type=lc.activation_type.as_ref().map(|v|v.0).unwrap_or(0);
    lc.requested_activation_date=input.requested_activation_date.map(|v|v.to_string());
    lc.last_update_user_ident=actor;
    Ok(lc)
}

fn write_entity_response(conn:&Connection,table:&str,id:i64,transient_activation:bool)->Result<Value> {
    let cols=if table=="USERS" {USER_COLS} else {AKUN_COLS};
    let row=conn.query_row(&format!("SELECT {},{} FROM {} WHERE ID=:1",cols,LC_COLS,table),&[&id])?;
    let mut value=if table=="USERS" {user_from_row_impl(&row,false)?} else {akun_from_row_impl(&row,false)?};
    if transient_activation {value["identActivationType"]=serde_json::json!(0);}
    Ok(value)
}

fn user_fields(conn:&Connection,id:i64)->Result<super::oracle_lifecycle::Fields> {
    let row=conn.query_row("SELECT NAME,EMAIL,USERNAME,PWD,PASSWORD,SALT,TO_CHAR(CREATEDDATE,'YYYY-MM-DD HH24:MI:SS'),TO_CHAR(UPDATEDDATE,'YYYY-MM-DD HH24:MI:SS') FROM USERS WHERE ID=:1",&[&id])?;
    let mut fields=Vec::new();
    for (index,name) in ["NAME","EMAIL","USERNAME","PWD","PASSWORD","SALT"].iter().enumerate() {
        fields.push((*name,row.get::<usize,Option<String>>(index)?));
    }
    fields.extend([("CREATEDDATE",row.get(6)?),("UPDATEDDATE",row.get(7)?)]);
    Ok(fields)
}

fn save_user(conn:&Connection,req:UsersRequest,create:bool)->Result<Value> {
    let input=crate::lifecycle::LifecycleRequest::from_value(&req.lifecycle_entity)?;
    let lc=converted_lifecycle(conn,"USERS",&input,req.actor_ident,create)?;
    let creates_revision = !create && lc.ident_master.is_none()
        && !(lc.activation_type.as_ref().map(|v|v.0)==Some(5)
            && super::oracle_lifecycle::has_future(conn,"USERS",input.ident_entity)?);
    let (salt,pwd,password)=if create || creates_revision {
        let password=req.password.as_deref().ok_or_else(||anyhow!("null password"))?;
        let salt=crate::auth::new_password_salt();
        let hash=crate::auth::hash_password_sha256(&salt,password);
        (Some(salt),Some(hash),Some(crate::auth::hash_password_sha1(password)))
    } else {
        let row=conn.query_row("SELECT SALT,PWD FROM USERS WHERE ID=:1",&[&input.ident_entity])?;
        let salt:Option<String>=row.get(0)?;
        let hash:Option<String>=row.get(1)?;
        match req.password.as_deref() {
            Some(p) if !p.is_empty()=> {
                let salt=salt.ok_or_else(||anyhow!("null salt"))?;
                if hash.as_deref()==Some(&crate::auth::hash_password_sha256(&salt,p)) {
                    (Some(salt),hash,req.password.clone())
                } else {
                    let salt=crate::auth::new_password_salt();
                    let hash=crate::auth::hash_password_sha256(&salt,p);
                    (Some(salt),Some(hash),req.password.clone())
                }
            },
            _=>(salt,hash,req.password.clone()),
        }
    };
    // Java updateUsers preserves PWD/SALT for an omitted password but writes
    // the input PASSWORD verbatim (including null). Preserve that legacy contract.
    let fields=vec![("NAME",req.name.clone()),("EMAIL",req.email.clone()),("USERNAME",req.username.clone()),
        ("PWD",pwd),("PASSWORD",password),("SALT",salt),("CREATEDDATE",None),("UPDATEDDATE",None)];
    let id=super::oracle_lifecycle::persist(conn,"USERS","USERS_SEQUENCE",input.ident_entity,lc,fields,create,
        |id|load_lifecycle(conn,"USERS",id),||next_seq(conn,"USERS_SEQUENCE"))?;
    let mut value=write_entity_response(conn,"USERS",id,create || creates_revision)?;
    // Oracle stores empty strings as NULL; the managed Java write response
    // still contains the request's empty strings until a subsequent GET.
    value["name"]=serde_json::json!(req.name);
    value["email"]=serde_json::json!(req.email);
    value["username"]=serde_json::json!(req.username);
    conn.commit()?;
    Ok(value)
}

fn save_account(conn:&Connection,req:AkunKeuanganRequest,email:&str,create:bool)->Result<Value> {
    let input=crate::lifecycle::LifecycleRequest::from_value(&req.lifecycle_entity)?;
    let lc=converted_lifecycle(conn,"AKUNKEUANGAN",&input,req.actor_ident,create)?;
    let account_number=if create {Some(crate::util::create_account_number(req.account_type).map_err(|e|anyhow!(e))?)} else {
        conn.query_row("SELECT ACCOUNT_NUMBER FROM AKUNKEUANGAN WHERE ID=:1",&[&input.ident_entity])?.get::<usize,Option<String>>(0)?
    };
    if create && req.name.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(super::ValidationError("'Name' is mandatory.".into()).into());
    }
    let actor=match req.actor_ident {
        Some(id)=>Some(id),
        None if !email.is_empty()=>conn.query("SELECT ID FROM USERS WHERE EMAIL=:1 AND IDENT_MASTER IS NULL",&[&email])?
            .next().transpose()?.map(|r|r.get::<usize,i64>(0)).transpose()?,
        None=>None,
    };
    let fields=vec![("NAME",req.name.clone()),("DESCRIPTION",req.description.clone()),("ID_USER",actor.map(|v|v.to_string())),
        ("SALDO",Some("0".into())),("ACCOUNT_NUMBER",account_number),("CREATEDDATE",None),("UPDATEDDATE",None)];
    let id=super::oracle_lifecycle::persist(conn,"AKUNKEUANGAN","SEQ_ACCOUNT_COMPOSITE",input.ident_entity,lc,fields,create,
        |id|load_lifecycle(conn,"AKUNKEUANGAN",id),||next_seq(conn,"SEQ_ACCOUNT_COMPOSITE"))?;
    let mut value=write_entity_response(conn,"AKUNKEUANGAN",id,true)?;
    value["name"]=serde_json::json!(req.name);
    value["description"]=serde_json::json!(req.description);
    conn.commit()?;
    Ok(value)
}

fn relation_id(value:&Option<Value>)->Option<i64> {
    value.as_ref().and_then(|v|v.get("id").or_else(||v.get("entityIdent")))
        .and_then(|v|v.as_i64().or_else(||v.as_str().and_then(|v|v.parse().ok()))).map(|v|v.max(0))
}

fn request_account_json(value:&Option<Value>)->Option<Value> {
    value.as_ref().map(|input| {
        let mut output=json_shape::akun_stub(relation_id(value).unwrap_or(0));
        for key in ["name","description","idUser","saldo","accountNumber","createdDate","updatedDate",
            "identMaster","identRevision","revisionEffectiveFrom","revisionEffectiveTo","requestedActivationDate",
            "activationType","revisionState","identActivationType","lastUpdateUserIdent","lastUpdateTime"] {
            if let Some(v)=input.get(key) {output[key]=v.clone();}
        }
        output
    })
}

fn request_category_json(value:&Option<Value>)->Result<Option<Value>> {
    value.as_ref().map(|v| {
        let cat:CategoryRequest=serde_json::from_value(v.clone())?;
        Ok(json_shape::category_json(cat.id,cat.category_name.as_deref(),
            cat.category_type.as_ref().map(|v|v.to_string()).as_deref(),request_account_json(&cat.akun_keuangan),
            cat.created_date.map(|v|v.format("%Y-%m-%d %H:%M:%S%.9f").to_string()).as_deref(),
            cat.updated_date.map(|v|v.format("%Y-%m-%d %H:%M:%S%.9f").to_string()).as_deref()))
    }).transpose()
}

fn timestamp_to_sql(value:Option<chrono::DateTime<chrono::Utc>>)->Option<String> {
    value.map(|v| {
        let offset=std::env::var("APP_UTC_OFFSET_MINUTES").ok().and_then(|v|v.parse::<i64>().ok()).unwrap_or(0);
        (v.naive_utc()+chrono::Duration::minutes(offset)).format("%Y-%m-%d %H:%M:%S%.9f").to_string()
    })
}

fn save_category(conn:&Connection,cat:CategoryRequest,create:bool)->Result<Value> {
    if create && cat.id>0 {return Err(anyhow!("detached entity passed to persist"));}
    let id=if create {next_seq(conn,"CATEGORY_SEQUENCE")?} else {cat.id};
    let previous_account=if create {None} else {
        conn.query_row("SELECT ID_AKUNTRANSAKSI FROM CATEGORY WHERE ID=:1",&[&id])?.get::<usize,Option<i64>>(0)?
    };
    let (created,updated)=if create {(Some(crate::util::app_now_string()),timestamp_to_sql(cat.updated_date))} else {
        let row=conn.query_row("SELECT TO_CHAR(CREATEDDATE,'YYYY-MM-DD HH24:MI:SS.FF9'),TO_CHAR(UPDATEDDATE,'YYYY-MM-DD HH24:MI:SS.FF9') FROM CATEGORY WHERE ID=:1",&[&id])?;
        (row.get(0)?,row.get(1)?)
    };
    let fields=vec![("CATEGORY_NAME",cat.category_name.clone()),("CATEGORY_TYPE",cat.category_type.as_ref().map(|v|v.to_string())),
        ("ID_AKUNTRANSAKSI",relation_id(&cat.akun_keuangan).map(|v|v.to_string())),("CREATEDDATE",created),("UPDATEDDATE",updated)];
    super::oracle_lifecycle::write_fields(conn,"CATEGORY",id,&fields,create)?;
    let row=conn.query_row(&format!("SELECT {} FROM CATEGORY WHERE ID=:1",CATEGORY_COLS),&[&id])?;
    let account=if create || previous_account!=relation_id(&cat.akun_keuangan) {request_account_json(&cat.akun_keuangan)} else {
        relation_id(&cat.akun_keuangan).map(|id|load_akun_value(conn,id)).transpose()?.flatten()
    };
    let mut response=category_from_row(&row,account)?;
    response["category_type"]=json_shape::fmt_decimal(cat.category_type.as_ref().map(|v|v.to_string()).as_deref());
    conn.commit()?;
    Ok(response)
}

fn save_transaksi(conn:&Connection,tk:TransaksiKeuanganRequest,create:bool)->Result<Value> {
    if create && tk.id>0 {return Err(anyhow!("detached entity passed to persist"));}
    let id=if create {next_seq(conn,"TRANSAKSIKEUANGAN_SEQUENCE")?} else {tk.id};
    let previous_category=if create {None} else {
        conn.query_row("SELECT CATEGORY_ID FROM TRANSAKSIKEUANGAN WHERE ID=:1",&[&id])?.get::<usize,Option<i64>>(0)?
    };
    let (created,updated,external,internal)=if create {
        (Some(crate::util::app_now_string()),timestamp_to_sql(tk.updated_date),tk.externalreference.clone(),
            Some(crate::util::generate_internal_reference(crate::util::app_now())))
    } else {
        let row=conn.query_row("SELECT TO_CHAR(CREATEDDATE,'YYYY-MM-DD HH24:MI:SS'),TO_CHAR(UPDATEDDATE,'YYYY-MM-DD HH24:MI:SS'),EXTERNALREFERENCE,INTERNALREFERENCE FROM TRANSAKSIKEUANGAN WHERE ID=:1",&[&id])?;
        (row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)
    };
    let fields=vec![("NAME",tk.name.clone()),("DESCRIPTION",tk.description.clone()),("ID_AKUNTRANSAKSI",tk.id_akun_keuangan.map(|v|v.to_string())),
        ("JENIS",tk.jenis.as_ref().map(|v|v.to_string())),("NOMINAL",tk.nominal.as_ref().map(|v|v.to_string())),
        ("CATEGORY_ID",relation_id(&tk.category).map(|v|v.to_string())),("CREATEDDATE",created),("UPDATEDDATE",updated),
        ("TRANSAKSI_DATE",timestamp_to_sql(tk.transaction_date)),("EXTERNALREFERENCE",external),("INTERNALREFERENCE",internal)];
    super::oracle_lifecycle::write_fields(conn,"TRANSAKSIKEUANGAN",id,&fields,create)?;
    let row=conn.query_row(&format!("SELECT {} FROM TRANSAKSIKEUANGAN WHERE ID=:1",TK_COLS),&[&id])?;
    let category=if create || previous_category!=relation_id(&tk.category) {request_category_json(&tk.category)?} else {transaksi_category(conn,id)?};
    let mut response=transaksi_from_row(&row,category)?;
    response["name"]=serde_json::json!(tk.name);
    response["description"]=serde_json::json!(tk.description);
    if create {response["externalreference"]=serde_json::json!(tk.externalreference);}
    response["jenis"]=json_shape::fmt_decimal(tk.jenis.as_ref().map(|v|v.to_string()).as_deref());
    response["nominal"]=json_shape::fmt_decimal(tk.nominal.as_ref().map(|v|v.to_string()).as_deref());
    conn.commit()?;
    Ok(response)
}

impl Repository for OracleRepo {
    fn resolve_actor_ident<'a>(&'a self,email:&'a str)->BoxFut<'a,Option<i64>> {
        let email=email.to_string();
        Box::pin(async move {
            let conn=self.conn()?;
            blocking(move || {
                conn.query("SELECT ID FROM USERS WHERE EMAIL=:1 AND IDENT_MASTER IS NULL",&[&email])?
                    .next().transpose()?.map(|row|row.get::<usize,i64>(0).map_err(anyhow::Error::from)).transpose()
            }).await
        })
    }
    fn get_system_prop_bool<'a>(&'a self, name: &'a str) -> BoxFut<'a, bool> {
        let name = name.to_string();
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                match conn
                    .query("SELECT PROPVALUE FROM SYSTEMPROP WHERE PROPNAME = :1", &[&name])?
                    .next()
                {
                    Some(Ok(r)) => {
                        let v: Option<String> = r.get(0)?;
                        Ok(v.map(|x| x.eq_ignore_ascii_case("true")).unwrap_or(false))
                    }
                    _ => Ok(false),
                }
            })
            .await
        })
    }

    fn authenticate_user_by_email<'a>(&'a self, email: &'a str, password: &'a str) -> BoxFut<'a, bool> {
        let (email, password) = (email.to_string(), password.to_string());
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                // user.findUserByEmail: identMaster is null (hanya master)
                match conn
                    .query(
                        "SELECT PASSWORD FROM USERS WHERE EMAIL = :1 AND IDENT_MASTER IS NULL",
                        &[&email],
                    )?
                    .next()
                {
                    Some(Ok(r)) => {
                        let stored: Option<String> = r.get(0)?;
                        Ok(stored
                            .map(|h| crate::auth::authenticate_user_sha1(&password, &h))
                            .unwrap_or(false))
                    }
                    _ => Ok(false),
                }
            })
            .await
        })
    }

    fn authenticate_user_v2<'a>(&'a self, email: &'a str, password: &'a str) -> BoxFut<'a, bool> {
        let (email, password) = (email.to_string(), password.to_string());
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                let count:i64=conn.query_row("SELECT COUNT(*) FROM USERS WHERE EMAIL=:1 AND IDENT_MASTER IS NULL",&[&email])?.get(0)?;
                if count>1 {return Err(anyhow!("useraccount.toomanyaccounts.found"));}
                match conn
                    .query(
                        "SELECT PWD, SALT FROM USERS WHERE EMAIL = :1 AND IDENT_MASTER IS NULL",
                        &[&email],
                    )?
                    .next()
                {
                    Some(Ok(r)) => {
                        let pwd: Option<String> = r.get(0)?;
                        let salt: Option<String> = r.get(1)?;
                        match (pwd, salt) {
                            (Some(h), Some(salt)) => {
                                Ok(crate::auth::hash_password_sha256(&salt, &password) == h)
                            }
                            _ => Ok(false),
                        }
                    }
                    _ => Ok(false),
                }
            })
            .await
        })
    }

    fn fetch_all_users(&self) -> BoxFut<'_, Vec<Value>> {
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                // user.findAllUser
                let sql = format!(
                    "SELECT {}, {} FROM USERS WHERE IDENT_MASTER IS NULL",
                    USER_COLS, LC_COLS
                );
                let mut out = Vec::new();
                for row in conn.query(&sql, &[])? {
                    out.push(user_from_row(&row?)?);
                }
                Ok(out)
            })
            .await
        })
    }

    fn fetch_user_by_id<'a>(&'a self, id: i64) -> BoxFut<'a, Option<Value>> {
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                // user.findUserById — tanpa filter identMaster
                let sql = format!("SELECT {}, {} FROM USERS WHERE ID = :1", USER_COLS, LC_COLS);
                match conn.query(&sql, &[&id])?.next() {
                    Some(Ok(r)) => Ok(Some(user_from_row(&r)?)),
                    _ => Ok(None),
                }
            })
            .await
        })
    }

    fn create_user<'a>(&'a self, req: UsersRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || save_user(&conn, req, true)).await
        })
    }

    fn update_user<'a>(&'a self, req: UsersRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || save_user(&conn, req, false)).await
        })
    }

    fn change_password<'a>(&'a self, ident_user: i64, new_password: &'a str) -> BoxFut<'a, Value> {
        let new_password = new_password.to_string();
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                let lc=load_lifecycle(&conn,"USERS",ident_user)?;
                let mut fields=user_fields(&conn,ident_user)?;
                let creates_revision=lc.ident_master.is_none()
                    && !(lc.activation_type.as_ref().map(|v|v.0)==Some(5)
                        && super::oracle_lifecycle::has_future(&conn,"USERS",ident_user)?);
                let salt=crate::auth::new_password_salt();
                // UserPersister.create is invoked for a new revision, and hashes
                // the already SHA-1 encoded PASSWORD a second time in Java.
                let encoded=crate::auth::hash_password_sha1(&new_password);
                let credential=if creates_revision {encoded.as_str()} else {new_password.as_str()};
                for (key,value) in fields.iter_mut() {
                    match *key {
                        "PWD" => *value=Some(crate::auth::hash_password_sha256(&salt,credential)),
                        "SALT" => *value=Some(salt.clone()),
                        "PASSWORD" => *value=Some(crate::auth::hash_password_sha1(credential)),
                        "CREATEDDATE" | "UPDATEDDATE" if creates_revision => *value=None,
                        _=>{},
                    }
                }
                let id=super::oracle_lifecycle::persist(&conn,"USERS","USERS_SEQUENCE",ident_user,lc,fields,false,
                    |id|load_lifecycle(&conn,"USERS",id),||next_seq(&conn,"USERS_SEQUENCE"))?;
                let value=write_entity_response(&conn,"USERS",id,creates_revision)?;
                conn.commit()?;
                Ok(value)
            }).await
        })
    }

    fn delete_user<'a>(&'a self, id: i64) -> BoxFut<'a, bool> {
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                // deleteLifecycleEntity: hanya revision Ready tanpa effectiveFrom/To
                let lc = load_lifecycle(&conn, "USERS", id)?;
                let is_future_ready = lc.revision_state.as_ref().map(|(i, _)| *i) == Some(1)
                    && lc.revision_effective_from.is_none()
                    && lc.revision_effective_to.is_none();
                if !is_future_ready {
                    // Java melempar RuntimeException → HTTP 500 body kosong
                    return Err(anyhow!("lifecycle.not.allowed.to.delete.revision"));
                }
                conn.execute(
                    "UPDATE USERS SET IDENT_REVISIONSTATE = 6 WHERE ID = :1",
                    &[&id],
                )?;
                conn.commit()?;
                Ok(true)
            })
            .await
        })
    }

    fn fetch_akun_keuangan_by_email<'a>(&'a self, email: &'a str) -> BoxFut<'a, Vec<Value>> {
        let email = email.to_string();
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                // akunKeuangan.findAllByIdEmail
                let sql = format!(
                    "SELECT {}, {} FROM AKUNKEUANGAN ak WHERE ak.ID_USER IN \
                     (SELECT u.ID FROM USERS u WHERE u.EMAIL = :1) AND ak.IDENT_MASTER IS NULL",
                    AKUN_COLS, LC_COLS
                );
                let mut out = Vec::new();
                for row in conn.query(&sql, &[&email])? {
                    out.push(akun_from_row(&row?)?);
                }
                Ok(out)
            })
            .await
        })
    }

    fn fetch_akun_by_id<'a>(&'a self, id: i64) -> BoxFut<'a, Option<Value>> {
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                let sql = format!("SELECT {}, {} FROM AKUNKEUANGAN WHERE ID = :1", AKUN_COLS, LC_COLS);
                match conn.query(&sql, &[&id])?.next() {
                    Some(Ok(r)) => Ok(Some(akun_from_row(&r)?)),
                    _ => Ok(None),
                }
            })
            .await
        })
    }

    fn create_akun_keuangan<'a>(&'a self, req: AkunKeuanganRequest, email: &'a str) -> BoxFut<'a, Value> {
        let email=email.to_string();
        Box::pin(async move {
            let conn=self.conn()?;
            blocking(move || save_account(&conn,req,&email,true)).await
        })
    }

    fn update_akun_keuangan<'a>(&'a self, req: AkunKeuanganRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let conn=self.conn()?;
            blocking(move || save_account(&conn,req,"",false)).await
        })
    }

    fn fetch_categories<'a>(&'a self, id_akun: i64, email: &'a str) -> BoxFut<'a, Vec<Value>> {
        let email = email.to_string();
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                // category.findByAkunKeuangan — order by createdDate desc
                let sql = format!(
                    "SELECT {} FROM CATEGORY c WHERE c.ID_AKUNTRANSAKSI = :1 \
                     AND EXISTS (SELECT 1 FROM AKUNKEUANGAN ak, USERS u \
                                 WHERE u.ID = ak.ID_USER AND ak.ID = :2 AND u.EMAIL = :3) \
                     ORDER BY c.CREATEDDATE DESC",
                    CATEGORY_COLS
                );
                let akun = load_akun_value(&conn, id_akun)?;
                let mut out = Vec::new();
                for row in conn.query(&sql, &[&id_akun, &id_akun, &email])? {
                    out.push(category_from_row(&row?, akun.clone())?);
                }
                Ok(out)
            })
            .await
        })
    }

    fn fetch_category_detail<'a>(
        &'a self,
        id_akun: i64,
        id_c: i64,
        email: &'a str,
    ) -> BoxFut<'a, Option<Value>> {
        let email = email.to_string();
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                let sql = format!(
                    "SELECT {} FROM CATEGORY c WHERE c.ID = :1 AND c.ID_AKUNTRANSAKSI = :2 \
                     AND EXISTS (SELECT 1 FROM AKUNKEUANGAN ak, USERS u \
                                 WHERE u.ID = ak.ID_USER AND ak.ID = :3 AND u.EMAIL = :4)",
                    CATEGORY_COLS
                );
                let akun = load_akun_value(&conn, id_akun)?;
                match conn.query(&sql, &[&id_c, &id_akun, &id_akun, &email])?.next() {
                    Some(Ok(r)) => Ok(Some(category_from_row(&r, akun)?)),
                    _ => Ok(None),
                }
            })
            .await
        })
    }

    fn create_category<'a>(&'a self, req: CategoryRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let conn=self.conn()?;
            blocking(move || save_category(&conn,req,true)).await
        })
    }

    fn update_category<'a>(&'a self, req: CategoryRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let conn=self.conn()?;
            blocking(move || save_category(&conn,req,false)).await
        })
    }

    fn fetch_transaksi_by_akun<'a>(&'a self, id_akun: i64, email: &'a str) -> BoxFut<'a, Vec<Value>> {
        let email = email.to_string();
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                let sql = format!(
                    "SELECT {} FROM TRANSAKSIKEUANGAN tk WHERE tk.ID_AKUNTRANSAKSI = :1 \
                     AND EXISTS (SELECT 1 FROM AKUNKEUANGAN ak, USERS u \
                                 WHERE u.ID = ak.ID_USER AND ak.IDENT_MASTER IS NULL \
                                 AND ak.ID = :2 AND u.EMAIL = :3) \
                     ORDER BY tk.TRANSAKSI_DATE DESC",
                    TK_COLS
                );
                let mut out = Vec::new();
                for row in conn.query(&sql, &[&id_akun, &id_akun, &email])? {
                    let r = row?;
                    let id: i64 = r.get(0)?;
                    let cat = transaksi_category(&conn, id)?;
                    out.push(transaksi_from_row(&r, cat)?);
                }
                Ok(out)
            })
            .await
        })
    }

    fn fetch_transaksi_detail<'a>(
        &'a self,
        id_akun: i64,
        id_tk: i64,
        email: &'a str,
    ) -> BoxFut<'a, Option<Value>> {
        let email = email.to_string();
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                let sql = format!(
                    "SELECT {} FROM TRANSAKSIKEUANGAN tk WHERE tk.ID = :1 \
                     AND tk.ID_AKUNTRANSAKSI = :2 \
                     AND EXISTS (SELECT 1 FROM AKUNKEUANGAN ak, USERS u \
                                 WHERE u.ID = ak.ID_USER AND ak.IDENT_MASTER IS NULL \
                                 AND ak.ID = :3 AND u.EMAIL = :4)",
                    TK_COLS
                );
                match conn.query(&sql, &[&id_tk, &id_akun, &id_akun, &email])?.next() {
                    Some(Ok(r)) => {
                        let cat = transaksi_category(&conn, id_tk)?;
                        Ok(Some(transaksi_from_row(&r, cat)?))
                    }
                    _ => Ok(None),
                }
            })
            .await
        })
    }

    fn create_transaksi<'a>(&'a self, req: TransaksiKeuanganRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let conn=self.conn()?;
            blocking(move || save_transaksi(&conn,req,true)).await
        })
    }

    fn update_transaksi<'a>(&'a self, req: TransaksiKeuanganRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let conn=self.conn()?;
            blocking(move || save_transaksi(&conn,req,false)).await
        })
    }

    fn fetch_transaksi_not_in_settlement_engine(&self) -> BoxFut<'_, Vec<Value>> {
        Box::pin(async move {
            let conn = self.conn()?;
            blocking(move || {
                // Native SQL lama, dipertahankan apa adanya
                let mut out = Vec::new();
                for row in conn.query(
                    "SELECT ID FROM TRANSAKSIKEUANGAN WHERE INTERNALREFERENCE IS NOT NULL \
                     AND INTERNALREFERENCE NOT IN \
                     (SELECT INSTRUCTIONREFERENCE FROM CSV_SETTLEMENTINSTRUCTION_AKUNKEUANGAN)",
                    &[],
                )? {
                    let r = row?;
                    let id: i64 = r.get(0)?;
                    out.push(serde_json::json!({ "id": id }));
                }
                Ok(out)
            })
            .await
        })
    }

    fn fetch_event_entries<'a>(&'a self, event_id: &'a str) -> BoxFut<'a, Vec<Value>> {
        let event_id = event_id.to_string();
        Box::pin(async move {
            let conn = self.audit_conn()?;
            blocking(move || {
                let mut out = Vec::new();
                let sql = "SELECT IDENT_AUDITEVENTENTRY, ENTRYNAME, \
                           TO_CHAR(ENTRYTIMESTAMP, 'YYYY-MM-DD HH24:MI:SS'), \
                           NANOSECONDSONTIMESTAMP, BUSINESSIDENTIFIER, ENTRYDATA, \
                           COMPRESSEDENTRYDATA, ENTITYNAME, ENTITYID, MASTERID, REVISIONID \
                           FROM AUDITEVENTENTRY WHERE EVENTID = :1 ORDER BY IDENT_AUDITEVENTENTRY";
                for row in conn.query(sql, &[&event_id])? {
                    let r = row?;
                    let entry_id: i64 = r.get(0)?;
                    let name: Option<String> = r.get(1)?;
                    let ts_v: Option<String> = r.get(2)?;
                    let nanos: Option<i32> = r.get(3)?;
                    let business_id: Option<String> = r.get(4)?;
                    let data: Option<String> = r.get(5)?;
                    let compressed: Option<Vec<u8>> = r.get(6)?;
                    let entity_name: Option<String> = r.get(7)?;
                    let entity_id: Option<i64> = r.get(8)?;
                    let master_id: Option<i64> = r.get(9)?;
                    let revision_id: Option<i64> = r.get(10)?;
                    out.push(crate::db::audit_gson::entry_json(
                        entry_id,
                        &event_id,
                        name,
                        ts_v,
                        nanos,
                        business_id,
                        data,
                        compressed,
                        entity_name,
                        entity_id,
                        master_id,
                        revision_id,
                    )?);
                }
                Ok(out)
            })
            .await
        })
    }
}

// ---------------------------------------------------------------------------
// Helper insert/load
// ---------------------------------------------------------------------------

fn load_lifecycle(conn: &Connection, table: &str, id: i64) -> Result<Lifecycle> {
    let sql = format!("SELECT {} FROM {} WHERE ID = :1", LC_COLS, table);
    let row = conn
        .query(&sql, &[&id])?
        .next()
        .ok_or_else(|| anyhow!("not found"))??;
    read_lifecycle(&row, 0)
}
