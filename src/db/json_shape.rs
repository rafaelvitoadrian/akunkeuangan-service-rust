//! Bentuk JSON respons — SATU-SATUNYA sumber kebenaran untuk format output.
//!
//! Semua urutan field di bawah ini **direkam dari sistem lama yang berjalan**
//! (Java, legacy reference server) pada 2026-10-17, bukan diasumsikan.
//! Fixture: golden/json/*.json
//!
//! Aturan yang ditemukan:
//! - `null` selalu dikirim eksplisit (Jackson default), tidak dihilangkan.
//! - BigDecimal → angka JSON; skala mengikuti nilai dari DB (150000.50 → 150000.5).
//! - Timestamp dengan @JsonFormat → "yyyy-MM-dd'T'HH:mm:ss" (UTC, tanpa zona).
//! - LocalDateTime tanpa @JsonFormat → ISO lokal dengan microsecond bila ada
//!   ("2026-10-15T01:38:00.822745").
//! - Timestamp TANPA @JsonFormat (AkunKeuanganPO.createdDate/updatedDate) → epoch millis.
//! - Setiap entity diakhiri `entityIdent` (dari getEntityIdent()).
//! - `id` muncul SETELAH field milik entity, sebelum getter boolean turunan.

use serde_json::{json, Map, Value};

/// Nilai lifecycle bersama (LifecycleEntityPO).
#[derive(Clone, Default)]
pub struct Lifecycle {
    pub ident_master: Option<i64>,
    pub ident_revision: Option<i64>,
    pub revision_effective_from: Option<String>,
    pub revision_effective_to: Option<String>,
    pub requested_activation_date: Option<String>,
    pub activation_type: Option<(i64, String)>,
    pub ident_activation_type: i64,
    pub last_update_user_ident: Option<i64>,
    pub last_update_time: Option<String>,
    pub revision_state: Option<(i64, String)>,
}

impl Lifecycle {
    /// Lifecycle default untuk entity baru: Active + Ready (seperti data hasil create).
    pub fn active_ready(id: i64) -> Self {
        Self {
            activation_type: Some((1, "Active".into())),
            ident_activation_type: 1,
            revision_state: Some((1, "Ready".into())),
            last_update_user_ident: Some(id),
            ..Default::default()
        }
    }
}

fn activation_type_json(id: i64, name: &str) -> Value {
    // Urutan direkam: typeName, active, suspended, identActivationType, inActive, entityIdent, deactivated
    json!({
        "typeName": name,
        "active": id == 1,
        "suspended": id == 4,
        "identActivationType": id,
        "inActive": id == 5,
        "entityIdent": id,
        "deactivated": id == 2,
    })
}

fn revision_state_json(id: i64, name: &str) -> Value {
    // Urutan direkam: name, error, entityIdent, ready, identRevisionState, deleted,
    // pending, awaitingApproval, rejected, validationFailed
    json!({
        "name": name,
        "error": name == "Error",
        "entityIdent": id,
        "ready": name == "Ready",
        "identRevisionState": id,
        "deleted": name == "Deleted",
        "pending": name == "Pending",
        "awaitingApproval": name == "AwaitingApproval",
        "rejected": name == "Rejected",
        "validationFailed": name == "ValidationFailed",
    })
}

/// LocalDateTime tanpa @JsonFormat → ISO lokal, microsecond hanya bila ada.
/// Contoh rekaman: "2026-10-15T01:38:00.822745"
pub fn fmt_local_datetime(v: Option<&str>) -> Value {
    let Some(s) = v else { return Value::Null };
    // Sudah ISO (dari DB/seed) — normalisasi pemisah & presisi
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return json!(fmt_iso_local(dt));
        }
    }
    json!(s)
}

fn fmt_iso_local(dt: chrono::NaiveDateTime) -> String {
    use chrono::Timelike;
    let base = dt.format("%Y-%m-%dT%H:%M:%S").to_string();
    let nanos = dt.nanosecond();
    if nanos == 0 {
        return base;
    }
    // Jackson JavaTimeModule → DateTimeFormatter.ISO_LOCAL_DATE_TIME:
    // fraksi ditulis dengan digit minimum, SELURUH trailing zero dibuang.
    // Dibuktikan: DB 22:06:03.131340 → API "2026-04-11T22:06:03.13134"
    let frac = format!("{:09}", nanos);
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        base
    } else {
        format!("{}.{}", base, frac)
    }
}

