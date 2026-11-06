//! Util module — jackson_serde, format tanggal, generator ID.

pub mod jackson_serde;
pub mod logging;

use rand::Rng;

/// Waktu aplikasi.
///
/// TEMUAN PENTING: deployment sistem lama berjalan dengan JVM default zone **UTC**
/// (dibuktikan: `createdDate` tersimpan "2026-10-17T19:02:43" sementara jam dinding
/// host +07:00). Semua `LocalDateTime.now()` / `new Timestamp(...)` di Java memakai
/// zona itu. Karena itu Rust memakai UTC secara default — BUKAN `chrono::Local`.
///
/// Override hanya bila deployment baru memang memakai zona lain:
/// `APP_UTC_OFFSET_MINUTES=420` (UTC+7).
pub fn app_now() -> chrono::NaiveDateTime {
    match std::env::var("APP_UTC_OFFSET_MINUTES")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
    {
        Some(min) => {
            let off = chrono::FixedOffset::east_opt(min * 60)
                .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap());
            chrono::Utc::now().with_timezone(&off).naive_local()
        }
        None => chrono::Utc::now().naive_utc(),
    }
}

/// `app_now()` diformat seperti nilai kolom DATE/TIMESTAMP Oracle (microsecond).
pub fn app_now_string() -> String {
    app_now().format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}

/// AccountIdentifierGenerator — meniru service/akunkeuangan/AccountIdentifierGenerator.java
///
/// Format: "AKN01" + 6 random uppercase alnum + 3-digit account type (leftpad "0")
///         + 2-digit check digit (IBAN-style mod 97, radix 36).
pub fn create_account_number(account_type: Option<i64>) -> Result<String, String> {
    let account_type = account_type.ok_or_else(|| "accountType is required".to_string())?;
    let mut account_id = String::from("AKN01");
    account_id.push_str(&generate_6_char_random_uppercase());
    account_id.push_str(&account_type_number_as_string(account_type));
    let check = generate_check_digit(&account_id)?;
    account_id.push_str(&check);
    Ok(account_id)
}

pub fn generate_6_char_random_uppercase() -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..6).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect()
}

/// StringUtils.leftPad(String.valueOf(n), 3, "0")
pub fn account_type_number_as_string(account_type: i64) -> String {
    format!("{:0>3}", account_type)
}

/// IBAN-style mod-97 check digit: setiap char di-convert ke digit radix-36,
/// digabung jadi BigInteger, lalu mod 97, leftpad 2 dengan '0'.
pub fn generate_check_digit(account_number_without_check_digit: &str) -> Result<String, String> {
    let mut code_iban = String::new();
    for c in account_number_without_check_digit.chars() {
        let d = char_digit_radix36(c).ok_or_else(|| format!("invalid char: {}", c))?;
        code_iban.push_str(&d.to_string());
    }
    let big = num_bigint_or_string_mod97(&code_iban)?;
    Ok(format!("{:0>2}", big))
}

fn char_digit_radix36(c: char) -> Option<u32> {
    // Character.digit(c, 36)
    c.to_digit(36)
}

/// mod 97 tanpa dependency bigint penuh: proses per-digit (Horner),
/// sama hasilnya dengan BigInteger.mod(97).
fn num_bigint_or_string_mod97(digits: &str) -> Result<u64, String> {
    let mut rem: u64 = 0;
    for ch in digits.chars() {
        let d = ch.to_digit(10).ok_or_else(|| format!("invalid digit: {}", ch))? as u64;
        rem = (rem * 10 + d) % 97;
    }
    Ok(rem)
}

/// TransaksiKeuanganPO.generateInternalReference()
/// Java: "STMT" + yyMMddHHmmss + "_" + 8-char UUID-upper + "_" + 8-char RandomStringUtils(upper)
/// = 4 + 12 + 1 + 8 + 1 + 8 = 34 karakter
pub fn generate_internal_reference(now_local: chrono::NaiveDateTime) -> String {
    let timestamp = now_local.format("%y%m%d%H%M%S").to_string();
    let uuid8 = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .to_uppercase()
        .chars()
        .take(8)
        .collect::<String>();
    let rand8 = random_alphanumeric_uppercase(8);
    format!("STMT{}_{}_{}", timestamp, uuid8, rand8)
}

/// RandomStringUtils.random(8, true, true).toUpperCase() — 8 char letters+digits, uppercased
pub fn random_alphanumeric_uppercase(n: usize) -> String {
    const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const DIGITS: &[u8] = b"0123456789";
    let all: Vec<u8> = LETTERS.iter().chain(DIGITS.iter()).copied().collect();
    let mut rng = rand::thread_rng();
    (0..n)
        .map(|_| all[rng.gen_range(0..all.len())] as char)
        .collect::<String>()
        .to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_number_format() {
        let acct = create_account_number(Some(1)).unwrap();
        // "AKN01"(5) + 6 random + 3-digit type + 2-digit check = 16
        assert_eq!(acct.len(), 16);
        assert!(acct.starts_with("AKN01"));
        // 3 digit tipe akun di posisi 11..14 (setelah "AKN01" + 6 random)
        assert_eq!(&acct[11..14], "001");
    }

    #[test]
    fn account_number_has_valid_mod97() {
        let acct = create_account_number(Some(7)).unwrap();
        let (body, check) = acct.split_at(acct.len() - 2);
        assert_eq!(generate_check_digit(body).unwrap(), check);
    }

    #[test]
    fn account_type_padding() {
        assert_eq!(account_type_number_as_string(1), "001");
        assert_eq!(account_type_number_as_string(25), "025");
        assert_eq!(account_type_number_as_string(999), "999");
    }

    #[test]
    fn internal_reference_format() {
        let now = app_now();
        let r = generate_internal_reference(now);
        assert!(r.starts_with("STMT"));
        assert_eq!(r.len(), 4 + 12 + 1 + 8 + 1 + 8);
        assert_eq!(&r[16..17], "_");
        assert_eq!(&r[25..26], "_");
    }
}
