//! Routes — meniru perilaku controller lama per endpoint.
//! Setiap handler mempertahankan: path, method, params, response shape, status code.

use crate::api::errors;
use crate::models::*;
use crate::AppState;
use crate::api::auth_middleware::AuthUser;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

type S = Arc<AppState>;

// ---------------------------------------------------------------------------
// SecurityController — base /api/v1/user, permitAll
// ---------------------------------------------------------------------------

/// POST /api/v1/user/login?email=&password=
/// 200: {"token": ...} + header Authorization: Bearer <token>
/// 401: {"error": "Invalid username or password"}
/// Missing params → 400 Whitelabel (Spring default)
pub async fn login(
    Query(mut params): Query<HashMap<String, String>>,
    State(state): State<S>,
    headers: HeaderMap,
    original: axum::extract::OriginalUri,
    form: Result<axum::Form<HashMap<String, String>>, axum::extract::rejection::FormRejection>,
) -> Response {
    if let Ok(axum::Form(form)) = form {
        for (key, value) in form { params.entry(key).or_insert(value); }
    }
    if !params.contains_key("email") || !params.contains_key("password") {
        tracing::warn!(email_present = params.contains_key("email"),
            password_present = params.contains_key("password"),
            "login rejected: missing email/password parameters; use query parameters or application/x-www-form-urlencoded (JSON body is not a login parameter source)");
    }
    let Some(email) = params.get("email") else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/user/login");
    };
    let Some(password) = params.get("password") else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/user/login");
    };

    // UserService.authenticateUser — SHA-1 hex di kolom `password`
    let authenticated = match state
        .repo
        .authenticate_user_by_email(email, password)
        .await {
            Ok(value)=>value,
            Err(e) => {
                tracing::error!(error = %e, "login database error");
                return errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR,"/api/v1/user/login");
            },
        };

    if !authenticated {
        tracing::warn!("login rejected: invalid credentials");
        return errors::login_invalid_credentials();
    }

    // issuer = base URI, audience = FULL request URI termasuk query string
    // (ServletUriComponentsBuilder.fromCurrentRequest())
    let issuer = base_uri(&headers);
    // ServletUriComponentsBuilder.fromCurrentRequest() → path + query string
    let audience = format!("{}{}", base_uri(&headers), original.0.path_and_query().map(|p| p.as_str()).unwrap_or("/api/v1/user/login"));
    let now = chrono::Utc::now().timestamp();

    match crate::auth::generate_token(email, &issuer, &audience, &state.security_key, now) {
        Ok(token) => {
            let mut resp = (StatusCode::OK, Json(json!({ "token": token }))).into_response();
            resp.headers_mut().insert(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {}", token)
                    .parse()
                    .unwrap(),
            );
            resp
        }
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/user/login")
        },
    }
}

/// POST /api/v1/user/loginv2 — kontrak identik dengan /login (auth path berbeda:
/// SessionKernel.login → SHA-256(salt+password) di kolom PWD)
pub async fn loginv2(
    Query(mut params): Query<HashMap<String, String>>,
    State(state): State<S>,
    headers: HeaderMap,
    original: axum::extract::OriginalUri,
    form: Result<axum::Form<HashMap<String, String>>, axum::extract::rejection::FormRejection>,
) -> Response {
    if let Ok(axum::Form(form)) = form {
        for (key, value) in form { params.entry(key).or_insert(value); }
    }
    if !params.contains_key("email") || !params.contains_key("password") {
        tracing::warn!(email_present = params.contains_key("email"),
            password_present = params.contains_key("password"),
            "login rejected: missing email/password parameters; use query parameters or application/x-www-form-urlencoded (JSON body is not a login parameter source)");
    }
    let Some(email) = params.get("email") else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/user/loginv2");
    };
    let Some(password) = params.get("password") else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/user/loginv2");
    };

    let authenticated = match state.repo.authenticate_user_v2(email, password).await {
        Ok(value)=>value,
        Err(e) => {
            tracing::error!(error = %e, "loginv2 database error");
            return errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR,"/api/v1/user/loginv2");
        },
    };
    if !authenticated {
        tracing::warn!("login rejected: invalid credentials");
        // Berbeda dari /login: 400 + {"response": ...}
        return errors::loginv2_invalid_credentials();
    }

    let issuer = base_uri(&headers);
    let audience = format!("{}{}", base_uri(&headers), original.0.path_and_query().map(|p| p.as_str()).unwrap_or("/api/v1/user/loginv2"));
    let now = chrono::Utc::now().timestamp();

    match crate::auth::generate_token(email, &issuer, &audience, &state.security_key, now) {
        Ok(token) => {
            let mut resp = (StatusCode::OK, Json(json!({ "token": token }))).into_response();
            resp.headers_mut().insert(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {}", token).parse().unwrap(),
            );
            resp
        }
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/user/loginv2")
        },
    }
}

