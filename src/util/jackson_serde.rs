//! Model serde yang meniru serialisasi Jackson dari PO Java secara EKSPLISIT.
//!
//! Aturan penting (dari kode lama):
//! - Field `Option<T>`: Java `null` → JSON `null` (Jackson default) — BUKAN dihilangkan.
//!   Gunakan `Option<T>` + `#[serde(serialize_with = ...)]` yang menulis `null`.
//! - `Timestamp` dengan `@JsonFormat(pattern="yyyy-MM-dd'T'HH:mm:ss")` → string format tsb.
//! - `Timestamp` TANPA annotasi (AkunKeuanganPO.createdDate/updatedDate) → epoch-millis number.
//! - BigDecimal → angka JSON (bukan string), presisi tetap.
//! - `id` + `ident` keduanya diekspos (duplikasi dari dua getter di PO Java).

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{
    de::{Deserializer, Error as DeError},
    ser::Serializer,
    Deserialize, Serialize,
};
use serde_json::Value;

/// JSON decimal backed by the original arbitrary precision JSON number, rather
/// than a 96-bit mantissa. Oracle NUMBER(30,0) is larger than rust_decimal.
#[derive(Debug,Clone)]
pub struct ExactDecimal(serde_json::Number);

impl std::fmt::Display for ExactDecimal {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {self.0.fmt(f)}
}

pub mod exact_decimal {
    use super::*;
    pub fn serialize<S:Serializer>(value:&Option<ExactDecimal>,s:S)->Result<S::Ok,S::Error> {
        match value {None=>s.serialize_none(),Some(v)=>v.0.serialize(s)}
    }
    pub fn deserialize<'de,D:Deserializer<'de>>(d:D)->Result<Option<ExactDecimal>,D::Error> {
        match Value::deserialize(d)? {
            Value::Null=>Ok(None),
            Value::Number(v)=>Ok(Some(ExactDecimal(v))),
            Value::String(v) if v.trim().is_empty()=>Ok(None),
            Value::String(v)=> {
                let v=v.trim().trim_start_matches('+');
                let pattern=regex::Regex::new(r"^-?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$").unwrap();
                if !pattern.is_match(v) {return Err(D::Error::custom("invalid BigDecimal"));}
                let (sign,rest)=if let Some(v)=v.strip_prefix('-') {("-",v)} else {("",v)};
                let parts:Vec<_>=rest.splitn(2,['e','E']).collect();
                let decimal:Vec<_>=parts[0].splitn(2,'.').collect();
                let integer=decimal[0].trim_start_matches('0');
                let integer=if integer.is_empty(){"0"}else{integer};
                let mut canonical=format!("{}{}",sign,integer);
                if decimal.len()>1 && !decimal[1].is_empty() {canonical.push('.');canonical.push_str(decimal[1]);}
                if parts.len()>1 {canonical.push('e');canonical.push_str(parts[1]);}
                serde_json::from_str::<serde_json::Number>(&canonical).map(|n|Some(ExactDecimal(n))).map_err(D::Error::custom)
            }, _=>Err(D::Error::custom("invalid BigDecimal")),
        }
    }
}

pub fn optional_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    match Value::deserialize(d)? {
        Value::Null => Ok(None),
        Value::String(v) => Ok(Some(v)),
        Value::Number(v) => Ok(Some(v.to_string())),
        Value::Bool(v) => Ok(Some(v.to_string())),
        _ => Err(D::Error::custom("expected scalar string")),
    }
}

pub fn optional_long<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    match Value::deserialize(d)? {
        Value::Null => Ok(None),
        Value::String(v) if v.trim().is_empty() => Ok(None),
        Value::String(v) => v.trim().parse::<i64>().map(Some).map_err(D::Error::custom),
        Value::Number(v) => {
            if let Some(v) = v.as_i64() { return Ok(Some(v)); }
            let text = v.to_string();
            // Convert with decimal arithmetic, never through binary floating point.
            let decimal = text.parse::<rust_decimal::Decimal>().map_err(D::Error::custom)?;
            decimal.trunc().to_string().parse::<i64>().map(Some).map_err(D::Error::custom)
        }
        _ => Err(D::Error::custom("expected Long")),
    }
}

pub fn long_default<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(optional_long(d)?.unwrap_or(0))
}

pub fn bool_default<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    match Value::deserialize(d)? {
        Value::Null => Ok(false), Value::Bool(v) => Ok(v),
        Value::Number(v) if v.is_i64() || v.is_u64() => Ok(v.to_string() != "0"),
        Value::String(v) => match v.trim() {
            "true" | "True" => Ok(true), "false" | "False" | "" => Ok(false),
            _ => Err(D::Error::custom("expected boolean")),
        },
        _ => Err(D::Error::custom("expected boolean")),
    }
}

