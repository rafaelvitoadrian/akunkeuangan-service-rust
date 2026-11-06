//! Deployment readiness: migrations complete before listening; probe the DB too.
use crate::AppState;
use axum::{extract::State, http::{header, StatusCode}, response::IntoResponse, Json};
use serde_json::json;
use std::{sync::Arc, time::Duration};

pub async fn ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let ready = matches!(
        tokio::time::timeout(Duration::from_secs(3), state.repo.get_system_prop_bool("is.test")).await,
        Ok(Ok(_))
    );
    let (status, label) = if ready {
        (StatusCode::OK, "UP")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "DOWN")
    };
    // Never return DB errors, connection strings or configuration to the probe.
    (status, [(header::CACHE_CONTROL, "no-store")], Json(json!({"status": label})))
}
