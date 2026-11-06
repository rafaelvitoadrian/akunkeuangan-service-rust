//! Native terminal + optional append-only file logging.
use std::{fs::File, io::{self, Write}, sync::{Arc, Mutex}};

#[derive(Clone)]
struct LogWriter(Option<Arc<Mutex<File>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        io::stdout().lock().write_all(bytes)?;
        if let Some(file) = &self.0 {
            file.lock().map_err(|_| io::Error::other("log file lock poisoned"))?.write_all(bytes)?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().lock().flush()?;
        if let Some(file) = &self.0 {
            file.lock().map_err(|_| io::Error::other("log file lock poisoned"))?.flush()?;
        }
        Ok(())
    }
}

pub fn init() -> anyhow::Result<()> {
    let log_path = std::env::var("APP_LOG_FILE").ok().filter(|p| !p.is_empty());
    let file = if let Some(path) = &log_path {
        let path = std::path::Path::new(path);
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        Some(Arc::new(Mutex::new(std::fs::OpenOptions::new()
            .create(true).append(true).open(path)?)))
    } else { None };
    let writer = LogWriter(file);
    tracing_subscriber::fmt().with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info".into()))
        .with_writer(move || writer.clone()).init();
    if let Some(path) = log_path { tracing::info!(path, "file logging enabled"); }
    Ok(())
}
