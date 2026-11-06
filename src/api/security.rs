//! Form/query binding used by Java's unannotated @ModelAttribute parameters.
use crate::{
    api::{auth_middleware::AuthUser, errors},
    security::*,
    AppState,
};
use axum::{
    extract::{Extension, OriginalUri, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use std::sync::Arc;

pub fn failure(error: anyhow::Error, path: &str) -> Response {
    if let Some(denied) = error.downcast_ref::<AccessDenied>() {
        return (
            StatusCode::NOT_ACCEPTABLE,
            Json(serde_json::json!({"response":denied.to_string()})),
        )
            .into_response();
    }
    // AkunValidationException (unlike AkunRestValidationException) is not
    // handled by Java's 400 advice; role business validation remains 500.
    tracing::error!(error=%error, path, "security operation failed");
    errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, path)
}
async fn read(state: Arc<AppState>, actor: AuthUser, query: SecurityQuery, path: &str) -> Response {
    match state.repo.security_query(query, &actor.0).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => failure(error, path),
    }
}
macro_rules! read_route {
    ($name:ident,$variant:expr) => {
        pub async fn $name(
            State(state): State<Arc<AppState>>,
            Extension(actor): Extension<AuthUser>,
            uri: OriginalUri,
        ) -> Response {
            read(state, actor, $variant, uri.0.path()).await
        }
    };
}
read_route!(roles, SecurityQuery::Roles(None));
read_route!(poa_roles, SecurityQuery::PoaRoles);
read_route!(functions, SecurityQuery::Functions);
read_route!(auth_levels, SecurityQuery::AuthLevels);
read_route!(access_levels, SecurityQuery::AccessLevels);
read_route!(authorized_profiles, SecurityQuery::AuthorizedProfiles);
pub async fn roles_search(
    State(state): State<Arc<AppState>>,
    Extension(actor): Extension<AuthUser>,
    Path(pattern): Path<String>,
    uri: OriginalUri,
) -> Response {
    read(
        state,
        actor,
        SecurityQuery::Roles(Some(pattern)),
        uri.0.path(),
    )
    .await
}
pub async fn write(
    State(state): State<Arc<AppState>>,
    Extension(actor): Extension<AuthUser>,
    uri: OriginalUri,
    headers: HeaderMap,
    body: String,
) -> Response {
    let is_form = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        })
        .unwrap_or(false);
    let role = || bind_form::<RoleRequest>(uri.0.query(), &body, is_form);
    let profile = || bind_form::<ProfileRequest>(uri.0.query(), &body, is_form);
    let command = match uri.0.path().rsplit('/').next().unwrap_or("") {
        "create-user-role" => role().map(SecurityWrite::CreateRole),
        "update-user-role" => role().map(SecurityWrite::UpdateRole),
        "delete-user-role" => role().map(SecurityWrite::DeleteRole),
        "create-permission-profile" => profile().map(SecurityWrite::CreateProfile),
        "update-permission-profile" => profile().map(SecurityWrite::UpdateProfile),
        "deactivate-permission-profile" => profile().map(SecurityWrite::DeactivateProfile),
        _ => unreachable!(),
    };
    let command = match command {
        Ok(c) => c,
        Err(_) => return errors::whitelabel_error(StatusCode::BAD_REQUEST, uri.0.path()),
    };
    match state.repo.security_write(command, &actor.0).await {
        Ok(serde_json::Value::Null) => errors::empty_body(StatusCode::OK),
        Ok(value) => Json(value).into_response(),
        Err(error) => failure(error, uri.0.path()),
    }
}