// ---------------------------------------------------------------------------
// UserController — base /api/v2/user (auth)
// ---------------------------------------------------------------------------

/// GET /api/v2/user → 200 Users[] (current Java model DTO).
pub async fn user_get_all(State(state): State<S>,Extension(actor):Extension<AuthUser>) -> Response {
    if let Err(error)=state.repo.require_function(&actor.0,"UserAccount",2).await {
        return super::security::failure(error,"/api/v2/user");
    }
    match state.repo.fetch_all_user_models().await {
        Ok(models) => Json(json!(models)).into_response(),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v2/user")
        },
    }
}

/// GET /api/v2/user/{id} → UserPO langsung (body bisa null dengan 200 jika tidak ketemu)
pub async fn user_get_by_id(State(state): State<S>, original: axum::extract::OriginalUri,
    id: Result<Path<i64>, axum::extract::rejection::PathRejection>) -> Response {
    let Ok(Path(id)) = id else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, original.0.path());
    };
    match state.repo.fetch_user_by_id(id).await {
        Ok(Some(user)) => user_response(&state,user,original.0.path()).await,
        Ok(None) => errors::empty_body(StatusCode::OK),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, &format!("/api/v2/user/{}", id))
        },
    }
}

/// POST /api/v2/user → 200 UserPO (dibuat via createMasterAndRevision)
pub async fn user_create(State(state): State<S>, Extension(actor): Extension<AuthUser>, body: String) -> Response {
    let mut user: UsersRequest = match crate::util::jackson_serde::parse_request(&body) {
        Ok(u) => u,
        Err(_) => return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v2/user"),
    };
    if let Err(error)=state.repo.require_function(&actor.0,"UserAccount",4).await {
        return super::security::failure(error,"/api/v2/user");
    }
    user.actor_ident=actor_ident(&state,&actor.0).await;
    match state.repo.create_user_model(user).await {
        Ok(created) => Json(created).into_response(),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v2/user")
        },
    }
}

/// POST /api/v2/user/change-password → 200 UserPO
pub async fn user_change_password(State(state): State<S>, body: String) -> Response {
    let req: ChangePasswordRequest = match crate::util::jackson_serde::parse_request(&body) {
        Ok(r) => r,
        Err(_) => return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v2/user/change-password"),
    };
    match state
        .repo
        .change_password(req.ident_user.unwrap_or(0), &req.new_password.unwrap_or_default())
        .await
    {
        Ok(updated) => (StatusCode::OK, Json(updated)).into_response(),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v2/user/change-password")
        },
    }
}

/// PUT /api/v2/user → 200 UserPO
pub async fn user_update(State(state): State<S>, Extension(actor): Extension<AuthUser>, body: String) -> Response {
    let mut user: UsersRequest = match crate::util::jackson_serde::parse_request(&body) {
        Ok(u) => u,
        Err(_) => return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v2/user"),
    };
    user.actor_ident=actor_ident(&state,&actor.0).await;
    match state.repo.update_user_model(user).await {
        Ok(updated) => Json(updated).into_response(),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v2/user")
        },
    }
}

async fn user_response(state:&S,raw:serde_json::Value,path:&str)->Response {
    match state.repo.user_model(raw).await {
        Ok(model)=>Json(model).into_response(),Err(error)=>super::security::failure(error,path),
    }
}