/// java.sql.Timestamp + @JsonFormat("yyyy-MM-dd'T'HH:mm:ss") → UTC tanpa zona.
pub fn fmt_timestamp(v: Option<&str>) -> Value {
    let Some(s) = v else { return Value::Null };
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return json!(dt.format("%Y-%m-%dT%H:%M:%S").to_string());
        }
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return json!(dt.naive_utc().format("%Y-%m-%dT%H:%M:%S").to_string());
    }
    json!(s)
}

/// `java.sql.Timestamp` TANPA `@JsonFormat` → Jackson default (`StdDateFormat`).
///
/// Dibuktikan dari sistem berjalan: DB `AKUNKEUANGAN.ID=124`
///   CREATEDDATE = 2025-08-09 15:30:03 (wall-clock WIB)
///   API         = "2025-08-09T08:30:03.000+00:00"
/// Jadi formatnya `yyyy-MM-dd'T'HH:mm:ss.SSSXXX` dengan offset UTC, dan nilainya
/// sudah dikonversi ke UTC oleh query (lihat utc_ts! di oracle_impl).
///
/// Dipakai AkunKeuanganPO.createdDate / updatedDate (satu-satunya field Timestamp
/// tanpa @JsonFormat di sistem lama).
pub fn fmt_timestamp_default(v: Option<&str>) -> Value {
    let Some(s) = v else { return Value::Null };
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return json!(format!("{}.{:03}+00:00", dt.format("%Y-%m-%dT%H:%M:%S"), dt.and_utc().timestamp_subsec_millis()));
        }
    }
    Value::Null
}

/// BigDecimal → angka JSON apa adanya (skala mengikuti sumber).
pub fn fmt_decimal(v: Option<&str>) -> Value {
    match v {
        None => Value::Null,
        Some(s) => serde_json::from_str::<serde_json::Number>(s)
            .map(Value::Number)
            .unwrap_or(Value::Null),
    }
}

fn insert_lifecycle(m: &mut Map<String, Value>, lc: &Lifecycle) {
    m.insert("identMaster".into(), json!(lc.ident_master));
    m.insert("identRevision".into(), json!(lc.ident_revision));
    m.insert(
        "revisionEffectiveFrom".into(),
        fmt_local_datetime(lc.revision_effective_from.as_deref()),
    );
    m.insert(
        "revisionEffectiveTo".into(),
        fmt_local_datetime(lc.revision_effective_to.as_deref()),
    );
    m.insert("requestedActivationDate".into(), json!(lc.requested_activation_date));
    m.insert(
        "activationType".into(),
        match &lc.activation_type {
            Some((id, name)) => activation_type_json(*id, name),
            None => Value::Null,
        },
    );
    m.insert("identActivationType".into(), json!(lc.ident_activation_type));
    m.insert("lastUpdateUserIdent".into(), json!(lc.last_update_user_ident));
    m.insert("lastUpdateTime".into(), fmt_local_datetime(lc.last_update_time.as_deref()));
    m.insert(
        "revisionState".into(),
        match &lc.revision_state {
            Some((id, name)) => revision_state_json(*id, name),
            None => Value::Null,
        },
    );
}

// ---------------------------------------------------------------------------
// UserPO — extends LifecycleEntityPO
// ---------------------------------------------------------------------------

