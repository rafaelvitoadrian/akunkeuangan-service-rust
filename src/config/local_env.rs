//! Small literal KEY=VALUE configuration format; no shell or interpolation.
use anyhow::{bail, Context, Result};
use std::path::Path;

/// Existing process environment takes precedence. Parse completely before applying.
/// Call once, before starting the async runtime or any worker threads.
pub fn load(path: &Path, required: bool) -> Result<bool> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("cannot read config {}", path.display())),
    };
    for (key, value) in parse(&text)? {
        if std::env::var_os(&key).is_none() {
            std::env::set_var(key, value);
        }
    }
    Ok(true)
}

fn parse(text: &str) -> Result<Vec<(String, String)>> {
    let mut values = Vec::new();
    for (index, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let (key, raw) = line.split_once('=').with_context(||
            format!("invalid config at line {}: expected KEY=VALUE", index + 1))?;
        let key = key.trim();
        if key.is_empty() || !key.bytes().enumerate().all(|(i, b)|
            b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit())) {
            bail!("invalid config key at line {}", index + 1);
        }
        let raw = raw.trim();
        let value = if raw.starts_with(['\'', '"']) {
            if raw.len() < 2 || !raw.ends_with(raw.chars().next().unwrap()) {
                bail!("unclosed config quote at line {}", index + 1);
            }
            &raw[1..raw.len() - 1]
        } else { raw };
        // Literal values preserve Windows paths and password characters ($, #, =).
        values.push((key.to_owned(), value.to_owned()));
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_values_preserve_passwords_and_windows_paths() {
        let values = parse("\u{feff}# comment\r\nexport USER = admin\r\nPASSWORD='a$#= b'\r\nCLIENT=\"D:\\Database\\oracle\\bin\"\nEMPTY=\n").unwrap();
        assert_eq!(values, vec![("USER".into(), "admin".into()),
            ("PASSWORD".into(), "a$#= b".into()),
            ("CLIENT".into(), "D:\\Database\\oracle\\bin".into()),
            ("EMPTY".into(), "".into())]);
    }

    #[test]
    fn invalid_config_errors_never_echo_secret_values() {
        for text in ["PASSWORD='secret", "1INVALID=secret", "secret"] {
            let error = parse(text).unwrap_err().to_string();
            assert!(!error.contains("secret"));
            assert!(error.contains("line 1"));
        }
    }
}