/// DELETE /api/v2/user/{id} → 200 `true`, atau 500 body KOSONG jika gagal
pub async fn user_delete(State(state): State<S>, original: axum::extract::OriginalUri,
    id: Result<Path<i64>, axum::extract::rejection::PathRejection>) -> Response {
    let Ok(Path(id)) = id else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, original.0.path());
    };
    match state.repo.delete_user(id).await {
        Ok(true) => (StatusCode::OK, Json(true)).into_response(),
        Ok(false) => errors::empty_body(StatusCode::INTERNAL_SERVER_ERROR),
        Err(_) => errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR,original.0.path()),
    }
}

// ---------------------------------------------------------------------------
// AkunKeuanganController — base /api/v1/akunkeuangan (auth)
// ---------------------------------------------------------------------------

/// GET /api/v1/akunkeuangan → {"data": [AkunKeuanganPO...]} (filter by Principal email)
pub async fn akun_get_all(State(state): State<S>, Extension(user): Extension<AuthUser>) -> Response {
    let email = user.0;
    match state.repo.fetch_akun_keuangan_by_email(&email).await {
        Ok(list) => (StatusCode::OK, Json(json!({ "data": list }))).into_response(),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/akunkeuangan")
        },
    }
}

/// GET /api/v1/akunkeuangan/{id} → AkunKeuanganPO (principal diabaikan di lookup lama)
pub async fn akun_get_by_id(State(state): State<S>, original: axum::extract::OriginalUri,
    id: Result<Path<i64>, axum::extract::rejection::PathRejection>) -> Response {
    let Ok(Path(id)) = id else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, original.0.path());
    };
    match state.repo.fetch_akun_by_id(id).await {
        Ok(Some(akun)) => (StatusCode::OK, Json(akun)).into_response(),
        Ok(None) => errors::empty_body(StatusCode::OK),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, &format!("/api/v1/akunkeuangan/{}", id))
        },
    }
}

/// GET /api/v1/akunkeuangan/test → String polos dengan Content-Type: application/xml
/// (TEMUAN: content-type XML tapi body string username — dipertahankan)
pub async fn akun_test(Extension(user): Extension<AuthUser>) -> Response {
    let email = user.0;
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/xml;charset=UTF-8")],
        email,
    )
        .into_response()
}

/// POST /api/v1/akunkeuangan → 200 AkunKeuanganPO (accountNumber di-generate dulu)
pub async fn akun_create(State(state): State<S>, Extension(user): Extension<AuthUser>, body: String) -> Response {
    let mut req: AkunKeuanganRequest = match crate::util::jackson_serde::parse_request(&body) {
        Ok(r) => r,
        Err(_) => return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/akunkeuangan"),
    };
    let email = user.0;
    req.actor_ident=actor_ident(&state,&email).await;
    match state.repo.create_akun_keuangan(req, &email).await {
        Ok(created) => (StatusCode::OK, Json(created)).into_response(),
        Err(e) => {
            if let Some(validation)=e.downcast_ref::<crate::db::ValidationError>() {
                return (StatusCode::BAD_REQUEST,Json(json!({"response":validation.0}))).into_response();
            }
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/akunkeuangan")
        },
    }
}

/// PUT /api/v1/akunkeuangan → 200 AkunKeuanganPO (accountNumber di-resolve dari DB)
pub async fn akun_update(State(state): State<S>, Extension(actor): Extension<AuthUser>, body: String) -> Response {
    let mut req: AkunKeuanganRequest = match crate::util::jackson_serde::parse_request(&body) {
        Ok(r) => r,
        Err(_) => return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/akunkeuangan"),
    };
    req.actor_ident=actor_ident(&state,&actor.0).await;
    match state.repo.update_akun_keuangan(req).await {
        Ok(updated) => (StatusCode::OK, Json(updated)).into_response(),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/akunkeuangan")
        },
    }
}

// ---------------------------------------------------------------------------
// CategoryController — base /api/v1/category (auth)
// ---------------------------------------------------------------------------

/// GET /api/v1/category/lists?idAkun= (required) → {"IdAkun": <long>, "data": [...]}
/// Kunci literal "IdAkun" (huruf I besar) — sesuai kode lama.
pub async fn category_lists(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<S>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let Some(id_akun_raw) = params.get("idAkun") else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/category/lists");
    };
    let Ok(id_akun) = id_akun_raw.parse::<i64>() else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/category/lists");
    };
    let email = user.0;
    match state.repo.fetch_categories(id_akun, &email).await {
        Ok(list) => (StatusCode::OK, Json(serde_json::json!({ "data": list, "IdAkun": id_akun }))).into_response(),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/category/lists")
        },
    }
}

