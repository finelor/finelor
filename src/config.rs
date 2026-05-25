use std::env;
use std::fs;

use config::{Environment, File, FileFormat};
use serde::{Deserialize, Deserializer};
use serde_json::Value as JsonValue;

use crate::error::AppResult;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub ollama: OllamaConfig,
    pub database: DatabaseConfig,
    pub session: SessionConfig,
    pub worker: WorkerConfig,
    pub web: WebConfig,
    pub telegram: TelegramConfig,
    pub upload: UploadConfig,
    pub export: ExportConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub models: OllamaModels,
    pub vision_prompt_path: String,
    pub accountant_prompt_path: String,
    pub assistant_soul_prompt_path: String,
    pub timeout_seconds: u64,
    pub max_retries: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaModels {
    pub vision: String,
    pub accountant: String,
    pub assistant: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub path: String,
    pub max_connections: u32,
}

impl DatabaseConfig {
    pub fn validate(&self) -> AppResult<()> {
        if self.path.trim().is_empty() {
            return Err(crate::error::AppError::Config(
                config::ConfigError::Message("database path must not be empty".to_string()),
            ));
        }

        Ok(())
    }

    pub fn url(&self) -> String {
        let path = self.path.trim();
        if path.starts_with("sqlite:") {
            return path.to_string();
        }
        if path == ":memory:" {
            return "sqlite::memory:".to_string();
        }
        format!("sqlite://{}", path)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    pub secret: String,
    pub expiry_days: i64,
    pub cleanup_interval_seconds: u64,
    pub secure: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkerConfig {
    pub max_job_retries: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebConfig {
    #[serde(deserialize_with = "deserialize_string_list")]
    pub allowed_hosts: Vec<String>,
    #[serde(deserialize_with = "deserialize_string_list")]
    pub allowed_origins: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub webhook_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UploadConfig {
    pub storage_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExportConfig {
    pub sie4_encoding: String,
    pub download_expiry_hours: u64,
    pub company_org_nr: String,
    pub company_name: String,
    pub fiscal_year_start: String,
    pub fiscal_year_end: String,
    pub output_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
    pub format: String,
}

pub fn load() -> AppResult<AppConfig> {
    let base_raw = fs::read_to_string("config.yaml")?;
    let mut builder =
        config::Config::builder().add_source(File::from_str(&base_raw, FileFormat::Yaml));

    if let Ok(local_raw) = fs::read_to_string("config.local.yaml") {
        builder = builder.add_source(File::from_str(&local_raw, FileFormat::Yaml));
    }

    let settings = builder
        .add_source(Environment::default().separator("__"))
        .build()?;
    let mut expanded: JsonValue = settings.try_deserialize()?;
    expand_env_placeholders_in_value(&mut expanded)?;
    let config: AppConfig = serde_json::from_value(expanded)
        .map_err(|e| crate::error::AppError::Config(config::ConfigError::Message(e.to_string())))?;
    config.database.validate()?;

    Ok(config)
}

fn expand_env_placeholders_in_value(value: &mut JsonValue) -> AppResult<()> {
    match value {
        JsonValue::String(s) => {
            *s = expand_env_placeholders(s)?;
            Ok(())
        }
        JsonValue::Array(items) => {
            for item in items {
                expand_env_placeholders_in_value(item)?;
            }
            Ok(())
        }
        JsonValue::Object(map) => {
            for item in map.values_mut() {
                expand_env_placeholders_in_value(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn expand_env_placeholders(input: &str) -> AppResult<String> {
    let mut output = String::with_capacity(input.len());
    let mut cursor = input;

    while let Some(start) = cursor.find("${") {
        output.push_str(&cursor[..start]);
        let remainder = &cursor[start + 2..];

        if let Some(end) = remainder.find('}') {
            let key = &remainder[..end];
            if let Ok(value) = env::var(key) {
                output.push_str(&value);
            } else {
                return Err(crate::error::AppError::Config(
                    config::ConfigError::Message(format!(
                        "missing required environment variable '{}' referenced by configuration",
                        key
                    )),
                ));
            }
            cursor = &remainder[end + 1..];
        } else {
            output.push_str(&cursor[start..]);
            cursor = "";
            break;
        }
    }

    output.push_str(cursor);
    Ok(output)
}

fn deserialize_string_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    match value {
        JsonValue::Array(items) => items
            .into_iter()
            .map(|item| match item {
                JsonValue::String(value) => Ok(value),
                other => Err(serde::de::Error::custom(format!(
                    "expected string list item, got {other}"
                ))),
            })
            .collect(),
        JsonValue::String(value) => Ok(value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect()),
        other => Err(serde::de::Error::custom(format!(
            "expected string or string list, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_path_is_read_from_environment_in_base_config() {
        let raw = fs::read_to_string("config.yaml").expect("config.yaml");
        assert!(raw.contains("path: \"${DATABASE_PATH}\""));
    }

    #[test]
    fn assistant_soul_prompt_path_exists_in_base_config() {
        let raw = fs::read_to_string("config.yaml").expect("config.yaml");
        assert!(raw.contains("assistant_soul_prompt_path:"));
    }

    #[test]
    fn web_security_defaults_exist_in_base_config() {
        let raw = fs::read_to_string("config.yaml").expect("config.yaml");
        assert!(raw.contains("web:"));
        assert!(raw.contains("allowed_hosts:"));
        assert!(raw.contains("allowed_origins:"));
        assert!(raw.contains("${WEB_ALLOWED_HOSTS}"));
        assert!(raw.contains("${WEB_ALLOWED_ORIGINS}"));
    }

    #[test]
    fn string_list_fields_accept_comma_separated_values() {
        let value = serde_json::json!({
            "allowed_hosts": "finelor.example,www.finelor.example",
            "allowed_origins": "https://finelor.example, https://www.finelor.example"
        });

        let config: WebConfig = serde_json::from_value(value).expect("deserialize web config");

        assert_eq!(
            config.allowed_hosts,
            vec!["finelor.example", "www.finelor.example"]
        );
        assert_eq!(
            config.allowed_origins,
            vec!["https://finelor.example", "https://www.finelor.example"]
        );
    }

    #[test]
    fn unresolved_placeholders_return_missing_env_name() {
        let err = expand_env_placeholders("database:\n  path: \"${FINELOR_TEST_MISSING_ENV}\"")
            .expect_err("placeholder should fail");
        assert!(
            err.to_string().contains("FINELOR_TEST_MISSING_ENV"),
            "error should name the missing env var: {err}"
        );
    }

    #[test]
    fn merged_config_value_without_placeholder_does_not_require_missing_env() {
        let mut merged = serde_json::json!({
            "database": {
                "path": "sqlite://override.db"
            }
        });

        expand_env_placeholders_in_value(&mut merged)
            .expect("override should not require missing env");
        assert_eq!(
            merged["database"]["path"].as_str(),
            Some("sqlite://override.db")
        );
    }

    #[test]
    fn unresolved_placeholders_in_merged_values_fail_with_env_name() {
        let mut merged = serde_json::json!({
            "database": {
                "path": "${FINELOR_TEST_MISSING_ENV_AFTER_MERGE}"
            }
        });

        let err = expand_env_placeholders_in_value(&mut merged)
            .expect_err("placeholder should fail after merge");
        assert!(
            err.to_string()
                .contains("FINELOR_TEST_MISSING_ENV_AFTER_MERGE"),
            "error should name the missing env var: {err}"
        );
    }

    #[test]
    fn database_url_accepts_plain_paths_and_sqlite_urls() {
        let config = DatabaseConfig {
            path: "data/finelor.db".to_string(),
            max_connections: 5,
        };
        assert_eq!(config.url(), "sqlite://data/finelor.db");

        let config = DatabaseConfig {
            path: " sqlite://data/existing.db ".to_string(),
            max_connections: 5,
        };
        assert_eq!(config.url(), "sqlite://data/existing.db");
    }

    #[test]
    fn database_url_maps_memory_path_to_sqlite_memory_url() {
        let config = DatabaseConfig {
            path: ":memory:".to_string(),
            max_connections: 1,
        };

        assert_eq!(config.url(), "sqlite::memory:");
    }

    #[test]
    fn database_validation_rejects_empty_path() {
        let config = DatabaseConfig {
            path: "   ".to_string(),
            max_connections: 1,
        };

        let err = config.validate().expect_err("empty path should fail");
        assert!(err.to_string().contains("database path must not be empty"));
    }
}
