//! File upload — meniru `filetransfer/UploadController.java` +
//! `service/filepersister/FilePersisterService.java`.
//!
//! Kontrak (direkam dari sistem berjalan 2026-10-18):
//! - `POST /fileupload/**` (multipart, part name `file`), butuh JWT.
//! - Header opsional `roles` — dipisah `:`; lolos jika SALAH SATU ada di
//!   `filetransfer.roles` (`Administration,Vito_Super`).
//!   Roles kosong / tidak ada / tidak cocok → **401 body KOSONG**.
//! - Header opsional `filter` — format `KEY:value` dipisah `,`, key di-uppercase.
//!   Hanya `PARTICIPANT` yang dipakai, sebagai postfix nama file.
//! - Sukses → **200** body `{"filename": "<nama>"}` (perhatikan SPASI setelah
//!   titik dua: Java merangkai string manual, bukan serializer JSON).
//! - Gagal I/O → **500 body KOSONG**.
//!
//! Nama file: `<base>_<epochMillis>_<postfix>.<ext>`
//! - postfix null → string literal `"null"` (Java `StringBuilder.append(null)`)
//! - tanpa ekstensi → tanpa titik
//!
//! Penulisan: salin ke `<nama>.temp` lalu `move` ke nama final.

use crate::api::auth_middleware::AuthUser;
use crate::config;
use axum::extract::{Extension, Multipart};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// `parseFilter`: `PARTICIPANT:ABC,OTHER:x` → {"PARTICIPANT": "ABC", "OTHER": "x"}
fn parse_filter(filter: Option<&str>) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Some(raw) = filter else { return out };
    // Java menelan exception apa pun di sini (try/catch kosong)
    for setting in raw.split(',') {
        // String.split removes trailing empty tokens; a malformed setting
        // aborts Java's entire loop, retaining only earlier valid settings.
        let mut parts = setting.trim_end_matches(':').split(':');
        let key = parts.next().unwrap_or("");
        let Some(value) = parts.next() else { break; };
        out.insert(key.to_uppercase(), value.to_string());
    }
    out
}

/// `parseRoles`: dipisah `:`
fn parse_roles(roles: Option<&str>) -> Vec<String> {
    match roles {
        None => Vec::new(),
        Some("") => vec![String::new()],
        Some(raw) => {
            let mut roles: Vec<_> = raw.split(':').map(str::to_string).collect();
            while roles.last().is_some_and(String::is_empty) { roles.pop(); }
            roles
        },
    }
}

/// `createFilenameWithPostfix`
fn create_filename_with_postfix(filename: &str, postfix: Option<&str>, millis: u128) -> String {
    let mut sb = String::new();
    match filename.rfind('.') {
        Some(idx) => sb.push_str(&filename[..idx]),
        None => sb.push_str(filename),
    }
    sb.push('_');
    sb.push_str(&millis.to_string());
    sb.push('_');
    // append(null) pada StringBuilder menulis "null"
    sb.push_str(postfix.unwrap_or("null"));
    if let Some(idx) = filename.rfind('.') {
        sb.push('.');
        if idx + 1 < filename.len() {
            sb.push_str(&filename[idx + 1..]);
        }
    }
    sb
}

/// `setFilenameEndingToTemp`
fn set_filename_ending_to_temp(filename: &str) -> String {
    let mut sb = String::new();
    match filename.rfind('.') {
        Some(idx) => sb.push_str(&filename[..idx]),
        None => sb.push_str(filename),
    }
    sb.push_str(".temp");
    sb
}

