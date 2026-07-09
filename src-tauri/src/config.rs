use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AppEnvironment {
    Development,
    Staging,
    Production,
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub environment: AppEnvironment,
}

pub fn load() -> AppConfig {
    let raw_environment = std::env::var("APP_ENV").unwrap_or_else(|_| "development".to_string());

    AppConfig {
        environment: AppEnvironment::from(raw_environment.as_str()),
    }
}

impl From<&str> for AppEnvironment {
    fn from(value: &str) -> Self {
        match value {
            "staging" => Self::Staging,
            "production" => Self::Production,
            _ => Self::Development,
        }
    }
}
