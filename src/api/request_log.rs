//! Request diagnostics without credentials, tokens, query strings or bodies.
use axum::{extract::Request, middleware::Next, response::Response};
use tracing::Instrument;

pub async fn log_request(req: Request, next: Next) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    let span = tracing::info_span!("http", request_id = %request_id,
        method = %req.method(), path = %req.uri().path());
    async move {
        let content_type = req.headers().get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()).unwrap_or("(none)").to_owned();
        let started = std::time::Instant::now();
        tracing::info!(%content_type, "request started");
        let mut response = next.run(req).await;
        let status = response.status().as_u16();
        let elapsed_ms = started.elapsed().as_millis() as u64;
        if status >= 500 {
            tracing::error!(status, elapsed_ms, "request completed");
        } else if status >= 400 {
            tracing::warn!(status, elapsed_ms, "request completed");
        } else {
            tracing::info!(status, elapsed_ms, "request completed");
        }
        response.headers_mut().insert("x-request-id", request_id.parse().unwrap());
        response
    }.instrument(span).await
}
