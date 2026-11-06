//! Spring MVC request media handling and Spring Security's default headers.
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

pub async fn request_media(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let json_body = matches!(req.method().as_str(), "POST" | "PUT")
        && matches!(path, "/api/v2/user" | "/api/v2/user/change-password"
            | "/api/v1/akunkeuangan" | "/api/v1/category" | "/api/v1/transaksikeuangan");
    if json_body {
        let media = req.headers().get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()).unwrap_or("application/octet-stream")
            .split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        if media != "application/json" && !(media.starts_with("application/") && media.ends_with("+json")) {
            return super::errors::whitelabel_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, path);
        }
    }
    next.run(req).await
}

pub async fn security_headers(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("x-xss-protection", "1; mode=block"),
        ("cache-control", "no-cache, no-store, max-age=0, must-revalidate"),
        ("pragma", "no-cache"), ("expires", "0"), ("x-frame-options", "DENY"),
    ] {
        response.headers_mut().insert(
            header::HeaderName::from_static(name), header::HeaderValue::from_static(value));
    }
    response
}