/// Urutan direkam dari golden/json/user_admin1.json:
/// [lifecycle...] name, email, username, pwdHash, password, salt, createdDate,
/// updatedDate, id, [booleans], entityIdent
#[allow(clippy::too_many_arguments)]
pub fn user_json(
    id: i64,
    lc: &Lifecycle,
    name: Option<&str>,
    email: Option<&str>,
    username: Option<&str>,
    pwd_hash: Option<&str>,
    password: Option<&str>,
    salt: Option<&str>,
    created_date: Option<&str>,
    updated_date: Option<&str>,
) -> Value {
    let mut m = Map::new();
    insert_lifecycle(&mut m, lc);
    m.insert("name".into(), json!(name));
    m.insert("email".into(), json!(email));
    m.insert("username".into(), json!(username));
    m.insert("pwdHash".into(), json!(pwd_hash));
    m.insert("password".into(), json!(password));
    m.insert("salt".into(), json!(salt));
    m.insert("createdDate".into(), fmt_timestamp(created_date));
    m.insert("updatedDate".into(), fmt_timestamp(updated_date));
    m.insert("id".into(), json!(id));
    // Getter boolean turunan. URUTANNYA TIDAK STABIL di Java: properti field
    // mengikuti deklarasi, tetapi getter mengikuti urutan Class.getDeclaredMethods()
    // yang berubah antar-restart JVM (dibuktikan: rekaman 2026-10-17 vs 2026-10-18
    // menghasilkan urutan berbeda untuk JVM yang sama). Urutan di bawah mengikuti
    // instance referensi terakhir; klien tidak boleh bergantung pada urutan ini.
    let act = lc.activation_type.as_ref().map(|(id, _)| *id);
    let rev = lc.revision_state.as_ref().map(|(_, n)| n.as_str());
    m.insert("active".into(), json!(act == Some(1)));
    m.insert("suspended".into(), json!(act == Some(4)));
    m.insert("revisionStateReady".into(), json!(rev == Some("Ready")));
    m.insert("revision".into(), json!(lc.ident_master.is_some()));
    m.insert("inActive".into(), json!(act == Some(5)));
    m.insert("master".into(), json!(lc.ident_master.is_none()));
    m.insert(
        "revisionStateWaitingForApproval".into(),
        json!(rev == Some("AwaitingApproval")),
    );
    m.insert("revisionStateDeleted".into(), json!(rev == Some("Deleted")));
    m.insert("revisionStateRejected".into(), json!(rev == Some("Rejected")));
    m.insert(
        "revisionStateValidationFailed".into(),
        json!(rev == Some("ValidationFailed")),
    );
    m.insert("revisionStateError".into(), json!(rev == Some("Error")));
    m.insert("revisionStatePending".into(), json!(rev == Some("Pending")));
    m.insert("inactive".into(), json!(act == Some(5)));
    m.insert("pending".into(), json!(act == Some(5)));
    m.insert("deactivated".into(), json!(act == Some(2)));
    m.insert("entityIdent".into(), json!(id));
    Value::Object(m)
}

// ---------------------------------------------------------------------------
// AkunKeuanganPO — extends LifecycleEntityPO, punya getIdent()
// ---------------------------------------------------------------------------

/// Urutan direkam dari golden/json/akun_detail.json:
/// ident, [lifecycle...], name, description, idUser, saldo, accountNumber,
/// createdDate, updatedDate, id, [booleans urutan KHUSUS akun], entityIdent
#[allow(clippy::too_many_arguments)]
pub fn akun_json(
    id: i64,
    lc: &Lifecycle,
    name: Option<&str>,
    description: Option<&str>,
    id_user: Option<i64>,
    saldo: Option<&str>,
    account_number: Option<&str>,
    created_date: Option<&str>,
    updated_date: Option<&str>,
) -> Value {
    let mut m = Map::new();
    m.insert("ident".into(), json!(id));
    insert_lifecycle(&mut m, lc);
    m.insert("name".into(), json!(name));
    m.insert("description".into(), json!(description));
    m.insert("idUser".into(), json!(id_user));
    m.insert("saldo".into(), fmt_decimal(saldo));
    m.insert("accountNumber".into(), json!(account_number));
    // Tanpa @JsonFormat → Jackson default (ISO + millis + offset UTC)
    m.insert("createdDate".into(), fmt_timestamp_default(created_date));
    m.insert("updatedDate".into(), fmt_timestamp_default(updated_date));
    m.insert("id".into(), json!(id));
    // Urutan getter boolean yang direkam untuk AkunKeuanganPO (berbeda dari UserPO!)
    // TEMUAN: isRevisionStateReady/Pending/Error DI-OVERRIDE di Java:
    //   isRevisionStateReady() -> true (selalu)
    //   isRevisionStatePending() -> true (selalu)
    //   isRevisionStateError() -> false (selalu)
    let act = lc.activation_type.as_ref().map(|(id, _)| *id);
    let rev = lc.revision_state.as_ref().map(|(_, n)| n.as_str());
    // Urutan getter (tidak stabil antar-restart JVM — lihat catatan di user_json)
    m.insert("revisionStateReady".into(), json!(true));
    m.insert("revisionStateError".into(), json!(false));
    m.insert("revisionStatePending".into(), json!(true));
    m.insert("active".into(), json!(act == Some(1)));
    m.insert("suspended".into(), json!(act == Some(4)));
    m.insert("revision".into(), json!(lc.ident_master.is_some()));
    m.insert("inActive".into(), json!(act == Some(5)));
    m.insert("master".into(), json!(lc.ident_master.is_none()));
    m.insert(
        "revisionStateWaitingForApproval".into(),
        json!(rev == Some("AwaitingApproval")),
    );
    m.insert("revisionStateDeleted".into(), json!(rev == Some("Deleted")));
    m.insert("revisionStateRejected".into(), json!(rev == Some("Rejected")));
    m.insert(
        "revisionStateValidationFailed".into(),
        json!(rev == Some("ValidationFailed")),
    );
    m.insert("inactive".into(), json!(act == Some(5)));
    m.insert("pending".into(), json!(act == Some(5)));
    m.insert("deactivated".into(), json!(act == Some(2)));
    m.insert("entityIdent".into(), json!(id));
    Value::Object(m)
}

