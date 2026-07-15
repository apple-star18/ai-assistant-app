use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tauri::{AppHandle, Manager};

const DIAGNOSTIC_LOG_FILE: &str = "diagnostics.log";
const MAX_LOG_BYTES: u64 = 512 * 1024;
static LOG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticLogSnapshot {
    path: String,
    contents: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCommandError {
    code: &'static str,
    message: String,
}

type CommandResult<T> = Result<T, DiagnosticCommandError>;

pub fn setup(app: &AppHandle) {
    record(
        app,
        "INFO",
        "app",
        &format!(
            "Diagnostic session started version={} os={}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS
        ),
    );
}

pub fn record(app: &AppHandle, level: &str, area: &str, message: &str) {
    let Ok(_guard) = LOG_LOCK.lock() else {
        return;
    };
    let Ok(path) = diagnostic_log_path(app) else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    if path
        .metadata()
        .map(|metadata| metadata.len() >= MAX_LOG_BYTES)
        .unwrap_or(false)
    {
        let rotated_path = path.with_extension("log.old");
        let _ = fs::remove_file(&rotated_path);
        let _ = fs::rename(&path, rotated_path);
    }

    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let level = clean_field(level);
    let area = clean_field(area);
    let message = clean_field(message);
    let line = format!("{timestamp_ms} [{level}] {area}: {message}\n");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

#[tauri::command]
pub fn diagnostics_get_log(app: AppHandle) -> CommandResult<DiagnosticLogSnapshot> {
    let _guard = LOG_LOCK.lock().map_err(|_| state_error())?;
    let path = diagnostic_log_path(&app).map_err(storage_error)?;
    let contents = if path.exists() {
        fs::read_to_string(&path).map_err(|error| storage_error(error.to_string()))?
    } else {
        String::new()
    };
    Ok(DiagnosticLogSnapshot {
        path: path.display().to_string(),
        contents,
    })
}

#[tauri::command]
pub fn diagnostics_clear_log(app: AppHandle) -> CommandResult<DiagnosticLogSnapshot> {
    let _guard = LOG_LOCK.lock().map_err(|_| state_error())?;
    let path = diagnostic_log_path(&app).map_err(storage_error)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| storage_error(error.to_string()))?;
    }
    fs::write(&path, "").map_err(|error| storage_error(error.to_string()))?;
    Ok(DiagnosticLogSnapshot {
        path: path.display().to_string(),
        contents: String::new(),
    })
}

fn diagnostic_log_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join(DIAGNOSTIC_LOG_FILE))
        .map_err(|error| format!("Failed to resolve diagnostic log directory: {error}"))
}

fn clean_field(value: &str) -> String {
    value
        .replace(['\r', '\n'], " ")
        .chars()
        .take(2_000)
        .collect()
}

fn state_error() -> DiagnosticCommandError {
    DiagnosticCommandError {
        code: "state_unavailable",
        message: "Diagnostic log is temporarily unavailable.".to_string(),
    }
}

fn storage_error(message: String) -> DiagnosticCommandError {
    DiagnosticCommandError {
        code: "storage_error",
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::clean_field;

    #[test]
    fn log_fields_are_kept_on_one_line() {
        assert_eq!(clean_field("timeout\r\nretry"), "timeout  retry");
    }
}
