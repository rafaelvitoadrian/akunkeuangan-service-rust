//! Requires an explicitly provided, disposable PostgreSQL test database.
//! Each test creates and removes only its own randomly named schema.
#![cfg(feature = "postgres")]

use akunkeuangan_service::{
    auth,
    db::{postgres_impl::PostgresRepo, Repository},
    models::*,
    router, AppState,
};
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::{postgres::PgConnectOptions, PgPool};
use std::{str::FromStr, sync::Arc};
use tower::ServiceExt;

struct TestDb {
    admin: PgPool,
    schema: String,
    options: PgConnectOptions,
}
impl TestDb {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL")
            .expect("set TEST_DATABASE_URL to a disposable PostgreSQL database");
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("akun_test_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let options = PgConnectOptions::from_str(&url)
            .unwrap()
            .options([("search_path", schema.as_str())]);
        Self {
            admin,
            schema,
            options,
        }
    }
    async fn repo(&self) -> PostgresRepo {
        PostgresRepo::connect_with_options(self.options.clone(), true)
            .await
            .unwrap()
    }
    async fn pool(&self) -> PgPool {
        PgPool::connect_with(self.options.clone()).await.unwrap()
    }
    async fn finish(self) {
        // Only a generated schema is removed; never drop the test database.
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn migrations_are_concurrent_repeatable_preserve_data_and_reject_checksum_changes() {
    let db = TestDb::new().await;
    assert!(
        PostgresRepo::connect_with_options(db.options.clone(), false)
            .await
            .is_err()
    );
    let pool = db.pool().await;
    sqlx::query("CREATE TABLE systemprop(propname TEXT PRIMARY KEY,propvalue TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO systemprop VALUES('is.test','true'),('existing','keep me')")
        .execute(&pool)
        .await
        .unwrap();
    let (first, second) = tokio::join!(db.repo(), db.repo());
    assert!(first.get_system_prop_bool("is.test").await.unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT propvalue FROM systemprop WHERE propname='existing'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        "keep me"
    );
    let (a, b) = tokio::join!(
        first.bootstrap_user("admin@example.test", "initial"),
        second.bootstrap_user("admin@example.test", "initial")
    );
    assert_ne!(a.unwrap(), b.unwrap());
    let initial = first.fetch_all_users().await.unwrap();
    assert_eq!(initial.len(), 1);
    assert!(!second
        .bootstrap_user("admin@example.test", "replacement")
        .await
        .unwrap());
    let third = db.repo().await;
    assert_eq!(third.fetch_all_users().await.unwrap(), initial);
    assert!(third
        .authenticate_user_v2("admin@example.test", "initial")
        .await
        .unwrap());
    assert!(!third
        .authenticate_user_v2("admin@example.test", "replacement")
        .await
        .unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    sqlx::query("UPDATE _sqlx_migrations SET checksum=decode('00','hex')")
        .execute(&pool)
        .await
        .unwrap();
    assert!(PostgresRepo::connect_with_options(db.options.clone(), true)
        .await
        .is_err());
    pool.close().await;
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn failed_schema_migration_rolls_back_without_replacing_existing_tables() {
    let db = TestDb::new().await;
    let pool = db.pool().await;
    sqlx::query("CREATE TABLE users(id BIGINT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users VALUES(99)")
        .execute(&pool)
        .await
        .unwrap();
    assert!(PostgresRepo::connect_with_options(db.options.clone(), true)
        .await
        .is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT id FROM users")
            .fetch_one(&pool)
            .await
            .unwrap(),
        99
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>("SELECT to_regclass('activationtype')::text")
            .fetch_one(&pool)
            .await
            .unwrap(),
        None
    );
    pool.close().await;
    db.finish().await;
}

fn account(name: &str, owner: i64, id: i64, activation: i64) -> AkunKeuanganRequest {
    AkunKeuanganRequest {
        name: Some(name.into()),
        account_type: Some(1),
        actor_ident: Some(owner),
        // Top-level ident is intentionally unrelated: the lifecycle identity wins.
        ident: 999999,
        lifecycle_entity: Some(json!({"identEntity":id,"identActivationType":activation})),
        ..Default::default()
    }
}

/// The benchmark owns a disposable schema and uses synthetic data only.
/// BENCH_TRANSACTION_COUNT allows a small before/after comparison; default is
/// the real 100k-row workload, never a production database.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn large_transaction_list_preserves_complete_legacy_response() {
    let db = TestDb::new().await;
    let repo = Arc::new(db.repo().await);
    let pool = db.pool().await;
    repo.bootstrap_user("list@example.test", "isolated").await.unwrap();
    let owner = repo.resolve_actor_ident("list@example.test").await.unwrap().unwrap();
    let account_value = repo.create_akun_keuangan(account("list", owner, 0, 1), "list@example.test").await.unwrap();
    let account_id = account_value["id"].as_i64().unwrap();
    // Related categories can point to another account. The response must use
    // that account, not replace it with the transaction's account.
    let related = repo.create_akun_keuangan(account("related", owner, 0, 1), "list@example.test").await.unwrap();
    let related_id = related["id"].as_i64().unwrap();
    sqlx::query("INSERT INTO category(id,category_name,category_type,id_akuntransaksi,createddate) SELECT n,'category '||n,1,CASE WHEN n%2=0 THEN $1 ELSE $2 END,TIMESTAMP '2026-10-15 01:38:00.822745' FROM generate_series(1,20) n")
        .bind(account_id).bind(related_id).execute(&pool).await.unwrap();
    let count = std::env::var("BENCH_TRANSACTION_COUNT").ok().map(|v|v.parse::<i64>().unwrap()).unwrap_or(100_000);
    assert!(count >= 30);
    sqlx::query("INSERT INTO transaksikeuangan(id,name,description,id_akuntransaksi,category_id,jenis,nominal,createddate,updateddate,transaksi_date,externalreference,internalreference) SELECT n,'unicode 雪 / '||n,E'quote \" slash \\ newline\\n', $1, CASE WHEN n%3=0 THEN NULL ELSE 1+n%20 END,1,12345678901234567890.123456789,TIMESTAMP '2026-10-15 07:00:00',NULL,TIMESTAMP '2026-10-15 12:00:00'+n*INTERVAL '1 second',NULL,'REF-'||n FROM generate_series(1,$2) n")
        .bind(account_id).bind(count).execute(&pool).await.unwrap();
    sqlx::query("ANALYZE transaksikeuangan").execute(&pool).await.unwrap();
    let start = std::time::Instant::now();
    let bytes = repo.fetch_transaksi_list_json(account_id, "list@example.test").await.unwrap();
    let elapsed = start.elapsed();
    #[derive(serde::Deserialize)]
    struct Envelope {
        #[serde(rename="IdAkun")]
        account: i64,
        data: Vec<Summary>,
    }
    #[derive(serde::Deserialize)]
    struct Summary { id: i64, nominal: serde_json::Number }
    let response: Envelope = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response.account, account_id);
    assert_eq!(response.data.len(), count as usize); // No hidden limit/pagination.
    assert_eq!(response.data.first().unwrap().id, count);
    assert_eq!(response.data.last().unwrap().id, 1);
    assert!(response.data.windows(2).all(|w| w[0].id > w[1].id));
    assert!(response.data.iter().all(|v| v.nominal.to_string()=="12345678901234567890.123456789"));
    println!("transaction_list rows={count} bytes={} elapsed_ms={}", bytes.len(), elapsed.as_millis());
    let app = router(Arc::new(AppState {repo:repo.clone(),security_key:auth::SecurityKey::Test}));
    let http_start = std::time::Instant::now();
    let compressed = app.clone().oneshot(Request::builder().uri(format!("/api/v1/transaksikeuangan/lists?idAkun={account_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", signed("list@example.test")))
        .header(header::ACCEPT_ENCODING,"gzip").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(compressed.status(), StatusCode::OK);
    assert_eq!(compressed.headers()[header::CONTENT_ENCODING], "gzip");
    let compressed = compressed.into_body().collect().await.unwrap().to_bytes();
    let http_elapsed = http_start.elapsed();
    use std::io::Read;
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(compressed.as_ref()).read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, bytes); // Same entire JSON, including field order/nulls.
    println!("transaction_http_gzip rows={count} wire_bytes={} elapsed_ms={}", compressed.len(), http_elapsed.as_millis());
    drop(decoded);
    drop(response);
    drop(bytes);

    // Compare every byte for a small fixture against the old full Value tree,
    // and check nested/null categories against independent detail reads.
    sqlx::query("DELETE FROM transaksikeuangan WHERE id>30").execute(&pool).await.unwrap();
    let legacy = repo.fetch_transaksi_by_akun(account_id, "list@example.test").await.unwrap();
    let encoded = repo.fetch_transaksi_list_json(account_id, "list@example.test").await.unwrap();
    assert_eq!(encoded, serde_json::to_vec(&json!({"data":legacy,"IdAkun":account_id})).unwrap());
    let response = app.clone().oneshot(Request::builder().uri(format!("/api/v1/transaksikeuangan/lists?idAkun={account_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", signed("list@example.test"))).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
    assert_eq!(response.into_body().collect().await.unwrap().to_bytes().as_ref(), encoded.as_slice());
    let response = app.oneshot(Request::builder().uri(format!("/api/v1/transaksikeuangan/lists?idAkun={account_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", signed("list@example.test")))
        .header(header::ACCEPT_ENCODING,"gzip;q=0").body(Body::empty()).unwrap()).await.unwrap();
    assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
    assert_eq!(response.into_body().collect().await.unwrap().to_bytes().as_ref(), encoded.as_slice());
    for transaction in &legacy {
        let detail = repo.fetch_transaksi_detail(account_id, transaction["id"].as_i64().unwrap(), "list@example.test").await.unwrap().unwrap();
        assert_eq!(transaction, &detail);
        if !transaction["category"].is_null() {
            let category_id = transaction["category"]["id"].as_i64().unwrap();
            assert_eq!(transaction["category"]["akunKeuangan"]["id"], if category_id%2==0 {account_id} else {related_id});
        }
    }
    assert_eq!(repo.fetch_transaksi_list_json(account_id, "stranger@example.test").await.unwrap(), serde_json::to_vec(&json!({"data":[],"IdAkun":account_id})).unwrap());
    // Changing category/account data must be visible immediately (no cache
    // shared across requests), including the historical null-activation error.
    sqlx::query("UPDATE category SET category_name='updated' WHERE id=1").execute(&pool).await.unwrap();
    let updated: Value = serde_json::from_slice(&repo.fetch_transaksi_list_json(account_id, "list@example.test").await.unwrap()).unwrap();
    assert!(updated["data"].as_array().unwrap().iter().filter(|v|v["category"]["id"]==1).all(|v|v["category"]["category_name"]=="updated"));
    sqlx::query("UPDATE akunkeuangan SET ident_activationtype=NULL WHERE id=$1").bind(related_id).execute(&pool).await.unwrap();
    assert!(repo.fetch_transaksi_list_json(account_id, "list@example.test").await.is_err());
    pool.close().await;
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn postgres_lifecycle_relations_precision_passwords_and_rollback() {
    let db = TestDb::new().await;
    let repo = db.repo().await;
    let pool = db.pool().await;
    repo.bootstrap_user("owner@example.test", "initial")
        .await
        .unwrap();
    let owner = repo
        .resolve_actor_ident("owner@example.test")
        .await
        .unwrap()
        .unwrap();
    let created = repo
        .create_akun_keuangan(account("first", owner, 0, 1), "owner@example.test")
        .await
        .unwrap();
    let master = created["id"].as_i64().unwrap();
    let old_revision = created["identRevision"].as_i64().unwrap();
    assert_eq!(created["identActivationType"], 0);
    let fetched = repo.fetch_akun_by_id(master).await.unwrap().unwrap();
    assert_eq!(fetched["identActivationType"], 1);
    assert_eq!(
        repo.fetch_akun_by_id(old_revision).await.unwrap().unwrap()["identMaster"],
        master
    );
    let updated = repo
        .update_akun_keuangan(account("suspended", owner, master, 4))
        .await
        .unwrap();
    assert_eq!(updated["id"], master);
    assert_eq!(updated["activationType"]["suspended"], true);
    assert_ne!(updated["identRevision"], old_revision);
    assert!(
        repo.fetch_akun_by_id(old_revision).await.unwrap().unwrap()["revisionEffectiveTo"]
            .is_string()
    );
    assert_eq!(updated["accountNumber"], created["accountNumber"]);

    let second = repo
        .create_akun_keuangan(account("second", owner, 0, 1), "owner@example.test")
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let cat = repo
        .create_category(
            serde_json::from_value(
                json!({"category_name":"income","category_type":1,"akunKeuangan":{"id":master}}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let cat_id = cat["id"].as_i64().unwrap();
    assert!(cat["akunKeuangan"]["name"].is_null()); // transient relation stub on create
    assert_eq!(
        repo.fetch_category_detail(master, cat_id, "owner@example.test")
            .await
            .unwrap()
            .unwrap()["akunKeuangan"]["name"],
        "suspended"
    );
    assert!(repo
        .fetch_categories(master, "someone-else@example.test")
        .await
        .unwrap()
        .is_empty());
    assert!(repo
        .fetch_category_detail(master, cat_id, "someone-else@example.test")
        .await
        .unwrap()
        .is_none());
    let original_created = cat["createdDate"].clone();
    repo.update_category(
        serde_json::from_value(
            json!({"id":cat_id,"category_name":"moved","akunKeuangan":{"id":second}}),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    assert!(repo
        .fetch_category_detail(master, cat_id, "owner@example.test")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.fetch_category_detail(second, cat_id, "owner@example.test")
            .await
            .unwrap()
            .unwrap()["createdDate"],
        original_created
    );

    let amount = "123456789012345678901234567890.123456789";
    let mut request: TransaksiKeuanganRequest = serde_json::from_str(&format!(r#"{{"name":"amount","idAkunKeuangan":{second},"category":{{"id":{cat_id}}},"nominal":{amount},"jenis":1,"transactionDate":"2026-10-18T12:00:00","externalreference":"external"}}"#)).unwrap();
    let transaction = repo.create_transaksi(request.clone()).await.unwrap();
    let tk_id = transaction["id"].as_i64().unwrap();
    let detail = repo
        .fetch_transaksi_detail(second, tk_id, "owner@example.test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail["nominal"].to_string(), amount);
    assert_eq!(detail["transactionDate"], "2026-10-18T12:00:00");
    assert_eq!(detail["category"]["akunKeuangan"]["id"], second);
    assert!(repo
        .fetch_transaksi_detail(second, tk_id, "someone-else@example.test")
        .await
        .unwrap()
        .is_none());
    request.id = tk_id;
    request.name = Some("changed".into());
    request.id_akun_keuangan = Some(master);
    request.category = None;
    request.externalreference = Some("must stay external".into());
    let changed = repo.update_transaksi(request).await.unwrap();
    assert!(changed["category"].is_null());
    assert_eq!(
        changed["internalreference"],
        transaction["internalreference"]
    );
    assert_eq!(changed["externalreference"], "external");
    assert!(repo
        .fetch_transaksi_by_akun(second, "owner@example.test")
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        repo.fetch_transaksi_by_akun(master, "owner@example.test")
            .await
            .unwrap()
            .len(),
        1
    );

    // Invalid relationship rejects the whole write; the prior row is intact.
    let failed: TransaksiKeuanganRequest = serde_json::from_value(
        json!({"id":tk_id,"name":"should rollback","category":{"id":9223372036854770000_i64}}),
    )
    .unwrap();
    assert!(repo.update_transaksi(failed).await.is_err());
    assert_eq!(
        repo.fetch_transaksi_detail(master, tk_id, "owner@example.test")
            .await
            .unwrap()
            .unwrap()["name"],
        "changed"
    );
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM akunkeuangan")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(repo
        .create_akun_keuangan(account("bad actor", 9223372036854770000, 0, 1), "")
        .await
        .is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM akunkeuangan")
            .fetch_one(&pool)
            .await
            .unwrap(),
        before
    );

    // An update addressed to a revision retains that revision's ID.
    let revision = repo.fetch_all_users().await.unwrap()[0]["identRevision"]
        .as_i64()
        .unwrap();
    let req: UsersRequest = serde_json::from_value(json!({"name":"changed owner","email":"owner@example.test","username":"owner@example.test","password":"revision-password", "lifecycleEntity":{"identEntity":revision,"activationType":{"identActivationType":1}}})).unwrap();
    assert_eq!(repo.update_user(req).await.unwrap()["id"], revision);
    assert!(repo
        .authenticate_user_v2("owner@example.test", "initial")
        .await
        .unwrap());
    // A password change must work with the plaintext password on both login paths.
    repo.change_password(owner, "new-password").await.unwrap();
    assert!(repo
        .authenticate_user_v2("owner@example.test", "new-password")
        .await
        .unwrap());
    assert!(!repo
        .authenticate_user_v2(
            "owner@example.test",
            &auth::hash_password_sha1("new-password")
        )
        .await
        .unwrap());
    assert!(repo
        .authenticate_user_by_email("owner@example.test", "new-password")
        .await
        .unwrap());
    assert!(repo.delete_user(owner).await.is_err());
    // Pending Ready revisions can be soft-deleted, effective revisions cannot.
    sqlx::query(
        "UPDATE users SET revision_effective_from=NULL,revision_effective_to=NULL WHERE id=$1",
    )
    .bind(revision)
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo.delete_user(revision).await.unwrap());
    assert_eq!(
        repo.fetch_user_by_id(revision).await.unwrap().unwrap()["revisionState"]["deleted"],
        true
    );
    pool.close().await;
    db.finish().await;
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
    media: &str,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, media);
    if let Some(token) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let data = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&data).unwrap_or(Value::Null))
}

fn signed(email: &str) -> String {
    auth::generate_token(
        email,
        "http://localhost",
        "http://localhost/api/v1/user/loginv2",
        &auth::SecurityKey::Test,
        chrono::Utc::now().timestamp(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn current_user_roles_profiles_and_frontend_forms_work_without_frontend_changes() {
    let db = TestDb::new().await;
    let repo = Arc::new(db.repo().await);
    repo.bootstrap_user("admin@example.test", "password")
        .await
        .unwrap();
    let app = router(Arc::new(AppState {
        repo: repo.clone(),
        security_key: auth::SecurityKey::Test,
    }));
    let admin = signed("admin@example.test");
    let form = "application/x-www-form-urlencoded";
    let jm = "application/json";
    let base = "/api/v2/user";
    for endpoint in [
        "fetchUserRoles",
        "fetchPoAUserRoles",
        "getAllPermissionFunctions",
        "getAuthLevels",
        "getAccessLevels",
        "fetchAuthorizedPermissionProfiles",
        "getUserRoles/*",
    ] {
        let (status, body) = send(
            &app,
            "GET",
            &format!("{base}/{endpoint}"),
            Some(&admin),
            "",
            form,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{endpoint}: {body}");
        assert!(body.is_array());
        assert_eq!(
            send(&app, "GET", &format!("{base}/{endpoint}"), None, "", form)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    let (_, catalog) = send(
        &app,
        "GET",
        &format!("{base}/getAllPermissionFunctions"),
        Some(&admin),
        "",
        form,
    )
    .await;
    let function = catalog[0]["function"]["identFunction"].as_i64().unwrap();
    let role_form=format!("shortname=FIN_VIEW&name=Finance+view&activationStatus=Active&applicableForPOA=true&userRoleFunctions[0].function.identFunction={function}&userRoleFunctions[0].accessLevel.identAccessLevel=2");
    let (status, role) = send(
        &app,
        "POST",
        &format!("{base}/create-user-role"),
        Some(&admin),
        &role_form,
        form,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{role}");
    let role_id = role["identUserRole"].as_i64().unwrap();
    assert_eq!(role["name"], "Finance view");
    assert_eq!(role["applicableForPOA"], true);
    assert_eq!(role["userRoleFunctions"][0]["ident"], 0);
    assert_eq!(role["userRoleFunctions"][0]["accessLevel"]["view"], true);
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{base}/create-user-role"),
            Some(&admin),
            &role_form.replace("FIN_VIEW", "fin_view"),
            form
        )
        .await
        .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    for bad in [
        "shortname=x&activationStatus=invalid",
        "shortname=x&userRoleFunctions[256].function.identFunction=1",
        "shortname=x&userRoleFunctions[0].accessLevel.identAccessLevel=oops",
    ] {
        assert_eq!(
            send(
                &app,
                "POST",
                &format!("{base}/create-user-role"),
                Some(&admin),
                bad,
                form
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status, profile) = send(
        &app,
        "POST",
        &format!("{base}/create-permission-profile"),
        Some(&admin),
        &format!("name=Finance+profile&userRoles[0].xmlIdentifier={role_id}"),
        form,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{profile}");
    let profile_id = profile["identPermissionProfile"].as_i64().unwrap();
    let body = json!({"name":"Viewer","email":"viewer@example.test","username":"viewer","password":"initial",
        "permissionProfiles":[{"xmlIdentifier":profile_id.to_string()}],"lifecycleEntity":{"activationType":{"identActivationType":1}}});
    let (status, user) = send(&app, "POST", base, Some(&admin), &body.to_string(), jm).await;
    assert_eq!(status, StatusCode::OK, "{user}");
    let user_id = user["ident"].as_i64().unwrap();
    assert_eq!(user["lifecycleEntity"]["identEntity"], user_id);
    assert_eq!(
        user["permissionProfiles"][0]["userRoles"][0]["identUserRole"],
        role_id
    );
    assert_eq!(
        user["lifecycleEntity"]["lastUpdateUser"]["email"],
        "admin@example.test"
    );
    let viewer = signed("viewer@example.test");
    let (status, users) = send(&app, "GET", base, Some(&viewer), "", jm).await;
    assert_eq!(status, StatusCode::OK, "{users}");
    assert!(users
        .as_array()
        .unwrap()
        .iter()
        .any(|u| u["ident"] == user_id));
    // Batched list hydration must retain the complete nested DTO, including
    // actor lifecycle, ordered functions/access levels and profile roles.
    let individual = repo.user_model(repo.fetch_user_by_id(user_id).await.unwrap().unwrap())
        .await.unwrap();
    assert_eq!(users.as_array().unwrap().iter().find(|u|u["ident"]==user_id).unwrap(), &individual);
    let (status, denied) = send(&app, "POST", base, Some(&viewer), &body.to_string(), jm).await;
    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
    assert_eq!(
        denied,
        json!({"response":"User have insufficient accesslevel for function 'UserAccount'."})
    );
    let before = repo.fetch_user_by_id(user_id).await.unwrap().unwrap();
    let update = json!({"name":"Renamed viewer","email":"viewer@example.test","username":"viewer",
        "permissionProfiles":[{"identPermissionProfile":profile_id}],
        "lifecycleEntity":{"identEntity":user_id,"identRevision":99999999,"activationType":{"identActivationType":1}}});
    let (status, changed) = send(&app, "PUT", base, Some(&admin), &update.to_string(), jm).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["ident"], user_id);
    assert_eq!(changed["name"], "Renamed viewer");
    assert_ne!(
        changed["lifecycleEntity"]["identRevision"],
        before["identRevision"]
    );
    assert_ne!(changed["lifecycleEntity"]["identRevision"], 99999999);
    let after = repo.fetch_user_by_id(user_id).await.unwrap().unwrap();
    for key in ["password", "pwdHash", "salt", "createdDate"] {
        assert_eq!(after[key], before[key], "{key}");
    }
    assert!(after["updatedDate"].is_string());
    for login in [false, true] {
        let ok = if login {
            repo.authenticate_user_v2("viewer@example.test", "initial")
                .await
                .unwrap()
        } else {
            repo.authenticate_user_by_email("viewer@example.test", "initial")
                .await
                .unwrap()
        };
        assert!(ok);
    }
    let current_revision = after["identRevision"].as_i64().unwrap();
    let revision = repo
        .user_model(
            repo.fetch_user_by_id(current_revision)
                .await
                .unwrap()
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        revision["permissionProfiles"][0]["identPermissionProfile"],
        profile_id
    );
    let mut bad = update.clone();
    bad["name"] = json!("Must rollback");
    bad["permissionProfiles"] = json!([{"identPermissionProfile":9223372036854770000_i64}]);
    assert_eq!(
        send(&app, "PUT", base, Some(&admin), &bad.to_string(), jm)
            .await
            .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        repo.fetch_user_by_id(user_id).await.unwrap().unwrap(),
        after
    );
    let role_update=format!("identUserRole={role_id}&{role_form}&description=Updated&userRoleFunctions[0].accessLevel.identAccessLevel=4");
    let (status, role_changed) = send(
        &app,
        "PUT",
        &format!("{base}/update-user-role"),
        Some(&admin),
        &role_update,
        form,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{role_changed}");
    assert_eq!(
        role_changed["userRoleFunctions"][0]["accessLevel"]["approve"],
        true
    );
    repo.require_function("viewer@example.test", "UserAccount", 4)
        .await
        .unwrap();
    let profile_update=format!("identPermissionProfile={profile_id}&name=Updated+profile&userRoles[0].identUserRole={role_id}");
    let (status, changed_profile) = send(
        &app,
        "PUT",
        &format!("{base}/update-permission-profile"),
        Some(&admin),
        &profile_update,
        form,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed_profile}");
    assert_eq!(changed_profile["name"], "Updated profile");
    let password = json!({"identUser":user_id,"newPassword":"changed-password"});
    let (status, response) = send(
        &app,
        "POST",
        &format!("{base}/change-password"),
        Some(&admin),
        &password.to_string(),
        jm,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert!(repo
        .authenticate_user_v2("viewer@example.test", "changed-password")
        .await
        .unwrap());
    assert!(repo
        .authenticate_user_by_email("viewer@example.test", "changed-password")
        .await
        .unwrap());
    assert!(!repo
        .authenticate_user_v2("viewer@example.test", "initial")
        .await
        .unwrap());
    let (_, fetched) = send(
        &app,
        "GET",
        &format!("{base}/{user_id}"),
        Some(&admin),
        "",
        jm,
    )
    .await;
    assert_eq!(
        fetched["permissionProfiles"][0]["identPermissionProfile"],
        profile_id
    );
    let (status, deactivated) = send(
        &app,
        "POST",
        &format!("{base}/deactivate-permission-profile"),
        Some(&admin),
        &profile_update,
        form,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deactivated}");
    assert_eq!(
        deactivated["deactivationDate"],
        akunkeuangan_service::util::app_now().date().to_string()
    );
    // Referenced role deletion rolls back; an independent role can be removed.
    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("{base}/delete-user-role"),
            Some(&admin),
            &format!("identUserRole={role_id}"),
            form
        )
        .await
        .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let (_, unused) = send(
        &app,
        "POST",
        &format!("{base}/create-user-role"),
        Some(&admin),
        "shortname=UNUSED&name=Unused&activationStatus=Active",
        form,
    )
    .await;
    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("{base}/delete-user-role"),
            Some(&admin),
            &format!("identUserRole={}", unused["identUserRole"]),
            form
        )
        .await,
        (StatusCode::OK, Value::Null)
    );
    let (_, roles) = send(
        &app,
        "GET",
        &format!("{base}/fetchUserRoles"),
        Some(&admin),
        "",
        form,
    )
    .await;
    let internal = roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["internal"] == true)
        .unwrap();
    assert_eq!(
        send(
            &app,
            "PUT",
            &format!("{base}/update-user-role"),
            Some(&admin),
            &format!(
                "identUserRole={}&shortname=HACK&internal=false",
                internal["identUserRole"]
            ),
            form
        )
        .await
        .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("{base}/delete-user-role"),
            Some(&admin),
            &format!("identUserRole={}", internal["identUserRole"]),
            form
        )
        .await
        .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    drop(app);
    drop(repo);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn individual_permission_profiles_are_owned_cloned_and_rolled_back_atomically() {
    let db = TestDb::new().await;
    let repo = db.repo().await;
    repo.bootstrap_user("admin@example.test", "password")
        .await
        .unwrap();
    let mut req:UsersRequest=serde_json::from_value(json!({"name":"Individual","email":"individual@example.test","username":"individual","password":"secret",
        "permissionProfiles":[{"name":"ignored","userRoles":[]}],"lifecycleEntity":{"activationType":{"identActivationType":1}}})).unwrap();
    let created = repo.create_user_model(req.clone()).await.unwrap();
    let id = created["ident"].as_i64().unwrap();
    let profile = &created["permissionProfiles"][0];
    assert_eq!(profile["name"], "AKN/INDIVIDUAL");
    assert_eq!(profile["individual"], true);
    assert_eq!(profile["userAccount"]["ident"], id);
    req.password = None;
    req.lifecycle_entity =
        Some(json!({"identEntity":id,"activationType":{"identActivationType":1}}));
    req.permission_profiles =
        Some(serde_json::from_value(created["permissionProfiles"].clone()).unwrap());
    req.username = Some("renamed-login".into());
    let requested_profile = &mut req.permission_profiles.as_mut().unwrap()[0];
    requested_profile.name = Some("AKN/FINANCE_CUSTOM".into());
    requested_profile.deactivation_date =
        Some(chrono::NaiveDate::from_ymd_opt(2030, 1, 2).unwrap());
    let updated = repo.update_user_model(req.clone()).await.unwrap();
    assert_ne!(
        updated["permissionProfiles"][0]["identPermissionProfile"],
        profile["identPermissionProfile"]
    );
    assert_eq!(updated["permissionProfiles"][0]["userAccount"]["ident"], id);
    assert_eq!(
        updated["permissionProfiles"][0]["name"],
        "AKN/FINANCE_CUSTOM"
    );
    assert_eq!(
        updated["permissionProfiles"][0]["deactivationDate"],
        "2030-01-02"
    );
    // Java only derives AKN/USERNAME for a new individual profile. An existing
    // profile's name/roles/date survive cloning even when username is absent.
    let mut without_username = req.clone();
    without_username.username = None;
    without_username.permission_profiles =
        Some(serde_json::from_value(updated["permissionProfiles"].clone()).unwrap());
    let updated = repo.update_user_model(without_username).await.unwrap();
    assert_eq!(updated["permissionProfiles"].as_array().unwrap().len(), 1);
    assert_eq!(
        updated["permissionProfiles"][0]["name"],
        "AKN/FINANCE_CUSTOM"
    );
    assert_eq!(
        updated["permissionProfiles"][0]["deactivationDate"],
        "2030-01-02"
    );
    assert!(repo
        .authenticate_user_v2("individual@example.test", "secret")
        .await
        .unwrap());
    let pool = db.pool().await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM permissionprofile")
        .fetch_one(&pool)
        .await
        .unwrap();
    req.permission_profiles = Some(
        serde_json::from_value(json!([{"userRoles":[{"identUserRole":9223372036854770000_i64}]}]))
            .unwrap(),
    );
    assert!(repo.update_user_model(req).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM permissionprofile")
            .fetch_one(&pool)
            .await
            .unwrap(),
        count
    );
    let existing = repo
        .user_model(repo.fetch_user_by_id(id).await.unwrap().unwrap())
        .await
        .unwrap();
    assert_eq!(
        existing["permissionProfiles"],
        updated["permissionProfiles"]
    );
    assert!(repo
        .require_function("individual@example.test", "UserAccount", 2)
        .await
        .is_err());
    repo.grant_security_admin("individual@example.test")
        .await
        .unwrap();
    repo.grant_security_admin("individual@example.test")
        .await
        .unwrap();
    repo.require_function("individual@example.test", "UserAccount", 4)
        .await
        .unwrap();
    pool.close().await;
    drop(repo);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn security_upgrade_preserves_imported_credentials_and_requires_explicit_admin_grant() {
    let db = TestDb::new().await;
    let pool = db.pool().await;
    let mut old = sqlx::migrate!("./migrations/postgres");
    old.migrations = std::borrow::Cow::Owned(old.iter().take(1).cloned().collect());
    old.run(&pool).await.unwrap();
    let salt = auth::new_password_salt();
    let sha1 = auth::hash_password_sha1("imported-password");
    let sha256 = auth::hash_password_sha256(&salt, "imported-password");
    sqlx::query("INSERT INTO users(id,name,email,username,password,pwd,salt,ident_activationtype,ident_revisionstate) VALUES(42,'Imported','imported@example.test','imported',$1,$2,$3,1,1)")
        .bind(&sha1).bind(&sha256).bind(&salt).execute(&pool).await.unwrap();
    let repo = db.repo().await;
    assert!(repo
        .authenticate_user_by_email("imported@example.test", "imported-password")
        .await
        .unwrap());
    assert!(repo
        .authenticate_user_v2("imported@example.test", "imported-password")
        .await
        .unwrap());
    assert!(repo
        .require_function("imported@example.test", "UserAccount", 2)
        .await
        .is_err());
    assert!(repo
        .grant_security_admin("missing@example.test")
        .await
        .is_err());
    repo.grant_security_admin("imported@example.test")
        .await
        .unwrap();
    repo.grant_security_admin("imported@example.test")
        .await
        .unwrap();
    repo.require_function("imported@example.test", "UserAccount", 4)
        .await
        .unwrap();
    let (password, pwd, actual_salt): (String, String, String) =
        sqlx::query_as("SELECT password,pwd,salt FROM users WHERE id=42")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((password, pwd, actual_salt), (sha1, sha256, salt));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM permissionprofile WHERE user_id=42")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    let restarted = db.repo().await;
    restarted
        .require_function("imported@example.test", "UserAccount", 4)
        .await
        .unwrap();
    let model = restarted
        .user_model(restarted.fetch_user_by_id(42).await.unwrap().unwrap())
        .await
        .unwrap();
    assert_eq!(model["ident"], 42);
    assert_eq!(model["permissionProfiles"].as_array().unwrap().len(), 1);
    pool.close().await;
    drop(repo);
    drop(restarted);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn postgres_http_contract_auth_validation_and_frontend_payloads() {
    let db = TestDb::new().await;
    let repo = Arc::new(db.repo().await);
    repo.bootstrap_user("admin@example.test", "password")
        .await
        .unwrap();
    let app = router(Arc::new(AppState {
        repo: repo.clone(),
        security_key: auth::SecurityKey::Test,
    }));
    let form = "application/x-www-form-urlencoded";
    let json_media = "application/json";
    assert_eq!(
        send(
            &app,
            "GET",
            "/health/ready",
            Some("invalid"),
            "",
            json_media
        )
        .await,
        (StatusCode::OK, json!({"status":"UP"}))
    );
    assert_eq!(
        send(&app, "GET", "/api/v2/user", None, "", json_media)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "GET", "/api/v2/user", Some("invalid"), "", json_media)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/v1/user/loginv2",
            None,
            r#"{"email":"admin@example.test","password":"password"}"#,
            json_media
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/v1/user/loginv2",
            None,
            "email=admin%40example.test&password=wrong",
            form
        )
        .await,
        (
            StatusCode::BAD_REQUEST,
            json!({"response":"Invalid user name/password"})
        )
    );
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/v1/user/login",
            None,
            "email=admin%40example.test&password=wrong",
            form
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, login) = send(
        &app,
        "POST",
        "/api/v1/user/loginv2",
        None,
        "email=admin%40example.test&password=password",
        form,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = login["token"].as_str().unwrap();
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/v1/user/login?email=admin%40example.test&password=password",
            None,
            "",
            form
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, validation) = send(
        &app,
        "POST",
        "/api/v1/akunkeuangan",
        Some(token),
        r#"{"name":" ","accountType":1,"lifecycleEntity":{"identActivationType":1}}"#,
        json_media,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(validation, json!({"response":"'Name' is mandatory."}));
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/v1/akunkeuangan",
            Some(token),
            r#"{"lifecycleEntity":[]}"#,
            json_media
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status,created) = send(&app,"POST","/api/v1/akunkeuangan",Some(token),r#"{"ident":9988,"name":"Frontend account","accountType":"1","lifecycleEntity":{"identActivationType":"1"}}"#,json_media).await;
    assert_eq!(status, StatusCode::OK);
    let id = created["id"].as_i64().unwrap();
    let (_, fetched) = send(
        &app,
        "GET",
        &format!("/api/v1/akunkeuangan/{id}"),
        Some(token),
        "",
        json_media,
    )
    .await;
    let owner = repo
        .resolve_actor_ident("admin@example.test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched["idUser"], owner);
    assert_eq!(fetched["activationType"]["active"], true);
    assert_eq!(
        created.as_object().unwrap().keys().collect::<Vec<_>>(),
        fetched.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    assert_eq!(created["id"], created["entityIdent"]);
    let body = json!({"ident":999999,"name":"Updated frontend account","lifecycleEntity":{"identEntity":id,"activationType":{"identActivationType":4}}}).to_string();
    let (status, updated) = send(
        &app,
        "PUT",
        "/api/v1/akunkeuangan",
        Some(token),
        &body,
        json_media,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["id"], id);
    assert_eq!(updated["idUser"], owner);
    assert_eq!(updated["activationType"]["suspended"], true);
    assert_eq!(
        send(
            &app,
            "GET",
            "/api/v2/user/9223372036854770000",
            Some(token),
            "",
            json_media
        )
        .await,
        (StatusCode::OK, Value::Null)
    );
    // A broken DB must fail readiness without revealing SQL/schema details.
    let pool = db.pool().await;
    sqlx::query("DROP TABLE systemprop")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        send(&app, "GET", "/health/ready", None, "", json_media).await,
        (StatusCode::SERVICE_UNAVAILABLE, json!({"status":"DOWN"}))
    );
    pool.close().await;
    db.finish().await;
}
