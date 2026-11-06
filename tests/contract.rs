//! Golden / contract test — membandingkan respons server Rust dengan fixture
//! yang direkam dari sistem lama (Java) atau diturunkan dari kode lama.
//!
//! Test ini TIDAK memerlukan network: router dipanggil langsung lewat `oneshot`.

use akunkeuangan_service::{auth, db, models::*, router, AppState, DynRepo};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_EMAIL: &str = "admin@example.test";
const TEST_PASSWORD: &str = "password";

#[tokio::test]
async fn admin_request_mapping_accepts_standard_methods_and_spring_options() {
    let h = Harness::new();
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"] {
        let (status, _, body) = h.send(Request::builder().method(method).uri("/admin")
            .header(header::AUTHORIZATION, format!("Bearer {}", h.token()))
            .body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{method}");
        if method == "HEAD" { assert!(body.is_empty()); }
        else { assert_eq!(json(&body)["path"], "/admin"); }
    }
    let (status, headers, body) = h.send(Request::builder().method("OPTIONS").uri("/admin")
        .header(header::AUTHORIZATION, format!("Bearer {}", h.token()))
        .body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::ALLOW], "GET,HEAD,POST,PUT,PATCH,DELETE,OPTIONS");
    assert!(body.is_empty());
}

#[tokio::test]
async fn upload_wildcard_includes_the_empty_suffix_like_spring() {
    let h = Harness::new();
    for path in ["/fileupload", "/fileupload/", "/fileupload/dashboard"] {
        let body = "--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"probe.txt\"\r\nContent-Type: text/plain\r\n\r\nprobe\r\n--boundary--\r\n";
        let (status, _, bytes) = h.send(Request::builder().method("POST").uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {}", h.token()))
            .header(header::CONTENT_TYPE, "multipart/form-data; boundary=boundary")
            .body(Body::from(body)).unwrap()).await;
        // Valid multipart reaches Java's role rejection, never a 404. No file
        // is written because no permitted upload role is supplied.
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert!(bytes.is_empty());
    }
}

