//! Repository trait with memory, Oracle and PostgreSQL implementations.
//!
//! PENTING: Rust tidak punya driver Oracle pure-Rust. Implementasi Oracle memakai
//! crate `oracle` (ODPI-C) yang butuh Oracle Instant Client — diaktifkan lewat
//! feature "oracle" saat client tersedia. Default: PostgreSQL; memory is used
//! explicitly for golden contract tests. PostgreSQL uses the native
//! Rust SQLx driver and runs its own embedded migrations at startup.

pub mod audit_gson;
pub mod json_shape;
pub mod memory;
#[cfg(feature = "postgres")]
pub mod postgres_impl;
#[cfg(feature = "postgres")]
mod postgres_lifecycle;
#[cfg(feature = "postgres")]
mod postgres_security;
#[cfg(feature = "postgres")]
mod postgres_security_graph;
#[cfg(feature = "oracle")]
pub mod oracle_impl;
#[cfg(feature = "oracle")]
mod oracle_lifecycle;

use crate::models::*;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ValidationError(pub String);

/// Repository — kontrak akses data yang dipakai routes.
/// Implementasi harus mempertahankan perilaku persister Java.
///
/// Menggunakan Box<dyn Future> agar dyn-compatible (dipakai sebagai Arc<dyn Repository>).
pub trait Repository: Send + Sync {
    fn fetch_all_user_models(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output=anyhow::Result<Vec<Value>>>+Send+'_>> {
        Box::pin(async move {
            let mut result = Vec::new();
            for raw in self.fetch_all_users().await? {
                result.push(self.user_model(raw).await?);
            }
            Ok(result)
        })
    }
    fn create_user_model<'a>(&'a self,req:UsersRequest)->std::pin::Pin<Box<dyn std::future::Future<Output=anyhow::Result<Value>>+Send+'a>> {
        Box::pin(async move { self.user_model(self.create_user(req).await?).await })
    }
    fn update_user_model<'a>(&'a self,req:UsersRequest)->std::pin::Pin<Box<dyn std::future::Future<Output=anyhow::Result<Value>>+Send+'a>> {
        Box::pin(async move { self.user_model(self.update_user(req).await?).await })
    }
    fn user_model<'a>(&'a self, raw: Value) -> std::pin::Pin<Box<dyn std::future::Future<Output=anyhow::Result<Value>>+Send+'a>> {
        Box::pin(async move { Ok(crate::security::user_model(&raw, serde_json::json!([]), None)) })
    }
    /// Legacy/memory repositories keep the recorded contract fixture behavior.
    /// PostgreSQL implements the current FunctionsAllowed service checks.
    fn require_function<'a>(&'a self, _email: &'a str, _function: &'a str, _minimum: i64) -> std::pin::Pin<Box<dyn std::future::Future<Output=anyhow::Result<()>>+Send+'a>> {
        Box::pin(async { Ok(()) })
    }
    fn security_query<'a>(&'a self, _query: crate::security::SecurityQuery, _email: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output=anyhow::Result<Value>>+Send+'a>> {
        Box::pin(async { anyhow::bail!("security management requires the PostgreSQL backend") })
    }
    fn security_write<'a>(&'a self, _write: crate::security::SecurityWrite, _email: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output=anyhow::Result<Value>>+Send+'a>> {
        Box::pin(async { anyhow::bail!("security management requires the PostgreSQL backend") })
    }
    fn resolve_actor_ident<'a>(&'a self,email:&'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output=anyhow::Result<Option<i64>>>+Send+'a>> {
        Box::pin(async move {
            Ok(self.fetch_all_users().await?.iter().find(|u|u["email"].as_str()==Some(email)).and_then(|u|u["id"].as_i64()))
        })
    }
    // System props
    fn get_system_prop_bool<'a>(
        &'a self,
        name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<bool>> + Send + 'a>>;
    // Auth
    fn authenticate_user_by_email<'a>(
        &'a self,
        email: &'a str,
        password: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<bool>> + Send + 'a>>;
    fn authenticate_user_v2<'a>(
        &'a self,
        email: &'a str,
        password: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<bool>> + Send + 'a>>;
    // Users
    fn fetch_all_users(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Vec<Value>>> + Send + '_>>;
    fn fetch_user_by_id<'a>(
        &'a self,
        id: i64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Option<Value>>> + Send + 'a>>;
    fn create_user<'a>(
        &'a self,
        req: UsersRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Value>> + Send + 'a>>;
    fn update_user<'a>(
        &'a self,
        req: UsersRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Value>> + Send + 'a>>;
    fn change_password<'a>(
        &'a self,
        ident_user: i64,
        new_password: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Value>> + Send + 'a>>;
    fn delete_user<'a>(
        &'a self,
        id: i64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<bool>> + Send + 'a>>;
    // AkunKeuangan
    fn fetch_akun_keuangan_by_email<'a>(
        &'a self,
        email: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Vec<Value>>> + Send + 'a>>;
    fn fetch_akun_by_id<'a>(
        &'a self,
        id: i64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Option<Value>>> + Send + 'a>>;
    fn create_akun_keuangan<'a>(
        &'a self,
        req: AkunKeuanganRequest,
        email: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Value>> + Send + 'a>>;
    fn update_akun_keuangan<'a>(
        &'a self,
        req: AkunKeuanganRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Value>> + Send + 'a>>;
    // Category
    fn fetch_categories<'a>(
        &'a self,
        id_akun: i64,
        email: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Vec<Value>>> + Send + 'a>>;
    fn fetch_category_detail<'a>(
        &'a self,
        id_akun: i64,
        id_c: i64,
        email: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Option<Value>>> + Send + 'a>>;
    fn create_category<'a>(
        &'a self,
        cat: CategoryRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Value>> + Send + 'a>>;
    fn update_category<'a>(
        &'a self,
        cat: CategoryRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Value>> + Send + 'a>>;
    // TransaksiKeuangan
    fn fetch_transaksi_by_akun<'a>(
        &'a self,
        id_akun: i64,
        email: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Vec<Value>>> + Send + 'a>>;
    /// Complete legacy list envelope, encoded before sending HTTP headers so
    /// database/serialization failures still use the normal error response.
    /// PostgreSQL overrides this to avoid retaining a large nested JSON tree.
    fn fetch_transaksi_list_json<'a>(
        &'a self,
        id_akun: i64,
        email: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            let list = self.fetch_transaksi_by_akun(id_akun, email).await?;
            Ok(serde_json::to_vec(&serde_json::json!({"data": list, "IdAkun": id_akun}))?)
        })
    }
    fn fetch_transaksi_detail<'a>(
        &'a self,
        id_akun: i64,
        id_tk: i64,
        email: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Option<Value>>> + Send + 'a>>;
    fn create_transaksi<'a>(
        &'a self,
        tk: TransaksiKeuanganRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Value>> + Send + 'a>>;
    fn update_transaksi<'a>(
        &'a self,
        tk: TransaksiKeuanganRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Value>> + Send + 'a>>;
    fn fetch_transaksi_not_in_settlement_engine(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Vec<Value>>> + Send + '_>>;
    // Audit
    fn fetch_event_entries<'a>(
        &'a self,
        event_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Vec<Value>>> + Send + 'a>>;
}
