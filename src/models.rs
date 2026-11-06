//! Entity model — meniru field & urutan JSON dari PO Java (urutan deklarasi getter).
//!
//! Sumber: UserPO.java, AkunKeuanganPO.java, CategoryPO.java, TransaksiKeuanganPO.java,
//! LifecycleEntityPO.java, EntityObject.java
//!
//! Aturan serialisasi (dari Jackson pada sistem lama):
//! - Urutan field mengikuti urutan deklarasi getter (Jackson default) → serde_json preserve_order
//! - null → JSON null (bukan dihilangkan)
//! - UserPO/AkunKeuanganPO extends LifecycleEntityPO → field lifecycle SETELAH field sendiri
//! - CategoryPO/TransaksiKeuanganPO extends EntityObject → tanpa field lifecycle

use crate::util::jackson_serde as js;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use crate::util::jackson_serde::ExactDecimal as Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Field yang diwarisi dari LifecycleEntityPO (urutan sesuai deklarasi di Java).
/// Untuk response JSON, Jackson mengekspos getter-nya.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct LifecycleFields {
    #[serde(with = "js::localdatetime_space_format", default)]
    pub revision_effective_from: Option<NaiveDateTime>,
    #[serde(with = "js::localdatetime_space_format", default)]
    pub revision_effective_to: Option<NaiveDateTime>,
    #[serde(default)]
    pub requested_activation_date: Option<NaiveDate>,
    /// LAZY ManyToOne ActivationTypePO — pada response bisa berupa objek atau null
    #[serde(default)]
    pub activation_type: Option<Value>,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_long")]
    pub last_update_user_ident: Option<i64>,
    #[serde(with = "js::localdatetime_space_format", default)]
    pub last_update_time: Option<NaiveDateTime>,
    /// LAZY RevisionStatePO — objek atau null
    #[serde(default)]
    pub revision_state: Option<Value>,
}

/// UserPO response — urutan field sesuai Jackson (getter declaration order):
/// id, ident (dari EntityObject), name, email, username, pwdHash, password, salt,
/// createdDate, updatedDate, lalu field lifecycle.
/// CATATAN TEMUAN: pwdHash/password/salt bocor di sistem lama (tanpa @JsonIgnore) —
/// dipertahankan untuk kompatibilitas 100%.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct UserPo {
    #[serde(default)]
    #[serde(deserialize_with = "js::long_default")]
    pub id: i64,
    #[serde(default)]
    #[serde(deserialize_with = "js::long_default")]
    pub ident: i64,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub name: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub email: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub username: Option<String>,
    /// TEMUAN: bocor di sistem lama — dipertahankan
    #[serde(rename = "pwdHash", default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub pwd_hash: Option<String>,
    /// TEMUAN: bocor di sistem lama — dipertahankan (SHA-1 hex)
    #[serde(default, deserialize_with = "js::optional_string")]
    pub password: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub salt: Option<String>,
    #[serde(default, with = "js::timestamp_json_format", rename = "createdDate")]
    pub created_date: Option<DateTime<Utc>>,
    #[serde(default, with = "js::timestamp_json_format", rename = "updatedDate")]
    pub updated_date: Option<DateTime<Utc>>,
    #[serde(flatten)]
    pub lifecycle: LifecycleFields,
}

/// AkunKeuanganPO response.
/// TEMUAN: createdDate/updatedDate TANPA @JsonFormat → epoch-millis number di sistem lama.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct AkunKeuanganPo {
    #[serde(default)]
    #[serde(deserialize_with = "js::long_default")]
    pub id: i64,
    #[serde(default)]
    #[serde(deserialize_with = "js::long_default")]
    pub ident: i64,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub name: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub description: Option<String>,
    #[serde(rename = "idUser")]
    #[serde(default, deserialize_with = "js::optional_long")]
    pub id_user: Option<i64>,
    #[serde(default, with = "js::exact_decimal")]
    pub saldo: Option<Decimal>,
    #[serde(rename = "accountNumber")]
    #[serde(default, deserialize_with = "js::optional_string")]
    pub account_number: Option<String>,
    #[serde(default, with = "js::timestamp_epoch_millis", rename = "createdDate")]
    pub created_date: Option<DateTime<Utc>>,
    #[serde(default, with = "js::timestamp_epoch_millis", rename = "updatedDate")]
    pub updated_date: Option<DateTime<Utc>>,
    #[serde(flatten)]
    pub lifecycle: LifecycleFields,
}

/// CategoryPO response — category_name/category_type mempertahankan snake_case
/// (dari getter getCategory_name di Java).
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CategoryPo {
    #[serde(default)]
    #[serde(deserialize_with = "js::long_default")]
    pub id: i64,
    #[serde(default)]
    #[serde(deserialize_with = "js::long_default")]
    pub ident: i64,
    #[serde(rename = "category_name")]
    #[serde(default, deserialize_with = "js::optional_string")]
    pub category_name: Option<String>,
    #[serde(default, with = "js::exact_decimal", rename = "category_type")]
    pub category_type: Option<Decimal>,
    /// LAZY ManyToOne AkunKeuanganPO — nested atau null
    #[serde(rename = "akunKeuangan")]
    #[serde(default, deserialize_with = "js::account_relation")]
    pub akun_keuangan: Option<Value>,
    #[serde(default, with = "js::timestamp_json_format", rename = "createdDate")]
    pub created_date: Option<DateTime<Utc>>,
    #[serde(default, with = "js::timestamp_json_format", rename = "updatedDate")]
    pub updated_date: Option<DateTime<Utc>>,
}