// ---------------------------------------------------------------------------
// CategoryPO — extends EntityObject (tanpa lifecycle)
// ---------------------------------------------------------------------------

/// Urutan direkam dari golden/json/category_create.json:
/// category_name, category_type, akunKeuangan, createdDate, updatedDate, id, entityIdent
pub fn category_json(
    id: i64,
    category_name: Option<&str>,
    category_type: Option<&str>,
    akun: Option<Value>,
    created_date: Option<&str>,
    updated_date: Option<&str>,
) -> Value {
    let mut m = Map::new();
    m.insert("category_name".into(), json!(category_name));
    m.insert("category_type".into(), fmt_decimal(category_type));
    m.insert("akunKeuangan".into(), akun.unwrap_or(Value::Null));
    m.insert("createdDate".into(), fmt_timestamp(created_date));
    m.insert("updatedDate".into(), fmt_timestamp(updated_date));
    m.insert("id".into(), json!(id));
    m.insert("entityIdent".into(), json!(id));
    Value::Object(m)
}

// ---------------------------------------------------------------------------
// TransaksiKeuanganPO — extends EntityObject (tanpa lifecycle)
// ---------------------------------------------------------------------------

/// Urutan direkam dari golden/json/tk_create.json:
/// name, description, idAkunKeuangan, category, jenis, nominal, createdDate,
/// updatedDate, transactionDate, externalreference, internalreference, id, entityIdent
#[allow(clippy::too_many_arguments)]
pub fn transaksi_json(
    id: i64,
    name: Option<&str>,
    description: Option<&str>,
    id_akun_keuangan: Option<i64>,
    category: Option<Value>,
    jenis: Option<&str>,
    nominal: Option<&str>,
    created_date: Option<&str>,
    updated_date: Option<&str>,
    transaction_date: Option<&str>,
    external_reference: Option<&str>,
    internal_reference: Option<&str>,
) -> Value {
    let mut m = Map::new();
    m.insert("name".into(), json!(name));
    m.insert("description".into(), json!(description));
    m.insert("idAkunKeuangan".into(), json!(id_akun_keuangan));
    m.insert("category".into(), category.unwrap_or(Value::Null));
    m.insert("jenis".into(), fmt_decimal(jenis));
    m.insert("nominal".into(), fmt_decimal(nominal));
    m.insert("createdDate".into(), fmt_timestamp(created_date));
    m.insert("updatedDate".into(), fmt_timestamp(updated_date));
    m.insert("transactionDate".into(), fmt_timestamp(transaction_date));
    m.insert("externalreference".into(), json!(external_reference));
    m.insert("internalreference".into(), json!(internal_reference));
    m.insert("id".into(), json!(id));
    m.insert("entityIdent".into(), json!(id));
    Value::Object(m)
}

/// Akun "kosong" untuk nested LAZY reference (seperti terlihat di category_create:
/// hanya id yang terisi, sisanya null, tapi boolean akun tetap dihitung).
pub fn akun_stub(id: i64) -> Value {
    akun_json(id, &Lifecycle::default(), None, None, None, None, None, None, None)
}