/// Java id and entityIdent setters target the same primitive field; the last
/// JSON property wins, and EntityObject clamps negative identifiers to zero.
pub fn parse_request<T: serde::de::DeserializeOwned>(body:&str)->serde_json::Result<T> {
    fn normalize(value:&mut Value) {
        match value {
            Value::Object(map)=> {
                let mut identity=None;
                for (key,value) in map.iter_mut() {
                    normalize(value);
                    if key=="id" || key=="entityIdent" {identity=Some(value.clone());}
                }
                if let Some(mut identity)=identity {
                    if identity.as_i64().map(|v|v<0).unwrap_or(false) {identity=Value::from(0);}
                    map.remove("entityIdent"); map.insert("id".into(),identity);
                }
            }, Value::Array(values)=>values.iter_mut().for_each(normalize), _=>{},
        }
    }
    let mut value:Value=serde_json::from_str(body)?;
    if !value.is_object() {
        return Err(<serde_json::Error as serde::de::Error>::custom("expected JSON object"));
    }
    normalize(&mut value);
    serde_json::from_value(value)
}

pub fn account_relation<'de,D:Deserializer<'de>>(d:D)->Result<Option<Value>,D::Error> {
    let value=Option::<Value>::deserialize(d)?;
    if let Some(ref v)=value {
        if !v.is_object() {return Err(D::Error::custom("expected account object"));}
        serde_json::from_value::<crate::models::AkunKeuanganPo>(v.clone()).map_err(D::Error::custom)?;
    }
    Ok(value)
}

