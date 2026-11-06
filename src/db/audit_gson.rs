//! Audit — output JSON kompatibel Gson untuk `/event/evententries`.
//!
//! Gson semantics (berbeda dari Jackson!):
//! - dateFormat "yyyy-MM-dd'T'HH:mm:ssXXX" (offset zona sistem)
//! - disableHtmlEscaping
//! - `data` getter mengembalikan hasil dekompresi bila compressedData terisi
//! - byte[] (compressedData) → Gson menulis ARRAY ANGKA (bukan Base64)
//!
//! Urutan field direkam dari EventEntry.java:
//! entryId, eventId, name, timeStamp, nanoSecondsOnTimestamp, businessIdentifier,
//! data, compressedData, entityName, entityId, masterId, revisionId

use anyhow::Result;
use serde_json::{json, Map, Value};

/// Bangun satu objek EventEntry dalam bentuk Gson.
#[allow(clippy::too_many_arguments)]
pub fn entry_json(
    entry_id: i64,
    event_id: &str,
    name: Option<String>,
    time_stamp: Option<String>,
    nano_seconds: Option<i32>,
    business_identifier: Option<String>,
    data: Option<String>,
    compressed_data: Option<Vec<u8>>,
    entity_name: Option<String>,
    entity_id: Option<i64>,
    master_id: Option<i64>,
    revision_id: Option<i64>,
) -> Result<Value> {
    let mut m = Map::new();
    m.insert("entryId".into(), json!(entry_id));
    m.insert("eventId".into(), json!(event_id));
    m.insert("name".into(), json!(name));
    m.insert("timeStamp".into(), fmt_gson_offset(time_stamp));
    m.insert("nanoSecondsOnTimestamp".into(), json!(nano_seconds));
    m.insert("businessIdentifier".into(), json!(business_identifier));

    // data: getter mengembalikan decompressed bila compressedData terisi
    let effective_data = data.or_else(|| {
        compressed_data
            .as_ref()
            .and_then(|c| inflate(c).ok())
            .and_then(|b| String::from_utf8(b).ok())
    });
    m.insert("data".into(), json!(effective_data));

    // Gson menulis byte[] sebagai array angka
    m.insert(
        "compressedData".into(),
        match compressed_data {
            None => Value::Null,
            Some(bytes) => Value::Array(
                bytes
                    .into_iter()
                    .map(|b| json!(b as i8 as i64))
                    .collect(),
            ),
        },
    );

    m.insert("entityName".into(), json!(entity_name));
    m.insert("entityId".into(), json!(entity_id));
    m.insert("masterId".into(), json!(master_id));
    m.insert("revisionId".into(), json!(revision_id));
    Ok(Value::Object(m))
}

/// Gson LocalDateTimeSerializer: "yyyy-MM-dd'T'HH:mm:ssXXX" dengan offset zona sistem.
/// Deployment lama memakai UTC → "+00:00".
fn fmt_gson_offset(v: Option<String>) -> Value {
    match v {
        None => Value::Null,
        // Nilai sudah "YYYY-MM-DD HH24:MI:SS[.FF6]" dari TO_CHAR
        Some(s) => {
            let base = s.split('.').next().unwrap_or(&s).replace(' ', "T");
            json!(format!("{}+00:00", base))
        }
    }
}

/// DEFLATE decompress (java.util.zip.Deflater) — CompressionUtil di kode lama
fn inflate(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut decoder = flate2::read::DeflateDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}
