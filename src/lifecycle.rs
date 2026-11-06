//! Request lifecycle fields used by the Java converter. Read-only metadata in
//! the request never overrides the persisted master/revision chain.
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleRequest {
    #[serde(default, deserialize_with = "crate::util::jackson_serde::long_default")]
    pub ident_entity: i64,
    #[serde(default)]
    pub activation_type: Option<ActivationRequest>,
    #[serde(default)]
    pub requested_activation_date: Option<NaiveDate>,
    // Existing React payload.js also emits this flat form. Accept both forms
    // without requiring any frontend edit; the nested Java form has precedence.
    #[serde(default, deserialize_with = "crate::util::jackson_serde::optional_long")]
    pub ident_activation_type: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationRequest {
    #[serde(default, deserialize_with = "crate::util::jackson_serde::long_default")]
    pub ident_activation_type: i64,
}

impl LifecycleRequest {
    pub fn from_value(value: &Option<serde_json::Value>) -> anyhow::Result<Self> {
        Ok(serde_json::from_value(value.clone().ok_or_else(|| anyhow::anyhow!("null lifecycleEntity"))?)?)
    }

    pub fn activation_id(&self) -> Option<i64> {
        self.activation_type.as_ref().map(|v| v.ident_activation_type)
            .or(self.ident_activation_type)
    }
}

pub fn default_request() -> Option<serde_json::Value> {
    Some(serde_json::json!({}))
}

pub fn deserialize_request<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<serde_json::Value>, D::Error> {
    use serde::de::Error;
    let value = Option::<serde_json::Value>::deserialize(d)?;
    if let Some(ref value) = value {
        if !value.is_object() {return Err(D::Error::custom("expected lifecycle object"));}
        if let Some(activation)=value.get("activationType") {
            if !activation.is_null() && !activation.is_object() {return Err(D::Error::custom("expected activation object"));}
        }
        serde_json::from_value::<LifecycleRequest>(value.clone()).map_err(D::Error::custom)?;
    }
    Ok(value)
}