/// TransaksiKeuanganPO response.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TransaksiKeuanganPo {
    #[serde(default)]
    #[serde(deserialize_with = "js::long_default")]
    pub id: i64,
    #[serde(default)]
    #[serde(deserialize_with = "js::long_default")]
    pub ident: i64,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub name: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub description: Option<String>,
    #[serde(rename = "idAkunKeuangan")]
    #[serde(default, deserialize_with = "js::optional_long")]
    pub id_akun_keuangan: Option<i64>,
    /// LAZY ManyToOne CategoryPO — nested atau null
    #[serde(default, deserialize_with = "js::category_relation")]
    pub category: Option<Value>,
    #[serde(default, with = "js::exact_decimal")]
    pub jenis: Option<Decimal>,
    #[serde(default, with = "js::exact_decimal")]
    pub nominal: Option<Decimal>,
    #[serde(default, with = "js::timestamp_json_format", rename = "createdDate")]
    pub created_date: Option<DateTime<Utc>>,
    #[serde(default, with = "js::timestamp_json_format", rename = "updatedDate")]
    pub updated_date: Option<DateTime<Utc>>,
    #[serde(default, with = "js::timestamp_json_format", rename = "transactionDate")]
    pub transaction_date: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "js::optional_string")]
    pub externalreference: Option<String>,
    #[serde(default, deserialize_with = "js::optional_string")]
    pub internalreference: Option<String>,
}

// ---------------------------------------------------------------------------
// Request DTOs (model/ package di Java)
// ---------------------------------------------------------------------------

/// model/users/Users.java — POST/PUT /api/v2/user.
/// Tanpa annotasi Jackson di Java → default naming.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct UsersRequest {
    #[serde(default)]
    #[serde(deserialize_with = "js::long_default")]
    pub ident: i64,
    #[serde(rename = "needsConfirmation", default)]
    #[serde(deserialize_with = "js::bool_default")]
    pub needs_confirmation: bool,
    #[serde(rename = "pendingLedgerConfirmation", default)]
    #[serde(deserialize_with = "js::bool_default")]
    pub pending_ledger_confirmation: bool,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub name: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub email: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub username: Option<String>,
    #[serde(rename = "pwdHash", default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub pwd_hash: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub password: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub salt: Option<String>,
    #[serde(default, rename = "permissionProfiles", deserialize_with = "crate::security::deserialize_profiles")]
    pub permission_profiles: Option<Vec<crate::security::ProfileRequest>>,
    #[serde(default = "crate::lifecycle::default_request", rename = "lifecycleEntity", deserialize_with = "crate::lifecycle::deserialize_request")]
    pub lifecycle_entity: Option<Value>,
    #[serde(skip)]
    #[serde(default, deserialize_with = "js::optional_long")]
    pub actor_ident: Option<i64>,
}

/// model/users/ChangePassword.java
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ChangePasswordRequest {
    #[serde(rename = "identUser")]
    #[serde(default, deserialize_with = "js::optional_long")]
    pub ident_user: Option<i64>,
    #[serde(rename = "newPassword")]
    #[serde(default, deserialize_with = "js::optional_string")]
    pub new_password: Option<String>,
}

/// model/akunkeuangan/AkunKeuangan.java — POST/PUT /api/v1/akunkeuangan
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct AkunKeuanganRequest {
    #[serde(default)]
    #[serde(deserialize_with = "js::long_default")]
    pub ident: i64,
    #[serde(rename = "needsConfirmation", default)]
    #[serde(deserialize_with = "js::bool_default")]
    pub needs_confirmation: bool,
    #[serde(rename = "pendingLedgerConfirmation", default)]
    #[serde(deserialize_with = "js::bool_default")]
    pub pending_ledger_confirmation: bool,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub name: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub description: Option<String>,
    #[serde(rename = "idUser", default)]
    #[serde(deserialize_with = "js::optional_long")]
    pub id_user: Option<i64>,
    #[serde(with = "js::exact_decimal", default)]
    pub saldo: Option<Decimal>,
    #[serde(rename = "accountNumber", default)]
    #[serde(deserialize_with = "js::optional_string")]
    pub account_number: Option<String>,
    #[serde(rename = "accountType", default)]
    #[serde(deserialize_with = "js::optional_long")]
    pub account_type: Option<i64>,
    #[serde(default = "crate::lifecycle::default_request", rename = "lifecycleEntity", deserialize_with = "crate::lifecycle::deserialize_request")]
    pub lifecycle_entity: Option<Value>,
    #[serde(skip)]
    #[serde(default, deserialize_with = "js::optional_long")]
    pub actor_ident: Option<i64>,
}

/// Request TransaksiKeuanganController menerima TransaksiKeuanganPO langsung
/// (bukan DTO terpisah) — jadi request dan response bentuknya sama.
pub type TransaksiKeuanganRequest = TransaksiKeuanganPo;

/// Request CategoryController menerima CategoryPO langsung.
pub type CategoryRequest = CategoryPo;
