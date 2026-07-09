use serde::Serialize;

use crate::config::{self, AppEnvironment};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppHealth {
    status: &'static str,
    version: &'static str,
    environment: AppEnvironment,
}

#[tauri::command]
pub fn get_app_health() -> AppHealth {
    AppHealth {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        environment: config::load().environment,
    }
}
