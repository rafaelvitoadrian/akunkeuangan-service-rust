//! Explicit frontend origin allowlist for cross-domain Railway deployments.
use anyhow::{ensure, Context, Result};
use axum::http::{header, HeaderName, HeaderValue, Method, Uri};
use tower_http::cors::CorsLayer;

pub fn from_env() -> Result<Option<CorsLayer>> {
    let raw = std::env::var("CORS_ALLOWED_ORIGINS").unwrap_or_default();
    if raw.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(from_origins(&raw)?))
}

fn from_origins(raw: &str) -> Result<CorsLayer> {
    let mut origins = Vec::new();
    for origin in raw.split(',').map(str::trim) {
        let uri: Uri = origin.parse().context("CORS origin must be an http(s) origin")?;
        let scheme = uri.scheme_str().unwrap_or("");
        let authority = uri.authority().context("CORS origin must include a host")?;
        ensure!(
            matches!(scheme, "http" | "https")
                && !authority.as_str().contains('@')
                && !authority.as_str().contains('*')
                && origin == format!("{scheme}://{authority}"),
            "CORS origins must be exact http(s) origins without path, trailing slash or wildcard"
        );
        origins.push(origin.parse::<HeaderValue>().context("invalid CORS origin header")?);
    }
    Ok(CorsLayer::new()
        .allow_origin(origins)
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
        .allow_headers([
            header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT,
            HeaderName::from_static("x-requested-with"),
            HeaderName::from_static("x-request-id"),
            HeaderName::from_static("roles"), HeaderName::from_static("filter"),
        ])
        .expose_headers([header::AUTHORIZATION, HeaderName::from_static("x-request-id")]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{auth, db::memory::MemoryRepo, router, AppState};
    use axum::{body::Body, http::{Request, StatusCode}};
    use std::sync::Arc;
    use tower::ServiceExt;

    #[test]
    fn rejects_wildcard_paths_and_non_http_origins() {
        for raw in ["*", "https://*.frontend.test", "null", "ftp://frontend.test", "https://frontend.test/", "https://frontend.test/path", "https://frontend.test?x=1", "https://user@frontend.test", "https://frontend.test,"] {
            assert!(from_origins(raw).is_err(), "accepted invalid origin");
        }
        assert!(from_origins("https://frontend.test,http://localhost:5173").is_ok());
    }

    #[tokio::test]
    async fn browser_preflight_works_and_auth_stays_protected() {
        let app = router(Arc::new(AppState {
            repo: Arc::new(MemoryRepo::new()), security_key: auth::SecurityKey::Test,
        })).layer(from_origins("https://frontend.test").unwrap());
        let response = app.clone().oneshot(Request::builder()
            .method("OPTIONS").uri("/api/v2/user")
            .header(header::ORIGIN, "https://frontend.test")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PUT")
            .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization,content-type")
            .body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "https://frontend.test");
        assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_CREDENTIALS], "true");
        for (origin, allowed) in [("https://frontend.test", true), ("https://other.test", false)] {
            let response = app.clone().oneshot(Request::builder()
                .uri("/api/v2/user").header(header::ORIGIN, origin)
                .body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(response.headers().contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN), allowed);
        }
    }
}
