use akunkeuangan_service::{auth, lifecycle::LifecycleRequest, models::*, util::jackson_serde::parse_request};
use serde_json::json;

#[test]
fn frontend_lifecycle_selects_nested_identity_and_accepts_both_activation_forms() {
    for lifecycle in [json!({"identEntity":"123","activationType":{"identActivationType":"4"}}),
        json!({"identEntity":123,"identActivationType":4})] {
        let req:AkunKeuanganRequest=parse_request(&json!({"ident":999,"name":"account","lifecycleEntity":lifecycle}).to_string()).unwrap();
        let lc=LifecycleRequest::from_value(&req.lifecycle_entity).unwrap();
        assert_eq!(lc.ident_entity,123);
        assert_eq!(lc.activation_id(),Some(4));
    }
}

#[test]
fn malformed_nested_entities_and_activation_are_rejected_before_repository() {
    assert!(parse_request::<UsersRequest>("[]").is_err());
    assert!(parse_request::<CategoryRequest>("[]").is_err());
    for body in [r#"{"lifecycleEntity":[]}"#,r#"{"lifecycleEntity":{"activationType":[]}}"#,
        r#"{"lifecycleEntity":{"activationType":{"identActivationType":{}}}}"#,
        r#"{"lifecycleEntity":{"requestedActivationDate":"not-a-date"}}"#] {
        assert!(parse_request::<AkunKeuanganRequest>(body).is_err(),"{body}");
    }
    assert!(parse_request::<CategoryRequest>(r#"{"akunKeuangan":[]}"#).is_err());
    assert!(parse_request::<TransaksiKeuanganRequest>(r#"{"category":[]}"#).is_err());
}

#[test]
fn jackson_scalars_and_primitive_nulls_do_not_require_frontend_changes() {
    let req:AkunKeuanganRequest=parse_request(r#"{"name":123,"description":true,"accountType":"1","needsConfirmation":null,"pendingLedgerConfirmation":1}"#).unwrap();
    assert_eq!(req.name.as_deref(),Some("123"));
    assert_eq!(req.description.as_deref(),Some("true"));
    assert_eq!(req.account_type,Some(1));
    assert!(!req.needs_confirmation);
    assert!(req.pending_ledger_confirmation);
    let req:ChangePasswordRequest=parse_request(r#"{"identUser":123.9,"newPassword":1234}"#).unwrap();
    assert_eq!(req.ident_user,Some(123));
    assert_eq!(req.new_password.as_deref(),Some("1234"));
}

#[test]
fn exact_decimal_does_not_round_thirty_digits_through_f64_or_96_bit_mantissa() {
    let amount="123456789012345678901234567890";
    let req:AkunKeuanganRequest=parse_request(&format!(r#"{{"saldo":{amount}}}"#)).unwrap();
    assert_eq!(req.saldo.as_ref().unwrap().to_string(),amount);
    assert!(serde_json::to_string(&req).unwrap().contains(&format!(r#""saldo":{amount}"#)));
    let req:TransaksiKeuanganRequest=parse_request(r#"{"nominal":"+00150.50"}"#).unwrap();
    assert_eq!(req.nominal.as_ref().unwrap().to_string(),"150.50");
}

#[test]
fn entity_identifier_setters_follow_json_order_and_negative_id_is_zero() {
    let req:CategoryRequest=parse_request(r#"{"id":1,"entityIdent":2,"akunKeuangan":{"entityIdent":"3"}}"#).unwrap();
    assert_eq!(req.id,2);
    assert_eq!(req.akun_keuangan.unwrap()["id"],"3");
    let req:CategoryRequest=parse_request(r#"{"entityIdent":2,"id":1}"#).unwrap();
    assert_eq!(req.id,1);
    let req:CategoryRequest=parse_request(r#"{"id":-3}"#).unwrap();
    assert_eq!(req.id,0);
}

#[test]
fn jwt_optional_claims_expiry_and_not_before_match_jjwt() {
    let key=jsonwebtoken::EncodingKey::from_secret(b"1234567890123456");
    let now=chrono::Utc::now().timestamp();
    let signed=|value|jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS512),&value,&key).unwrap();
    assert_eq!(auth::validate_token(&signed(json!({"sub":"admin1"})),&auth::SecurityKey::Test),Some("admin1".into()));
    assert!(auth::validate_token(&signed(json!({"sub":"admin1","exp":now-1})),&auth::SecurityKey::Test).is_none());
    assert!(auth::validate_token(&signed(json!({"sub":"admin1","nbf":now+60})),&auth::SecurityKey::Test).is_none());
}
