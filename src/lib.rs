//! akunkeuangan_service — library entrypoint.
//!
//! Dipakai oleh `main.rs` (server) dan integration test (`tests/`).
//! Kontrak eksternal terdokumentasi di docs/CONTRACT.md dan diverifikasi
//! oleh golden fixture di golden/.

pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod models;
pub mod lifecycle;
pub mod security;
pub mod util;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

pub type DynRepo = Arc<dyn db::Repository>;

pub struct AppState {
    pub repo: DynRepo,
    /// JWT key — random per-boot (default) atau statis saat SYSTEMPROP is.test=true
    pub security_key: auth::SecurityKey,
}

/// Bangun router lengkap — dipakai `main()` dan integration test.
///
/// Middleware auth dipasang GLOBAL (seperti Spring Security filter chain),
/// sehingga fallback 404 dan 405 juga terkena aturan auth:
/// path tidak dikenal tanpa token → 403 (direkam dari sistem lama).
pub fn router(state: Arc<AppState>) -> Router {
    let routes = Router::new()
        // ---- permitAll (dicek di middleware) ----
        .route("/api/v1/user/login", post(api::routes::login))
        .route("/api/v1/user/loginv2", post(api::routes::loginv2))
        // ---- protected ----
        // UserController
        .route(
            "/api/v2/user",
            get(api::routes::user_get_all)
                .post(api::routes::user_create)
                .put(api::routes::user_update),
        )
        .route(
            "/api/v2/user/:id",
            get(api::routes::user_get_by_id).delete(api::routes::user_delete),
        )
        .route(
            "/api/v2/user/change-password",
            post(api::routes::user_change_password),
        )
        .route("/api/v2/user/getUserRoles/:shortName", get(api::security::roles_search))
        .route("/api/v2/user/fetchUserRoles", get(api::security::roles))
        .route("/api/v2/user/fetchPoAUserRoles", get(api::security::poa_roles))
        .route("/api/v2/user/getAllPermissionFunctions", get(api::security::functions))
        .route("/api/v2/user/getAuthLevels", get(api::security::auth_levels))
        .route("/api/v2/user/getAccessLevels", get(api::security::access_levels))
        .route("/api/v2/user/fetchAuthorizedPermissionProfiles", get(api::security::authorized_profiles))
        .route("/api/v2/user/create-user-role", post(api::security::write))
        .route("/api/v2/user/update-user-role", axum::routing::put(api::security::write).delete(api::security::write))
        .route("/api/v2/user/delete-user-role", axum::routing::delete(api::security::write))
        .route("/api/v2/user/create-permission-profile", post(api::security::write))
        .route("/api/v2/user/update-permission-profile", axum::routing::put(api::security::write))
        .route("/api/v2/user/deactivate-permission-profile", post(api::security::write))
        // AkunKeuanganController
        .route(
            "/api/v1/akunkeuangan",
            get(api::routes::akun_get_all)
                .post(api::routes::akun_create)
                .put(api::routes::akun_update),
        )
        .route("/api/v1/akunkeuangan/test", get(api::routes::akun_test))
        .route("/api/v1/akunkeuangan/:id", get(api::routes::akun_get_by_id))
        // CategoryController
        .route(
            "/api/v1/category",
            post(api::routes::category_create).put(api::routes::category_update),
        )
        .route("/api/v1/category/lists", get(api::routes::category_lists))
        .route("/api/v1/category/detail", get(api::routes::category_detail))
        // TransaksiKeuanganController
        .route(
            "/api/v1/transaksikeuangan",
            post(api::routes::tk_create).put(api::routes::tk_update),
        )
        .route(
            "/api/v1/transaksikeuangan/lists",
            get(api::routes::tk_lists).layer(
                tower_http::compression::CompressionLayer::new().compress_when(
                    tower_http::compression::predicate::SizeAbove::new(4096),
                ),
            ),
        )
        .route(
            "/api/v1/transaksikeuangan/detail",
            get(api::routes::tk_detail),
        )
        // IntegrationController — TERNYATA protected (pola /* butuh segmen)
        .route("/api/v1/integration", get(api::routes::integration_check))
        // AuditController
        .route("/event/evententries", get(api::routes::event_entries))
        // UploadController — multipart, path multi-segmen
        .route("/fileupload", post(api::upload::upload))
        .route("/fileupload/", post(api::upload::upload))
        .route("/fileupload/*tail", post(api::upload::upload))
        // AdminController — TEMUAN: di sistem berjalan selalu 500 (template hilang);
        // perilaku lama dipertahankan (lihat docs/OPEN-QUESTIONS.md)
        .route("/admin", get(api::routes::admin_broken)
            .post(api::routes::admin_broken)
            .put(api::routes::admin_broken)
            .patch(api::routes::admin_broken)
            .delete(api::routes::admin_broken)
            .options(api::routes::admin_options))
        // 405 Whitelabel (Spring Boot juga JSON)
        .method_not_allowed_fallback(api::routes::fallback_405)
        .fallback(api::routes::fallback_404)
        .layer(middleware::from_fn(api::http_contract::request_media))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            api::auth_middleware::authenticate,
        ))
        .layer(middleware::from_fn(api::http_contract::security_headers))
        .layer(middleware::from_fn(api::request_log::log_request))
        .with_state(state.clone());

    // Public infrastructure endpoint kept outside the legacy auth/media filters.
    // Existing protected paths, fallback 403 and login semantics stay identical.
    routes.merge(Router::new().route("/health/ready", get(api::health::ready)).with_state(state))
}