pub fn category_relation<'de,D:Deserializer<'de>>(d:D)->Result<Option<Value>,D::Error> {
    let value=Option::<Value>::deserialize(d)?;
    if let Some(ref v)=value {
        if !v.is_object() {return Err(D::Error::custom("expected category object"));}
        serde_json::from_value::<crate::models::CategoryPo>(v.clone()).map_err(D::Error::custom)?;
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// Timestamp "yyyy-MM-dd'T'HH:mm:ss" (tanpa millis, tanpa timezone)
// Jackson @JsonFormat(pattern = "yyyy-MM-dd'T'HH:mm:ss")
// ---------------------------------------------------------------------------

pub mod timestamp_json_format {
    use super::*;

    const FMT: &str = "%Y-%m-%dT%H:%M:%S";

    pub fn serialize<S: Serializer>(t: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            // Jackson menulis null secara eksplisit untuk field null
            None => s.serialize_none(),
            Some(dt) => s.serialize_str(&dt.format(FMT).to_string()),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime<Utc>>, D::Error> {
        match Value::deserialize(d)? {
            Value::Null => Ok(None),
            Value::String(s) if s.trim().is_empty()=>Ok(None),
            Value::Number(n)=>n.as_i64().and_then(DateTime::from_timestamp_millis).map(Some)
                .ok_or_else(||D::Error::custom("invalid timestamp")),
            Value::String(s) => Ok(Some(
                parse_flexible(&s).ok_or_else(|| D::Error::custom(format!("invalid timestamp: {}", s)))?,
            )),
            _=>Err(D::Error::custom("invalid timestamp")),
        }
    }
}

/// Jackson bisa menerima beberapa format tanggal; sistem lama umumnya mengirim
/// "yyyy-MM-dd'T'HH:mm:ss" (tanpa fraction). Lenient juga menerima ISO 8601 lengkap.
fn parse_flexible(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(DateTime::from_naive_utc_and_offset(ndt, Utc));
    }
    // ISO 8601 dengan offset / fraction
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // Timestamp epoch millis (Jackson WRITE_DATES_AS_TIMESTAMPS)
    if let Ok(millis) = s.parse::<i64>() {
        return Some(DateTime::from_timestamp_millis(millis)?);
    }
    None
}

// ---------------------------------------------------------------------------
// Timestamp epoch-millis (Jackson default untuk java.sql.Timestamp TANPA @JsonFormat)
// Dipakai AkunKeuanganPO.createdDate / updatedDate
// ---------------------------------------------------------------------------

pub mod timestamp_epoch_millis {
    use super::*;

    pub fn serialize<S: Serializer>(t: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            None => s.serialize_none(),
            Some(dt) => s.serialize_i64(dt.timestamp_millis()),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime<Utc>>, D::Error> {
        match Value::deserialize(d)? {
            Value::Null => Ok(None),
            Value::Number(n) => {
                let millis = n
                    .as_i64()
                    .or_else(|| n.as_f64().map(|f| f as i64))
                    .ok_or_else(|| D::Error::custom("invalid epoch millis"))?;
                Ok(DateTime::from_timestamp_millis(millis))
            }
            Value::String(s) => parse_flexible(&s)
                .map(Some)
                .ok_or_else(|| D::Error::custom(format!("invalid timestamp: {}", s))),
            _ => Err(D::Error::custom("invalid timestamp value")),
        }
    }
}

// ---------------------------------------------------------------------------
// LocalDateTime "yyyy-MM-dd HH:mm:ss" (LifecycleEntity @JsonFormat pada request DTO)
// ---------------------------------------------------------------------------

pub mod localdatetime_space_format {
    use super::*;
    use chrono::NaiveDateTime;

    const FMT: &str = "%Y-%m-%d %H:%M:%S";

    pub fn serialize<S: Serializer>(t: &Option<NaiveDateTime>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            None => s.serialize_none(),
            Some(dt) => s.serialize_str(&dt.format(FMT).to_string()),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<NaiveDateTime>, D::Error> {
        let opt: Option<String> = Option::deserialize(d)?;
        match opt {
            None => Ok(None),
            Some(s) => NaiveDateTime::parse_from_str(&s, FMT)
                .map(Some)
                .or_else(|_| DateTime::parse_from_rfc3339(&s).map(|dt| Some(dt.naive_utc())))
                .map_err(|_| D::Error::custom(format!("invalid datetime: {}", s))),
        }
    }
}

// ---------------------------------------------------------------------------
// BigDecimal → JSON number (Jackson menulis BigDecimal sebagai angka, bukan string)
// rust_decimal preserve scale agar "12.50" tidak jadi "12.5"
// ---------------------------------------------------------------------------

pub mod bigdecimal_number {
    use super::*;
    use rust_decimal::Decimal;

    pub fn serialize<S: Serializer>(v: &Option<Decimal>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(d) => {
                // serde_json::Number preserves scale via arbitrary precision feature
                let num: serde_json::Number = serde_json::from_str(&d.to_string())
                    .map_err(serde::ser::Error::custom)?;
                Value::Number(num).serialize(s)
            }
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Decimal>, D::Error> {
        match Value::deserialize(d)? {
            Value::Null => Ok(None),
            Value::Number(n) => {
                let s = n.to_string();
                rust_decimal::Decimal::from_str_exact(&s)
                    .map(Some)
                    .or_else(|_| s.parse::<Decimal>().map(Some))
                    .map_err(|_| D::Error::custom(format!("invalid decimal: {}", s)))
            }
            Value::String(s) => s
                .parse::<Decimal>()
                .map(Some)
                .map_err(|_| D::Error::custom(format!("invalid decimal: {}", s))),
            _ => Err(D::Error::custom("invalid decimal value")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    #[test]
    fn timestamp_null_is_json_null() {
        let s = serde_json::to_string(&Wrapper { t: None }).unwrap();
        assert_eq!(s, r#"{"t":null}"#);
    }

    #[test]
    fn timestamp_format_matches_jackson() {
        let dt = DateTime::from_timestamp_millis(0).unwrap();
        let s = serde_json::to_string(&Wrapper { t: Some(dt) }).unwrap();
        assert_eq!(s, r#"{"t":"1970-01-01T00:00:00"}"#);
    }

    #[test]
    fn epoch_millis_number() {
        let dt = DateTime::from_timestamp_millis(1729160000000).unwrap();
        let s = serde_json::to_string(&Wrapper2 { t: Some(dt) }).unwrap();
        assert_eq!(s, r#"{"t":1729160000000}"#);
    }

    #[test]
    fn decimal_preserves_scale() {
        let s = serde_json::to_string(&Wrapper3 {
            d: Some(Decimal::new(1250, 2)),
        })
        .unwrap();
        assert_eq!(s, r#"{"d":12.50}"#);
    }

    #[derive(Serialize)]
    struct Wrapper {
        #[serde(with = "timestamp_json_format")]
        t: Option<DateTime<Utc>>,
    }
    #[derive(Serialize)]
    struct Wrapper2 {
        #[serde(with = "timestamp_epoch_millis")]
        t: Option<DateTime<Utc>>,
    }
    #[derive(Serialize)]
    struct Wrapper3 {
        #[serde(with = "bigdecimal_number")]
        d: Option<Decimal>,
    }
}
