//! Middleware auth — meniru SecurityConfig + JwtAuthenticationFilter dari sistem lama.
//!
//! Perilaku yang direplikasi (direkam dari sistem berjalan 2026-10-17):
//! 1. Path di permitAll → diteruskan tanpa token.
//!    permitAll di SecurityConfig: `/api/v1/user/register`, `/api/v1/user/*`,
//!    `/api/v1/integration/*`.
//!    CATATAN: `/api/v1/integration` (tanpa segmen tambahan) TIDAK cocok dengan
//!    pola `/api/v1/integration/*` → **butuh token** (direkam: 403 tanpa token).
//! 2. Path protected tanpa header Authorization → 403 body KOSONG
//!    (Http403ForbiddenEntryPoint — direkam: users_no_token 403 0B).
//! 3. Header ada tapi token invalid/expired → 401 body KOSONG
//!    (JwtAuthenticationFilter setStatus(401) — direkam: users_bad_token 401 0B).
//! 4. Path tidak dikenal tanpa token → **403**, bukan 404, karena filter berjalan
//!    sebelum dispatcher (direkam: unknown_path 403 0B).
//! 5. Token valid → principal = subject (email); authorities selalu KOSONG,
//!    sehingga `@PreAuthorize("hasAuthority('FileUpload')")` tidak pernah lolos.

use crate::auth::validate_token;
use crate::AppState;
use axum::extract::{Request, State};
use axum::http::{header::AUTHORIZATION, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

/// Principal pengganti SecurityContextHolder.
#[derive(Clone, Debug)]
pub struct AuthUser(pub String);

/// SecurityConfig permitAll — dicocokkan seperti Spring AntPathMatcher.
pub fn is_permit_all(path: &str) -> bool {
    if path == "/api/v1/user/register" {
        return true;
    }
    // "/api/v1/user/*" — tepat satu segmen setelah prefix
    if let Some(rest) = path.strip_prefix("/api/v1/user/") {
        if !rest.is_empty() && !rest.contains('/') {
            return true;
        }
    }
    // "/api/v1/integration/*" — tepat satu segmen; path tanpa segmen TIDAK cocok
    if let Some(rest) = path.strip_prefix("/api/v1/integration/") {
        if !rest.is_empty() && !rest.contains('/') {
            return true;
        }
    }
    false
}

pub async fn authenticate(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();

    let header = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let Some(header_value) = header else {
        if is_permit_all(&path) { return next.run(req).await; }
        return crate::api::errors::empty_body(StatusCode::FORBIDDEN);
    };

    let Some(token) = header_value.strip_prefix("Bearer ") else {
        if is_permit_all(&path) { return next.run(req).await; }
        return crate::api::errors::empty_body(StatusCode::FORBIDDEN);
    };

    match validate_token(token.trim(), &state.security_key) {
        Some(email) => {
            req.extensions_mut().insert(AuthUser(email));
            next.run(req).await
        }
        None => crate::api::errors::empty_body(StatusCode::UNAUTHORIZED),
    }
}

#[cfg(test)]
mod tests {
    use super::is_permit_all;

    #[test]
    fn permit_all_matches_spring_patterns() {
        assert!(is_permit_all("/api/v1/user/login"));
        assert!(is_permit_all("/api/v1/user/loginv2"));
        assert!(is_permit_all("/api/v1/user/register"));
        // pola "/api/v1/integration/*" butuh satu segmen
        assert!(is_permit_all("/api/v1/integration/run"));
        // direkam: tanpa segmen tetap butuh token
        assert!(!is_permit_all("/api/v1/integration"));
        // dua segmen tidak cocok dengan pola satu segmen
        assert!(!is_permit_all("/api/v1/user/login/extra"));
        assert!(!is_permit_all("/api/v2/user"));
        assert!(!is_permit_all("/api/v1/akunkeuangan"));
    }
}