/// GET /api/v1/category/detail?idAkun=&idC= (both required) → CategoryPO
pub async fn category_detail(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<S>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let (Some(id_akun_raw), Some(id_c_raw)) = (params.get("idAkun"), params.get("idC")) else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/category/detail");
    };
    let (Ok(id_akun), Ok(id_c)) = (id_akun_raw.parse::<i64>(), id_c_raw.parse::<i64>()) else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/category/detail");
    };
    let email = user.0;
    match state.repo.fetch_category_detail(id_akun, id_c, &email).await {
        Ok(Some(cat)) => (StatusCode::OK, Json(cat)).into_response(),
        Ok(None) => errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/category/detail"),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/category/detail")
        },
    }
}

/// POST /api/v1/category → CategoryPO
pub async fn category_create(State(state): State<S>, body: String) -> Response {
    let cat: CategoryRequest = match crate::util::jackson_serde::parse_request(&body) {
        Ok(c) => c,
        Err(_) => return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/category"),
    };
    match state.repo.create_category(cat).await {
        Ok(created) => (StatusCode::OK, Json(created)).into_response(),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/category")
        },
    }
}

/// PUT /api/v1/category → CategoryPO
pub async fn category_update(State(state): State<S>, body: String) -> Response {
    let cat: CategoryRequest = match crate::util::jackson_serde::parse_request(&body) {
        Ok(c) => c,
        Err(_) => return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/category"),
    };
    match state.repo.update_category(cat).await {
        Ok(updated) => (StatusCode::OK, Json(updated)).into_response(),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/category")
        },
    }
}

// ---------------------------------------------------------------------------
// TransaksiKeuanganController — base /api/v1/transaksikeuangan (auth)
// ---------------------------------------------------------------------------

/// GET /api/v1/transaksikeuangan/lists?idAkun= → {"IdAkun": <long>, "data": [...]}
pub async fn tk_lists(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<S>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let Some(id_akun_raw) = params.get("idAkun") else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/transaksikeuangan/lists");
    };
    let Ok(id_akun) = id_akun_raw.parse::<i64>() else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/transaksikeuangan/lists");
    };
    let email = user.0;
    match state.repo.fetch_transaksi_list_json(id_akun, &email).await {
        Ok(bytes) => (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "application/json")], bytes).into_response(),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/transaksikeuangan/lists")
        },
    }
}

/// GET /api/v1/transaksikeuangan/detail?idAkun=&idTk= → {"IdAkun":..., "data": TransaksiPO}
pub async fn tk_detail(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<S>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let (Some(id_akun_raw), Some(id_tk_raw)) = (params.get("idAkun"), params.get("idTk")) else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/transaksikeuangan/detail");
    };
    let (Ok(id_akun), Ok(id_tk)) = (id_akun_raw.parse::<i64>(), id_tk_raw.parse::<i64>()) else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/transaksikeuangan/detail");
    };
    let email = user.0;
    match state.repo.fetch_transaksi_detail(id_akun, id_tk, &email).await {
        Ok(Some(data)) => (StatusCode::OK, Json(serde_json::json!({ "data": data, "IdAkun": id_akun }))).into_response(),
        Ok(None) => errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/transaksikeuangan/detail"),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/transaksikeuangan/detail")
        },
    }
}

/// POST /api/v1/transaksikeuangan → TransaksiKeuanganPO (internalreference di-generate @PrePersist)
pub async fn tk_create(State(state): State<S>, body: String) -> Response {
    let tk: TransaksiKeuanganRequest = match crate::util::jackson_serde::parse_request(&body) {
        Ok(t) => t,
        Err(_) => return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/transaksikeuangan"),
    };
    match state.repo.create_transaksi(tk).await {
        Ok(created) => (StatusCode::OK, Json(created)).into_response(),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/transaksikeuangan")
        },
    }
}

/// PUT /api/v1/transaksikeuangan → TransaksiKeuanganPO
pub async fn tk_update(State(state): State<S>, body: String) -> Response {
    let tk: TransaksiKeuanganRequest = match crate::util::jackson_serde::parse_request(&body) {
        Ok(t) => t,
        Err(_) => return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/api/v1/transaksikeuangan"),
    };
    match state.repo.update_transaksi(tk).await {
        Ok(updated) => (StatusCode::OK, Json(updated)).into_response(),
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/transaksikeuangan")
        },
    }
}

