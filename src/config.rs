//! Konfigurasi aplikasi — nama property mengikuti `application.properties` lama.
//!
//! | Property lama | Env var Rust | Default |
//! |---|---|---|
//! | `server.port` | `SERVER_PORT` | `9090` |
//! | `filetransfer.targetDirectory` | `FILETRANSFER_TARGET_DIRECTORY` | `C:\vmh\files\input` |
//! | `filetransfer.roles` | `FILETRANSFER_ROLES` | `Administration,Vito_Super` |
//! | `spring.datasource.*` | `SPRING_DATASOURCE_*` | — |
//! | `audit.datasource.*` | `AUDIT_DATASOURCE_*` | sama dengan main |
//! | `spring.activemq.broker-url` | `SPRING_ACTIVEMQ_BROKER_URL` | Fase 2 |
//!
//! Property DB (`si.server.url`, `is.test`) dibaca dari tabel `SYSTEMPROP`,
//! bukan dari file — sama seperti `AkunConfigurationCache` di sistem lama.

use std::sync::OnceLock;

pub mod local_env;
pub mod cors;

/// PaaS port wins over local configuration; reject invalid values at startup.
pub fn server_port() -> anyhow::Result<u16> {
    parse_server_port(std::env::var("PORT").ok(), std::env::var("SERVER_PORT").ok())
}

fn parse_server_port(port: Option<String>, server_port: Option<String>) -> anyhow::Result<u16> {
    use anyhow::{ensure, Context};
    let value = port.or(server_port).unwrap_or_else(|| "9091".into());
    let port = value.parse::<u16>().context("PORT/SERVER_PORT must be a valid port number")?;
    ensure!(port != 0, "PORT/SERVER_PORT must be between 1 and 65535");
    Ok(port)
}

#[cfg(test)]
mod port_tests {
    use super::parse_server_port;

    #[test]
    fn railway_port_overrides_local_port() {
        assert_eq!(parse_server_port(Some("8080".into()), Some("9091".into())).unwrap(), 8080);
        assert_eq!(parse_server_port(None, Some("9092".into())).unwrap(), 9092);
        assert_eq!(parse_server_port(None, None).unwrap(), 9091);
        for value in ["", "invalid", "0", "65536"] {
            assert!(parse_server_port(Some(value.into()), Some("9091".into())).is_err());
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileTransferConfig {
    pub target_directory: String,
    pub roles: Vec<String>,
}

impl FileTransferConfig {
    fn from_env() -> Self {
        let target_directory = std::env::var("FILETRANSFER_TARGET_DIRECTORY")
            .ok()
            .or_else(|| std::env::var("filetransfer.targetDirectory").ok())
            .unwrap_or_else(|| r"C:\vmh\files\input".to_string());

        let roles_raw = std::env::var("FILETRANSFER_ROLES")
            .ok()
            .or_else(|| std::env::var("filetransfer.roles").ok())
            .unwrap_or_else(|| "Administration,Vito_Super".to_string());

        // ConfigService.setRoles(List<String>) — dipisah koma
        let roles = roles_raw
            .split(',')
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .collect();

        Self {
            target_directory,
            roles,
        }
    }

    /// ConfigService.hasRole(List<String> roles)
    pub fn has_role(&self, requested: &[String]) -> bool {
        if requested.is_empty() {
            return false;
        }
        requested.iter().any(|r| self.roles.iter().any(|c| c == r))
    }
}

static FILE_TRANSFER: OnceLock<FileTransferConfig> = OnceLock::new();

pub fn file_transfer() -> &'static FileTransferConfig {
    FILE_TRANSFER.get_or_init(FileTransferConfig::from_env)
}

/// Inisialisasi eksplisit (dipanggil main agar env sudah final).
pub fn init() {
    let _ = file_transfer();
}