/// POST /fileupload/*
pub async fn upload(
    Extension(_user): Extension<AuthUser>,
    headers: axum::http::HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let cfg = config::file_transfer();

    let roles_header = headers
        .get("roles")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let filter_header = headers
        .get("filter")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Role check SEBELUM membaca body (Java: di dalam try, sebelum persist)
    let roles = parse_roles(roles_header.as_deref());
    if !cfg.has_role(&roles) {
        return crate::api::errors::empty_body(StatusCode::UNAUTHORIZED);
    }

    // Baca part "file"
    let mut original_filename: Option<String> = None;
    let mut bytes: Vec<u8> = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            original_filename = field.file_name().map(|s| s.to_string());
            match field.bytes().await {
                Ok(b) => bytes = b.to_vec(),
                Err(_) => return crate::api::errors::empty_body(StatusCode::INTERNAL_SERVER_ERROR),
            }
            break;
        }
    }

    let Some(original) = original_filename else {
        return crate::api::errors::empty_body(StatusCode::INTERNAL_SERVER_ERROR);
    };

    let filters = parse_filter(filter_header.as_deref());
    let postfix = filters.get("PARTICIPANT").map(|s| s.as_str());

    // System.currentTimeMillis()
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let new_name = create_filename_with_postfix(&original, postfix, millis);
    let temp_name = set_filename_ending_to_temp(&new_name);

    let dir = std::path::Path::new(&cfg.target_directory);
    let temp_path = dir.join(&temp_name);
    let final_path = dir.join(&new_name);

    // copy(InputStream, temp) lalu move(temp, final)
    if std::fs::write(&temp_path, &bytes).is_err() {
        return crate::api::errors::empty_body(StatusCode::INTERNAL_SERVER_ERROR);
    }
    if std::fs::rename(&temp_path, &final_path).is_err() {
        let _ = std::fs::remove_file(&temp_path);
        return crate::api::errors::empty_body(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Java merangkai string manual: "{\"filename\": \"" + filename + "\"}"
    // → ada SPASI setelah titik dua.
    let body = format!("{{\"filename\": \"{}\"}}", new_name);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_with_extension_and_postfix() {
        assert_eq!(
            create_filename_with_postfix("data.csv", Some("ABC"), 1700000000000),
            "data_1700000000000_ABC.csv"
        );
    }

    #[test]
    fn filename_without_postfix_uses_literal_null() {
        // Java: StringBuilder.append((String) null) → "null"
        assert_eq!(
            create_filename_with_postfix("data.csv", None, 1700000000000),
            "data_1700000000000_null.csv"
        );
    }

    #[test]
    fn filename_without_extension() {
        assert_eq!(
            create_filename_with_postfix("noext", Some("P1"), 1700000000000),
            "noext_1700000000000_P1"
        );
    }

    #[test]
    fn temp_ending_replaces_extension() {
        assert_eq!(set_filename_ending_to_temp("a_1_x.csv"), "a_1_x.temp");
        assert_eq!(set_filename_ending_to_temp("noext"), "noext.temp");
    }

    #[test]
    fn filter_parsing_uppercases_keys() {
        let f = parse_filter(Some("participant:ABC,other:x"));
        assert_eq!(f.get("PARTICIPANT").map(String::as_str), Some("ABC"));
        assert_eq!(f.get("OTHER").map(String::as_str), Some("x"));
    }

    #[test]
    fn malformed_upload_filters_stop_like_java_instead_of_accepting_later_values() {
        assert!(parse_filter(Some("broken,PARTICIPANT:ABC")).is_empty());
        assert!(parse_filter(Some("PARTICIPANT:,PARTICIPANT:ABC")).is_empty());
        assert_eq!(parse_filter(Some("PARTICIPANT:FIRST,broken,PARTICIPANT:SECOND"))["PARTICIPANT"], "FIRST");
        assert_eq!(parse_filter(Some("PARTICIPANT::suffix"))["PARTICIPANT"], "");
    }

    #[test]
    fn roles_split_by_colon() {
        assert_eq!(parse_roles(Some("Wrong:Vito_Super")), vec!["Wrong", "Vito_Super"]);
        assert!(parse_roles(None).is_empty());
        assert_eq!(parse_roles(Some("Wrong:Vito_Super::")), vec!["Wrong", "Vito_Super"]);
        assert!(parse_roles(Some("::")).is_empty());
    }
}
