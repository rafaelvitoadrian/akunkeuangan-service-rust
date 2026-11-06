//! Current Java UserController contract and Spring @ModelAttribute form binding.
use crate::util::jackson_serde as js;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
#[error("User have insufficient accesslevel for function '{0}'.")]
pub struct AccessDenied(pub String);

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleRequest {
    #[serde(default, deserialize_with = "js::long_default")]
    pub ident_user_role: i64,
    #[serde(default, deserialize_with = "js::optional_string")]
    pub shortname: Option<String>,
    #[serde(default, deserialize_with = "js::optional_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "js::optional_string")]
    pub description: Option<String>,
    #[serde(default)]
    pub activation_status: Option<ActivationStatus>,
    #[serde(
        default,
        rename = "applicableForPOA",
        deserialize_with = "js::bool_default"
    )]
    pub applicable_for_p_o_a: bool,
    #[serde(default, deserialize_with = "js::bool_default")]
    pub internal: bool,
    #[serde(default)]
    pub user_role_functions: Vec<RoleFunctionRequest>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ActivationStatus {
    Active,
    Suspended,
    Deactivated,
}
impl ActivationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Suspended => "Suspended",
            Self::Deactivated => "Deactivated",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleFunctionRequest {
    pub function: Option<FunctionRef>,
    pub access_level: Option<AccessRef>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FunctionRef {
    #[serde(
        rename = "identFunction",
        default,
        deserialize_with = "js::long_default"
    )]
    pub id: i64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AccessRef {
    #[serde(
        rename = "identAccessLevel",
        default,
        deserialize_with = "js::long_default"
    )]
    pub id: i64,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRequest {
    #[serde(default, deserialize_with = "js::long_default")]
    pub ident_permission_profile: i64,
    #[serde(default, deserialize_with = "js::optional_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "null_sequence")]
    pub user_roles: Vec<RoleRequest>,
    pub user_account: Option<UserRef>,
    #[serde(default, deserialize_with = "optional_date")]
    pub deactivation_date: Option<chrono::NaiveDate>,
}
fn null_sequence<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}
fn optional_date<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<chrono::NaiveDate>, D::Error> {
    match Option::<String>::deserialize(d)? {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UserRef {
    #[serde(default, deserialize_with = "js::long_default")]
    pub ident: i64,
}

/// Base.setXmlIdentifier and each concrete identity setter write the same
/// Java field. JSON applies setters in property order; Spring's form binder
/// visits parameter names in sorted order, so xmlIdentifier is applied last.
pub trait FormModel {
    const ID_FIELD: &'static str;
}
impl FormModel for RoleRequest { const ID_FIELD: &'static str = "identUserRole"; }
impl FormModel for ProfileRequest { const ID_FIELD: &'static str = "identPermissionProfile"; }

fn normalize_model_identity(value: &mut Value, id_field: &str, form: bool) -> anyhow::Result<()> {
    let Some(map) = value.as_object_mut() else { return Ok(()); };
    for (key, child_id) in [("userRoles", "identUserRole"), ("permissionProfiles", "identPermissionProfile"), ("userRoleFunctions", "ident")] {
        if let Some(Value::Array(children)) = map.get_mut(key) {
            for child in children { normalize_model_identity(child, child_id, form)?; }
        }
    }
    for (key, child_id) in [("function", "identFunction"), ("accessLevel", "identAccessLevel"), ("userAccount", "ident")] {
        if let Some(child) = map.get_mut(key) { normalize_model_identity(child, child_id, form)?; }
    }
    let xml_identity = |value: &Value| -> anyhow::Result<Value> {
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            _ => anyhow::bail!("invalid xmlIdentifier"),
        };
        // Long.parseLong in Base.setXmlIdentifier does not trim/accept blanks.
        Ok(json!(text.parse::<i64>()?))
    };
    let mut identity = None;
    if form {
        if let Some(xml) = map.get("xmlIdentifier") { identity = Some(xml_identity(xml)?); }
    } else {
        for (key, value) in map.iter() {
            if key == "xmlIdentifier" { identity = Some(xml_identity(value)?); }
            else if key == "ident" || key == id_field { identity = Some(value.clone()); }
        }
    }
    if let Some(identity) = identity { map.insert(id_field.into(), identity); }
    Ok(())
}

pub fn deserialize_profiles<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<ProfileRequest>>, D::Error> {
    use serde::de::Error;
    Option::<Vec<Value>>::deserialize(d)?.map(|values| values.into_iter().map(|mut value| {
        normalize_model_identity(&mut value, "identPermissionProfile", false).map_err(D::Error::custom)?;
        serde_json::from_value(value).map_err(D::Error::custom)
    }).collect()).transpose()
}

pub enum SecurityQuery {
    Roles(Option<String>),
    PoaRoles,
    Functions,
    AuthLevels,
    AccessLevels,
    AuthorizedProfiles,
}
pub enum SecurityWrite {
    CreateRole(RoleRequest),
    UpdateRole(RoleRequest),
    DeleteRole(RoleRequest),
    CreateProfile(ProfileRequest),
    UpdateProfile(ProfileRequest),
    DeactivateProfile(ProfileRequest),
}

/// All DTOs extend Java Base. Keep this separate from persistence entity JSON.
pub fn base(id: i64) -> Value {
    json!({"ident":id,"needsConfirmation":false,"pendingLedgerConfirmation":false,
        "xmlIdentifier":id.to_string(),"valid":id!=0})
}
pub fn user_model(raw: &Value, profiles: Value, actor: Option<Value>) -> Value {
    let id = raw["id"]
        .as_i64()
        .or_else(|| raw["ident"].as_i64())
        .unwrap_or(0);
    let mut result = base(id);
    for key in ["name", "email", "username", "pwdHash", "password", "salt"] {
        result[key] = raw[key].clone();
    }
    let mut lifecycle = json!({"identEntity":id,"workflowStatus":null,"lastUpdateUser":actor});
    for key in [
        "identMaster",
        "identRevision",
        "revisionEffectiveFrom",
        "revisionEffectiveTo",
        "requestedActivationDate",
        "lastUpdateTime",
    ] {
        lifecycle[key] = raw[key].clone();
    }
    for key in [
        "revisionEffectiveFrom",
        "revisionEffectiveTo",
        "lastUpdateTime",
    ] {
        if let Some(value) = raw[key].as_str() {
            lifecycle[key] = json!(value.split('.').next().unwrap_or(value).replace('T', " "));
        }
    }
    for (key, ident, label) in [
        ("activationType", "identActivationType", "typeName"),
        ("revisionState", "identRevisionState", "name"),
    ] {
        let value = &raw[key];
        lifecycle[key] = if value.is_object() {
            let mut dto = base(value[ident].as_i64().unwrap_or(0));
            dto[ident] = value[ident].clone();
            dto[label] = value[label].clone();
            if key == "activationType" {
                for flag in ["active", "inActive", "suspended", "deactivated"] {
                    dto[flag] = value[flag].clone();
                }
            }
            dto
        } else {
            Value::Null
        };
    }
    result["lifecycleEntity"] = lifecycle;
    result["permissionProfiles"] = profiles;
    result
}

/// Spring's indexed bean properties, including query parameters and form data.
/// Bound the same default auto-grow limit (256) before allocating arrays.
pub fn bind_form<T: serde::de::DeserializeOwned + FormModel>(
    query: Option<&str>,
    body: &str,
    is_form: bool,
) -> anyhow::Result<T> {
    let mut root = json!({});
    for part in query.into_iter().chain(is_form.then_some(body)) {
        for pair in part.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let decode = |s: &str| {
                percent_encoding::percent_decode_str(&s.replace('+', " "))
                    .decode_utf8()
                    .map(|v| v.into_owned())
            };
            let key = decode(key)?;
            let value = decode(value)?;
            let tokens: Vec<&str> = key
                .split(['.', '[', ']'])
                .filter(|s| !s.is_empty())
                .collect();
            if tokens.is_empty()
                || !matches!(
                    tokens[0],
                    "ident"
                        | "xmlIdentifier"
                        | "identUserRole"
                        | "identPermissionProfile"
                        | "shortname"
                        | "name"
                        | "description"
                        | "activationStatus"
                        | "applicableForPOA"
                        | "internal"
                        | "userRoleFunctions"
                        | "userRoles"
                        | "userAccount"
                        | "deactivationDate"
                )
            {
                continue;
            }
            anyhow::ensure!(tokens.len() <= 8, "invalid bean path");
            insert_property(&mut root, &tokens, Value::String(value))?;
        }
    }
    normalize_model_identity(&mut root, T::ID_FIELD, true)?;
    Ok(serde_json::from_value(root)?)
}
fn insert_property(node: &mut Value, tokens: &[&str], value: Value) -> anyhow::Result<()> {
    if tokens.is_empty() {
        *node = value;
        return Ok(());
    }
    if let Ok(index) = tokens[0].parse::<usize>() {
        anyhow::ensure!(index < 256, "bean index exceeds auto-grow limit");
        if node.is_null() {
            *node = json!([]);
        }
        let array = node
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("invalid array binding"))?;
        while array.len() <= index {
            array.push(Value::Null);
        }
        insert_property(&mut array[index], &tokens[1..], value)
    } else {
        if node.is_null() {
            *node = json!({});
        }
        let map = node
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("invalid object binding"))?;
        insert_property(
            map.entry(tokens[0]).or_insert(Value::Null),
            &tokens[1..],
            value,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base_identity_aliases_follow_java_json_setter_order_and_spring_form_order() {
        for body in ["xmlIdentifier=9&identUserRole=7", "identUserRole=7&xmlIdentifier=9"] {
            assert_eq!(bind_form::<RoleRequest>(None, body, true).unwrap().ident_user_role, 9);
        }
        let profile: ProfileRequest = bind_form(None, "xmlIdentifier=9&userRoles[0].xmlIdentifier=12&userRoles[0].userRoleFunctions[0].function.xmlIdentifier=111&userRoles[0].userRoleFunctions[0].accessLevel.xmlIdentifier=4", true).unwrap();
        assert_eq!(profile.ident_permission_profile, 9);
        assert_eq!(profile.user_roles[0].ident_user_role, 12);
        assert_eq!(profile.user_roles[0].user_role_functions[0].function.as_ref().unwrap().id, 111);
        assert_eq!(profile.user_roles[0].user_role_functions[0].access_level.as_ref().unwrap().id, 4);
        for (body, expected) in [(r#"{"xmlIdentifier":"9","identPermissionProfile":7}"#, 7), (r#"{"identPermissionProfile":7,"xmlIdentifier":"9"}"#, 9), (r#"{"ident":5}"#, 5)] {
            let request: crate::models::UsersRequest = crate::util::jackson_serde::parse_request(&format!(r#"{{"permissionProfiles":[{body}]}}"#)).unwrap();
            assert_eq!(request.permission_profiles.unwrap()[0].ident_permission_profile, expected);
        }
        for xml in ["", " 9 ", "9223372036854775808"] {
            assert!(bind_form::<RoleRequest>(None, &format!("xmlIdentifier={xml}"), true).is_err());
        }
    }
    #[test]
    fn indexed_forms_decode_unicode_query_and_empty_dates() {
        let role:RoleRequest=bind_form(Some("name=Nama+awal&shortname=VIEW"),"name=Keuangan+%E2%82%AC&activationStatus=Active&applicableForPOA=true&userRoleFunctions[0].function.identFunction=9&userRoleFunctions[0].accessLevel.identAccessLevel=2",true).unwrap();
        assert_eq!(role.name.as_deref(), Some("Keuangan €"));
        assert!(role.applicable_for_p_o_a);
        assert_eq!(role.user_role_functions[0].function.as_ref().unwrap().id, 9);
        let profile: ProfileRequest = bind_form(
            None,
            "name=Shared&deactivationDate=&userRoles[0].identUserRole=7",
            true,
        )
        .unwrap();
        assert!(profile.deactivation_date.is_none());
        assert_eq!(profile.user_roles[0].ident_user_role, 7);
    }
    #[test]
    fn invalid_forms_fail_before_repository_and_full_dtos_roundtrip() {
        for form in [
            "internal=maybe",
            "activationStatus=UNKNOWN",
            "userRoleFunctions[0].function.identFunction=NaN",
            "userRoleFunctions[256].function.identFunction=1",
            "userRoleFunctions[0].function.identFunction=9223372036854775808",
        ] {
            assert!(
                bind_form::<RoleRequest>(None, form, true).is_err(),
                "{form}"
            );
        }
        assert!(bind_form::<ProfileRequest>(None, "deactivationDate=not-a-date", true).is_err());
        let profile:ProfileRequest=serde_json::from_value(json!({"ident":7,"identPermissionProfile":7,"xmlIdentifier":"7","name":"Test","userRoles":[{"ident":9,"identUserRole":9,"xmlIdentifier":"9","userRoleFunctions":[{"function":{"ident":1,"identFunction":1},"accessLevel":{"ident":2,"identAccessLevel":2}}]}]})).unwrap();
        assert_eq!(profile.ident_permission_profile, 7);
        assert_eq!(profile.user_roles[0].ident_user_role, 9);
    }
}
