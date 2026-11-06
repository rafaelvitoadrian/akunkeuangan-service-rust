//! Error response — meniru perilaku Spring Boot 2.7 default (Whitelabel /error JSON)
//! dan error eksplisit dari controller lama.
//!
//! Sistem lama TIDAK punya @ControllerAdvice. Jadi:
//! - Missing/failed param → 400 dengan body Whitelabel JSON
//! - Malformed JSON → 400 Whitelabel
//! - Unknown path → 404 Whitelabel
//! - Uncaught exception → 500 Whitelabel
//! - Token invalid/expired (dengan header Authorization: Bearer ...) → 401 dari filter
//! - Endpoint protected tanpa auth → 401 (Spring Security default untuk request tanpa
//!   credentials di resource protected: sebenarnya 403 untuk anonymous? Di Spring Security 5.7+
//!   default AuthenticationEntryPoint = Http403ForbiddenEntryPoint TIDAK; default = 401 via
//!   LoginUrlAuthenticationEntryPoint... konfigurasi lama tanpa formLogin/custom entry point.
//!   DARI REKAMAN LIVE: request tanpa token ke /api/v2/user → 401.
//!   Format body direkam di golden/headers.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Whitelabel /error JSON ala Spring Boot:
/// {"timestamp":"yyyy-MM-dd'T'HH:mm:ss.SSSXXX","status":N,"error":"...","path":"..."}
/// (field "message" tidak muncul kecuali server.error.include-message=always)
pub fn whitelabel_error(status: StatusCode, path: &str) -> Response {
    let now = chrono::Utc::now();
    // Spring default: ISO offset dengan millis, timezone JVM (di server lama UTC+00:00 terlihat di rekaman)
    let timestamp = now.format("%Y-%m-%dT%H:%M:%S%.3f").to_string()
        + "+00:00"; // JVM default container: UTC (sesuai rekaman live)
    let body = json!({
        "timestamp": timestamp,
        "status": status.as_u16(),
        "error": status.canonical_reason().unwrap_or("Unknown"),
        "path": path,
    });
    (status, axum::Json(body)).into_response()
}

/// SecurityController `/login` (v1): userService.authenticateUser() == false
/// → 401 `{"error": "Invalid username or password"}`
pub fn login_invalid_credentials() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(json!({"error": "Invalid username or password"})),
    )
        .into_response()
}

/// SecurityController `/loginv2`: SessionKernel.login() →
/// AuthenticatorDriver.authenticate() melempar `AkunRestValidationException`
/// → ditangani `AkunValidationExceptionHandler` (@RestControllerAdvice)
/// → **400** `{"response": "<pesan i18n>"}`.
///
/// Pesan dari ErrorMessages_en.properties:
///   invalid.user.name.password=Invalid user name/password
/// Perhatikan spasi pada "user name" — berbeda dari pesan `/login`.
pub fn loginv2_invalid_credentials() -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(json!({"response": "Invalid user name/password"})),
    )
        .into_response()
}

/// Body KOSONG tanpa header Content-Type.
///
/// Java (Tomcat) mengirim `Content-Length: 0` dan TIDAK mengirim Content-Type
/// untuk ResponseEntity tanpa body. Axum menambahkan `text/plain; charset=utf-8`
/// bila memakai `(status, "")`, jadi respons dibangun manual.
pub fn empty_body(status: StatusCode) -> Response {
    let mut resp = Response::new(axum::body::Body::empty());
    *resp.status_mut() = status;
    resp.headers_mut()
        .insert(axum::http::header::CONTENT_LENGTH, "0".parse().unwrap());
    resp
}