// ---------------------------------------------------------------------------
// IntegrationController — base /api/v1/integration (permitAll, DENGAN SIDE EFFECT)
// ---------------------------------------------------------------------------

/// GET /api/v1/integration → 200 {"size": "N"}
/// SIDE EFFECT: tiap transaksi yang belum masuk settlement engine di-register
/// sebagai commit action (resend SI notification). Fase 1: resend logika DB-level
/// dijalankan; pengiriman JMS aktual adalah Fase 2.
pub async fn integration_check(State(state): State<S>) -> Response {
    match state.repo.fetch_transaksi_not_in_settlement_engine().await {
        Ok(list) => {
            let size = list.len();
            // Fase 2: register TKSIUpdateCommitAction per transaksi (kirim ke queue
            // siUpdateNotifications). Fase 1: hanya menghitung, dengan log.
            tracing::info!(
                "integration: {} transaksi belum masuk settlement engine (resend JMS = Fase 2)",
                size
            );
            (
                StatusCode::OK,
                Json(json!({ "size": size.to_string() })), // String.valueOf di Java → string
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/api/v1/integration")
        },
    }
}

// ---------------------------------------------------------------------------
// AuditController — /event/evententries (auth, Gson output)
// ---------------------------------------------------------------------------

/// GET /event/evententries?eventId= (required) → JSON via Gson:
/// - dateFormat "yyyy-MM-dd'T'HH:mm:ssXXX" (offset, system zone)
/// - disableHtmlEscaping
/// - compressedData byte[] → Gson default (array angka)
pub async fn event_entries(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<S>,
) -> Response {
    let Some(event_id) = params.get("eventId") else {
        return errors::whitelabel_error(StatusCode::BAD_REQUEST, "/event/evententries");
    };
    match state.repo.fetch_event_entries(event_id).await {
        Ok(entries) => {
            // Gson menulis List kosong sebagai [], List null sebagai null
            let json_str = serde_json::to_string(&entries).unwrap_or_else(|_| "null".to_string());
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                json_str,
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("internal error: {}", e);
            errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/event/evententries")
        },
    }
}

// ---------------------------------------------------------------------------
// Fallback & helpers
// ---------------------------------------------------------------------------

pub async fn fallback_404(req: axum::http::Request<axum::body::Body>) -> Response {
    let path = req.uri().path().to_string();
    errors::whitelabel_error(StatusCode::NOT_FOUND, &path)
}

/// 405 Method Not Allowed — Spring Boot tetap mengembalikan Whitelabel JSON.
/// Golden: golden/json/method_not_allowed.json (direkam dari sistem lama).
pub async fn fallback_405(req: axum::http::Request<axum::body::Body>) -> Response {
    let path = req.uri().path().to_string();
    errors::whitelabel_error(StatusCode::METHOD_NOT_ALLOWED, &path)
}

fn base_uri(headers: &HeaderMap) -> String {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    format!("http://{}", host)
}

async fn actor_ident(state:&S,email:&str)->Option<i64> {
    state.repo.resolve_actor_ident(email).await.ok().flatten()
}


/// GET /admin — TEMUAN: sistem lama mengembalikan **500** Whitelabel
/// (`{"timestamp":...,"status":500,"error":"Internal Server Error","path":"/admin"}`)
/// karena template Thymeleaf `admin.html` tidak ditemukan. Perilaku ini
/// DIPERTAHANKAN apa adanya (tidak diperbaiki diam-diam).
pub async fn admin_broken() -> Response {
    let mut response = errors::whitelabel_error(StatusCode::INTERNAL_SERVER_ERROR, "/admin");
    response.headers_mut().insert(axum::http::header::CONTENT_TYPE,
        "application/json;charset=UTF-8".parse().unwrap());
    response
}

/// Spring handles OPTIONS for an unconstrained @RequestMapping itself.
pub async fn admin_options() -> Response {
    ([(axum::http::header::ALLOW, "GET,HEAD,POST,PUT,PATCH,DELETE,OPTIONS")], ()).into_response()
}