#[tokio::test]
async fn request_id_is_unique_for_login_errors_and_success() {
    let h = Harness::new();
    let (status, first, _) = h.post_form("/api/v1/user/loginv2").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let first_id = first.get("x-request-id").unwrap().to_str().unwrap();
    assert!(uuid::Uuid::parse_str(first_id).is_ok());
    let (status, second, _) = h.post_form(
        "/api/v1/user/loginv2?email=admin%40example.test&password=password").await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(first_id, second.get("x-request-id").unwrap().to_str().unwrap());
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Harness {
    app: axum::Router,
    key: auth::SecurityKey,
}

impl Harness {
    fn new() -> Self {
        let mem = db::memory::MemoryRepo::new();
        mem.seed_user(1, TEST_EMAIL, TEST_PASSWORD, Some("Admin"));
        mem.seed_akun(1, "Tabungan", 1);
        // is.test=true → JWT key statis, token bisa dibuat di test
        mem.set_prop("is.test", "true");
        let repo: DynRepo = Arc::new(mem);
        let key = auth::SecurityKey::new(true);
        let state = Arc::new(AppState {
            repo,
            security_key: key.clone(),
        });
        Self {
            app: router(state),
            key,
        }
    }

    fn token(&self) -> String {
        auth::generate_token(
            TEST_EMAIL,
            "http://localhost:9090",
            "http://localhost:9090/api/v1/user/login",
            &self.key,
            chrono::Utc::now().timestamp(),
        )
        .unwrap()
    }

    async fn send(&self, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, String) {
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, headers, String::from_utf8_lossy(&bytes).to_string())
    }

    async fn get(&self, path: &str) -> (StatusCode, axum::http::HeaderMap, String) {
        self.send(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
    }

    async fn get_auth(&self, path: &str) -> (StatusCode, axum::http::HeaderMap, String) {
        self.send(
            Request::builder()
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {}", self.token()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn post_form(&self, path: &str) -> (StatusCode, axum::http::HeaderMap, String) {
        self.send(
            Request::builder()
                .method("POST")
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn post_json_auth(&self, path: &str, body: &str) -> (StatusCode, String) {
        let (s, _, b) = self
            .send(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {}", self.token()))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await;
        (s, b)
    }
}

fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body bukan JSON: {e}\n{body}"))
}

fn golden(name: &str) -> String {
    std::fs::read_to_string(format!("golden/json/{name}.json"))
        .unwrap_or_else(|_| panic!("fixture golden/json/{name}.json tidak ada"))
        .trim()
        .to_string()
}

// ---------------------------------------------------------------------------
// 1. Login — kontrak SecurityController
// ---------------------------------------------------------------------------

/// RECORDED: golden/json/login_401.json
#[tokio::test]
async fn login_wrong_password_401_matches_golden() {
    let h = Harness::new();
    let (status, _, body) = h
        .post_form("/api/v1/user/login?email=admin@example.test&password=salah")
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, golden("login_401"));
}

#[tokio::test]
async fn login_missing_params_400_whitelabel() {
    let h = Harness::new();
    let (status, _, body) = h.post_form("/api/v1/user/login").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v = json(&body);
    // Spring Boot Whitelabel: timestamp, status, error, path (tanpa "message")
    assert_eq!(v["status"], 400);
    assert_eq!(v["error"], "Bad Request");
    assert_eq!(v["path"], "/api/v1/user/login");
    assert!(v["timestamp"].as_str().unwrap().ends_with("+00:00"));
}

#[tokio::test]
async fn login_success_returns_token_in_body_and_header() {
    let h = Harness::new();
    let (status, headers, body) = h
        .post_form(&format!(
            "/api/v1/user/login?email={}&password={}",
            TEST_EMAIL, TEST_PASSWORD
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    let v = json(&body);
    let token = v["token"].as_str().expect("body harus punya field token");

    // Header Authorization: Bearer <token> (HttpHeaders.AUTHORIZATION)
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .expect("header Authorization wajib ada")
        .to_str()
        .unwrap();
    assert_eq!(auth_header, format!("Bearer {}", token));

    // Token harus bisa dipakai untuk endpoint protected
    let (s2, _, _) = h
        .send(
            Request::builder()
                .uri("/api/v2/user")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(s2, StatusCode::OK);
}

/// RECORDED: /loginv2 TIDAK identik dengan /login saat gagal.
/// AuthenticatorDriver melempar AkunRestValidationException →
/// AkunValidationExceptionHandler → **400** `{"response": "..."}`
/// (pesan i18n `invalid.user.name.password`, perhatikan spasi "user name").
#[tokio::test]
async fn loginv2_wrong_password_400_response_key() {
    let h = Harness::new();
    let (status, _, body) = h
        .post_form("/api/v1/user/loginv2?email=admin@example.test&password=salah")
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, golden("loginv2_401"));
    let v = json(&body);
    assert_eq!(v["response"], "Invalid user name/password");
    assert!(v.get("error").is_none());
}

// ---------------------------------------------------------------------------
// 2. Auth middleware — meniru SecurityConfig + JwtAuthenticationFilter
// ---------------------------------------------------------------------------

/// Tanpa header Authorization → 403 body KOSONG (Http403ForbiddenEntryPoint)
#[tokio::test]
async fn protected_without_token_403_empty_body() {
    let h = Harness::new();
    for path in [
        "/api/v2/user",
        "/api/v1/akunkeuangan",
        "/api/v1/category/lists?idAkun=1",
        "/event/evententries?eventId=x",
    ] {
        let (status, _, body) = h.get(path).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "path {path}");
        assert!(body.is_empty(), "body harus kosong untuk {path}, dapat: {body}");
    }
}

/// Token invalid → 401 body KOSONG (JwtAuthenticationFilter setStatus(401))
#[tokio::test]
async fn protected_with_invalid_token_401_empty_body() {
    let h = Harness::new();
    let (status, _, body) = h
        .send(
            Request::builder()
                .uri("/api/v2/user")
                .header(header::AUTHORIZATION, "Bearer not.a.valid.jwt")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.is_empty(), "body harus kosong, dapat: {body}");
}

/// Header ada tapi bukan skema Bearer → diperlakukan anonim → 403
#[tokio::test]
async fn protected_with_non_bearer_header_403() {
    let h = Harness::new();
    let (status, _, _) = h
        .send(
            Request::builder()
                .uri("/api/v2/user")
                .header(header::AUTHORIZATION, "Basic abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// 3. Response shape per endpoint
// ---------------------------------------------------------------------------

/// GET /api/v1/akunkeuangan/test → body = email, Content-Type application/xml
/// TEMUAN: content-type XML tapi body string polos (dipertahankan).
#[tokio::test]
async fn akun_test_returns_username_with_xml_content_type() {
    let h = Harness::new();
    let (status, headers, body) = h.get_auth("/api/v1/akunkeuangan/test").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, TEST_EMAIL);
    // RECORDED: Tomcat menambahkan charset UTF-8
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap().to_str().unwrap(),
        "application/xml;charset=UTF-8"
    );
}

/// GET /api/v1/category/lists → key literal "IdAkun" (huruf I besar)
#[tokio::test]
async fn category_lists_uses_capital_IdAkun_key() {
    let h = Harness::new();
    let (status, _, body) = h.get_auth("/api/v1/category/lists?idAkun=1").await;
    assert_eq!(status, StatusCode::OK);
    let v = json(&body);
    assert_eq!(v["IdAkun"], 1);
    assert!(v["data"].is_array());
    assert!(v.get("idAkun").is_none(), "kunci harus 'IdAkun', bukan 'idAkun'");
}

#[tokio::test]
async fn transaksi_lists_uses_capital_IdAkun_key() {
    let h = Harness::new();
    let (status, _, body) = h.get_auth("/api/v1/transaksikeuangan/lists?idAkun=1").await;
    assert_eq!(status, StatusCode::OK);
    let v = json(&body);
    assert_eq!(v["IdAkun"], 1);
    assert!(v["data"].is_array());
}

#[tokio::test]
async fn category_lists_missing_idAkun_400() {
    let h = Harness::new();
    let (status, _, body) = h.get_auth("/api/v1/category/lists").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json(&body)["status"], 400);
}

#[tokio::test]
async fn category_lists_non_numeric_idAkun_400() {
    let h = Harness::new();
    let (status, _, _) = h.get_auth("/api/v1/category/lists?idAkun=abc").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Current Java UserController assembles Users DTOs, including nested lifecycle.
#[tokio::test]
async fn user_response_uses_current_dto_and_retains_credential_fields() {
    let h = Harness::new();
    let (status, _, body) = h.get_auth("/api/v2/user").await;
    assert_eq!(status, StatusCode::OK);
    let v = json(&body);
    assert!(v.is_array());
    let user = &v[0];
    assert_eq!(user["ident"], 1);
    assert_eq!(user["lifecycleEntity"]["identEntity"], 1);
    assert!(user.get("id").is_none());
    // TEMUAN: field sensitif bocor di sistem lama — dipertahankan
    assert!(user.get("pwdHash").is_some(), "pwdHash harus ada (temuan lama)");
    assert!(user.get("password").is_some(), "password harus ada (temuan lama)");
    assert!(user.get("salt").is_some(), "salt harus ada (temuan lama)");
    assert!(user["lifecycleEntity"].get("revisionState").is_some());
    assert!(user["permissionProfiles"].is_array());
}

/// AkunKeuanganPO.createdDate = epoch millis (TANPA @JsonFormat di Java)
#[tokio::test]
async fn akun_created_date_is_epoch_millis_number() {
    let h = Harness::new();
    let (status, _, body) = h.get_auth("/api/v1/akunkeuangan").await;
    assert_eq!(status, StatusCode::OK);
    let v = json(&body);
    let akun = &v["data"][0];
    // RECORDED: createdDate/updatedDate null (AkunKeuanganPO.init() dikomentari di Java).
    // Jika terisi, Jackson menulis epoch-millis karena tidak ada @JsonFormat.
    assert!(akun["createdDate"].is_null() || akun["createdDate"].is_number());
    assert!(akun["saldo"].is_number(), "saldo harus angka (bukan string)");
    // RECORDED: AkunKeuanganPO punya "ident" di awal (getIdent()) dan "entityIdent" di akhir
    assert_eq!(akun["ident"], akun["id"]);
    assert_eq!(akun["entityIdent"], akun["id"]);
}

/// Entity tidak ditemukan → 200 dengan body literal `null` (bukan 404)
#[tokio::test]
async fn missing_entity_returns_200_empty_body() {
    let h = Harness::new();
    let (status, _, body) = h.get_auth("/api/v1/akunkeuangan/999999").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());
}

#[tokio::test]
async fn missing_details_and_invalid_ids_match_spring_errors() {
    let h = Harness::new();
    for (path, expected) in [
        ("/api/v2/user/abc", StatusCode::BAD_REQUEST),
        ("/api/v1/akunkeuangan/abc", StatusCode::BAD_REQUEST),
        ("/api/v1/category/detail?idAkun=1&idC=999999", StatusCode::INTERNAL_SERVER_ERROR),
        ("/api/v1/transaksikeuangan/detail?idAkun=1&idTk=999999", StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let (status, headers, body) = h.get_auth(path).await;
        assert_eq!(status, expected, "{path}");
        assert_eq!(headers[header::CONTENT_TYPE], "application/json");
        let value = json(&body);
        assert_eq!(value["status"], expected.as_u16());
        assert_eq!(value["path"], path.split('?').next().unwrap());
    }
    for path in ["/api/v2/user/999999", "/api/v1/akunkeuangan/999999"] {
        let (status, headers, body) = h.get_auth(path).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_empty());
        assert!(!headers.contains_key(header::CONTENT_TYPE));
    }
}

#[tokio::test]
async fn json_endpoints_reject_unsupported_media_before_parsing() {
    let h = Harness::new();
    for path in ["/api/v2/user", "/api/v2/user/change-password", "/api/v1/akunkeuangan",
        "/api/v1/category", "/api/v1/transaksikeuangan"] {
        let (status, _, body) = h.send(Request::builder().method("POST").uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {}", h.token()))
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::from("{\"name\":\"must not persist\"}")).unwrap()).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{path}");
        assert_eq!(json(&body)["error"], "Unsupported Media Type");
    }
}

#[tokio::test]
async fn invalid_token_has_empty_body_and_security_headers() {
    let h = Harness::new();
    let (status, headers, body) = h.send(Request::builder().uri("/api/v2/user")
        .header(header::AUTHORIZATION, "Bearer invalid").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.is_empty());
    assert!(!headers.contains_key(header::CONTENT_TYPE));
    assert_eq!(headers["x-frame-options"], "DENY");
    assert_eq!(headers["cache-control"], "no-cache, no-store, max-age=0, must-revalidate");
}

#[test]
fn request_property_names_match_java_dtos() {
    let cat: CategoryRequest = serde_json::from_str(r#"{"category_name":"test","akunKeuangan":{"id":630034}}"#).unwrap();
    assert_eq!(cat.akun_keuangan.unwrap()["id"], 630034);
    let akun: AkunKeuanganRequest = serde_json::from_str(r#"{"lifecycleEntity":{"activationType":{"identActivationType":1}}}"#).unwrap();
    assert!(akun.lifecycle_entity.is_some());
    let user: UsersRequest = serde_json::from_str(r#"{"lifecycleEntity":{"identMaster":1}}"#).unwrap();
    assert_eq!(user.lifecycle_entity.unwrap()["identMaster"], 1);
}

#[tokio::test]
async fn login_accepts_servlet_form_parameters() {
    let h = Harness::new();
    for endpoint in ["login", "loginv2"] {
        let (status, headers, body) = h.send(Request::builder().method("POST")
            .uri(format!("/api/v1/user/{endpoint}"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!("email=admin%40example.test&password={TEST_PASSWORD}"))).unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        let token = json(&body)["token"].as_str().unwrap().to_string();
        assert_eq!(headers[header::AUTHORIZATION], format!("Bearer {token}"));
    }
}

// ---------------------------------------------------------------------------
// 4. Error fallback — 404 / 405 Whitelabel
// ---------------------------------------------------------------------------

/// RECORDED: tanpa token, path tak dikenal → 403 body kosong (filter jalan dulu).
#[tokio::test]
async fn unknown_path_without_token_403() {
    let h = Harness::new();
    let (status, _, body) = h.get("/api/v1/doesnotexist").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.is_empty(), "body harus kosong, dapat: {body}");
}

/// Dengan token valid, path tak dikenal → 404 Whitelabel.
#[tokio::test]
async fn unknown_path_with_token_404_whitelabel() {
    let h = Harness::new();
    let (status, _, body) = h.get_auth("/api/v1/doesnotexist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let v = json(&body);
    assert_eq!(v["status"], 404);
    assert_eq!(v["error"], "Not Found");
    assert_eq!(v["path"], "/api/v1/doesnotexist");
}

/// RECORDED: golden/json/method_not_allowed.json
/// Struktur harus identik; timestamp berbeda (waktu request).
#[tokio::test]
async fn method_not_allowed_405_whitelabel_matches_golden_shape() {
    let h = Harness::new();
    let (status, _, body) = h.get("/api/v1/user/login").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

    let expected = json(&golden("method_not_allowed"));
    let actual = json(&body);
    assert_eq!(actual["status"], expected["status"]);
    assert_eq!(actual["error"], expected["error"]);
    assert_eq!(actual["path"], expected["path"]);
    // format timestamp: yyyy-MM-dd'T'HH:mm:ss.SSS+00:00
    let ts = actual["timestamp"].as_str().unwrap();
    assert_eq!(ts.len(), 29, "timestamp: {ts}");
    assert!(ts.ends_with("+00:00"), "timestamp: {ts}");
    assert_eq!(&ts[4..5], "-");
    assert_eq!(&ts[10..11], "T");
}

// ---------------------------------------------------------------------------
// 5. DELETE user — kontrak aneh: false → 500 body kosong
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_existing_user_returns_true() {
    let h = Harness::new();
    let (status, _, body) = h
        .send(
            Request::builder()
                .method("DELETE")
                .uri("/api/v2/user/1")
                .header(header::AUTHORIZATION, format!("Bearer {}", h.token()))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "true");
}

#[tokio::test]
async fn delete_missing_user_returns_500_empty_body() {
    let h = Harness::new();
    let (status, _, body) = h
        .send(
            Request::builder()
                .method("DELETE")
                .uri("/api/v2/user/424242")
                .header(header::AUTHORIZATION, format!("Bearer {}", h.token()))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.is_empty(), "body harus kosong, dapat: {body}");
}

// ---------------------------------------------------------------------------
// 6. Business rules — generator & perhitungan
// ---------------------------------------------------------------------------

/// AccountIdentifierGenerator: AKN01 + 6 random + 3-digit tipe + 2-digit mod-97
#[tokio::test]
async fn create_akun_generates_account_number_and_forces_saldo_zero() {
    let h = Harness::new();
    let (status, body) = h
        .post_json_auth(
            "/api/v1/akunkeuangan",
            r#"{"name":"Giro","description":"baru","accountType":7,"saldo":999.99}"#,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let v = json(&body);
    let acct = v["accountNumber"].as_str().unwrap();
    assert!(acct.starts_with("AKN01"), "accountNumber: {acct}");
    assert_eq!(acct.len(), 16, "accountNumber: {acct}");
    assert_eq!(&acct[11..14], "007", "tipe akun 3 digit: {acct}");
    // Konverter lama SELALU menimpa saldo jadi 0
    assert_eq!(v["saldo"], 0, "saldo harus dipaksa 0 (perilaku lama)");
    assert_eq!(v["id"], v["ident"]);
}

/// internalreference: "STMT" + yyMMddHHmmss + "_" + 8 + "_" + 8 = 34 char
#[tokio::test]
async fn create_transaksi_generates_internal_reference() {
    let h = Harness::new();
    let (status, body) = h
        .post_json_auth(
            "/api/v1/transaksikeuangan",
            r#"{"name":"Beli","idAkunKeuangan":1,"jenis":2,"nominal":150000.50}"#,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let v = json(&body);
    let r = v["internalreference"].as_str().unwrap();
    assert!(r.starts_with("STMT"), "internalreference: {r}");
    assert_eq!(r.len(), 34, "internalreference: {r}");
    assert_eq!(&r[16..17], "_");
    assert_eq!(&r[25..26], "_");
    // BigDecimal tetap angka dengan skala dipertahankan
    assert_eq!(v["nominal"], 150000.50);
}

/// IntegrationController: {"size": "<N>"} — String.valueOf → STRING, bukan angka
/// RECORDED: /api/v1/integration TIDAK cocok pola permitAll "/api/v1/integration/*".
#[tokio::test]
async fn integration_without_token_403() {
    let h = Harness::new();
    let (status, _, body) = h.get("/api/v1/integration").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.is_empty());
}

#[tokio::test]
async fn integration_returns_size_as_string() {
    let h = Harness::new();
    let (status, _, body) = h.get_auth("/api/v1/integration").await;
    assert_eq!(status, StatusCode::OK);
    let v = json(&body);
    assert!(v["size"].is_string(), "size harus string (String.valueOf): {v}");
}

// ---------------------------------------------------------------------------
// 7. Model serde — kontrak Jackson
// ---------------------------------------------------------------------------

#[test]
fn user_po_serializes_in_jackson_order() {
    let user = UserPo {
        id: 1,
        ident: 1,
        name: Some("Admin".into()),
        email: Some(TEST_EMAIL.into()),
        username: None,
        pwd_hash: Some("abc".into()),
        password: Some("def".into()),
        salt: Some("ghi".into()),
        created_date: None,
        updated_date: None,
        lifecycle: Default::default(),
    };
    let s = serde_json::to_string(&user).unwrap();
    // urutan field harus id, ident, name, email, username, pwdHash, password, salt,
    // createdDate, updatedDate, lalu lifecycle
    assert!(s.starts_with(r#"{"id":1,"ident":1,"name":"Admin""#), "{s}");
    assert!(s.contains(r#""pwdHash":"abc""#));
    // null tetap dikirim sebagai null (Jackson default), bukan dihilangkan
    assert!(s.contains(r#""username":null"#), "{s}");
}

// ---------------------------------------------------------------------------
// 8. Urutan field — dibandingkan langsung dengan rekaman sistem lama
// ---------------------------------------------------------------------------

fn golden_keys(name: &str) -> Vec<String> {
    let v = json(&golden(name));
    v.as_object()
        .unwrap_or_else(|| panic!("{name} bukan object"))
        .keys()
        .cloned()
        .collect()
}

fn response_keys(v: &Value) -> Vec<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

/// Key entity di dalam wrapper `{"data": {...}, "IdAkun": N}`.
fn golden_data_keys(name: &str) -> Vec<String> {
    let v = json(&golden(name));
    let entity = if v.get("data").is_some() { &v["data"] } else { &v };
    entity
        .as_object()
        .unwrap_or_else(|| panic!("{name} bukan object"))
        .keys()
        .cloned()
        .collect()
}

/// Kumpulan field HARUS sama. Urutan tidak diuji karena sistem lama sendiri
/// tidak deterministik: properti field mengikuti deklarasi, tetapi getter
/// mengikuti `Class.getDeclaredMethods()` yang berubah antar-restart JVM
/// (dibuktikan: rekaman 2026-10-17 vs 2026-10-18 berbeda urutan).
fn assert_same_field_set(actual: Vec<String>, expected: Vec<String>, label: &str) {
    let mut a = actual.clone();
    let mut e = expected.clone();
    a.sort();
    e.sort();
    assert_eq!(a, e, "{label}: set field berbeda");
}

/// User DTO contract from current Java Users/Base models (supersedes old PO fixture).
#[tokio::test]
async fn user_fields_match_recorded_contract() {
    let h = Harness::new();
    let (_, _, body) = h.get_auth("/api/v2/user/1").await;
    let actual = response_keys(&json(&body));
    let expected=["ident","needsConfirmation","pendingLedgerConfirmation","xmlIdentifier","valid",
        "name","email","username","pwdHash","password","salt","lifecycleEntity","permissionProfiles"];
    assert_same_field_set(actual,expected.iter().map(|s|s.to_string()).collect(),"Users");
}

/// AkunKeuanganPO
#[tokio::test]
async fn akun_fields_match_recorded_contract() {
    let h = Harness::new();
    let (_, _, body) = h.get_auth("/api/v1/akunkeuangan/1").await;
    let actual = response_keys(&json(&body));
    let expected = golden_keys("akun_detail");
    assert_same_field_set(actual.clone(), expected.clone(), "AkunKeuanganPO");
    // "ident" selalu field pertama, "entityIdent" selalu terakhir
    assert_eq!(actual.first().map(String::as_str), Some("ident"));
    assert_eq!(actual.last().map(String::as_str), Some("entityIdent"));
}

/// CategoryPO — urutan PENUH deterministik (tanpa getter boolean turunan)
#[tokio::test]
async fn category_field_order_matches_recorded_contract() {
    let h = Harness::new();
    let (_, body) = h
        .post_json_auth(
            "/api/v1/category",
            r#"{"category_name":"X","category_type":1,"akunKeuangan":{"id":1}}"#,
        )
        .await;
    let actual = response_keys(&json(&body));
    let expected = golden_data_keys("category_detail");
    assert_eq!(actual, expected, "\nrust   : {actual:?}\nlegacy : {expected:?}");
}

/// TransaksiKeuanganPO — urutan PENUH deterministik
#[tokio::test]
async fn transaksi_field_order_matches_recorded_contract() {
    let h = Harness::new();
    let (_, body) = h
        .post_json_auth(
            "/api/v1/transaksikeuangan",
            r#"{"name":"T","idAkunKeuangan":1,"jenis":2,"nominal":10.5}"#,
        )
        .await;
    let actual = response_keys(&json(&body));
    let expected = golden_data_keys("tk_detail");
    assert_eq!(actual, expected, "\nrust   : {actual:?}\nlegacy : {expected:?}");
}

/// Wrapper list: RECORDED {"data":[...],"IdAkun":N} — "data" lebih dulu.
#[tokio::test]
async fn list_wrapper_key_order_matches_recorded_contract() {
    let h = Harness::new();
    let (_, _, body) = h.get_auth("/api/v1/category/lists?idAkun=1").await;
    let keys = response_keys(&json(&body));
    assert_eq!(keys, vec!["data".to_string(), "IdAkun".to_string()], "{keys:?}");
}

/// Nested activationType / revisionState harus identik dengan rekaman.
#[tokio::test]
async fn nested_activation_and_revision_state_match_recorded_contract() {
    let h = Harness::new();
    let (_, _, body) = h.get_auth("/api/v1/akunkeuangan/1").await;
    let v = json(&body);
    let expected_act = json(&golden("akun_detail"))["activationType"].clone();
    let expected_rev = json(&golden("akun_detail"))["revisionState"].clone();
    // Urutan getter nested juga tidak stabil antar-restart JVM → bandingkan set
    assert_same_field_set(
        response_keys(&v["activationType"]),
        response_keys(&expected_act),
        "activationType",
    );
    assert_same_field_set(
        response_keys(&v["revisionState"]),
        response_keys(&expected_rev),
        "revisionState",
    );
}
