use std::env;
use std::fs;

use config::{Environment, File, FileFormat};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::error::AppResult;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub ollama: OllamaConfig,
    pub database: DatabaseConfig,
    pub session: SessionConfig,
    pub worker: WorkerConfig,
    pub messaging: MessagingConfig,
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
pub struct MessagingConfig {
    pub provider: MessagingProvider,
    pub telegram: TelegramConfig,
    pub slack: SlackConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessagingProvider {
    Telegram,
    Slack,
    None,
}

impl MessagingProvider {
    pub const fn channel_type(self) -> Option<crate::db::ChannelType> {
        match self {
            Self::Telegram => Some(crate::db::ChannelType::Telegram),
            Self::Slack => Some(crate::db::ChannelType::Slack),
            Self::None => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Slack => "slack",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub webhook_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SlackConfig {
    pub bot_token: String,
    pub app_token: String,
    #[serde(default)]
    pub allowed_channel_ids: Vec<String>,
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
    config.validate_messaging()?;

    Ok(config)
}

impl AppConfig {
    fn validate_messaging(&self) -> AppResult<()> {
        match self.messaging.provider {
            MessagingProvider::Telegram if self.messaging.telegram.bot_token.trim().is_empty() => {
                Err(config_error(
                    "messaging.telegram.bot_token is required when messaging.provider is telegram",
                ))
            }
            MessagingProvider::Slack
                if self.messaging.slack.bot_token.trim().is_empty()
                    || self.messaging.slack.app_token.trim().is_empty() =>
            {
                Err(config_error(
                    "messaging.slack.bot_token and messaging.slack.app_token are required when messaging.provider is slack",
                ))
            }
            _ => Ok(()),
        }
    }
}

fn config_error(message: impl Into<String>) -> crate::error::AppError {
    crate::error::AppError::Config(config::ConfigError::Message(message.into()))
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
            let expression = &remainder[..end];
            let (key, default) = expression
                .split_once(":-")
                .map_or((expression, None), |(key, default)| (key, Some(default)));
            if let Ok(value) = env::var(key) {
                output.push_str(&value);
            } else if let Some(default) = default {
                output.push_str(default);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_path_is_read_from_environment_in_base_config() {
        let raw = fs::read_to_string("config.yaml").expect("config.yaml");
        assert!(raw.contains("path: \"${DATABASE_PATH}\""));
    }

    #[test]
    fn messaging_env_names_are_read_from_base_config() {
        let raw = fs::read_to_string("config.yaml").expect("config.yaml");
        assert!(raw.contains("provider: \"${MESSAGING_PROVIDER}\""));
        assert!(raw.contains("bot_token: \"${MESSAGING_TELEGRAM_BOT_TOKEN:-}\""));
        assert!(raw.contains("bot_token: \"${MESSAGING_SLACK_BOT_TOKEN:-}\""));
        assert!(raw.contains("app_token: \"${MESSAGING_SLACK_APP_TOKEN:-}\""));
    }

    #[test]
    fn assistant_soul_prompt_path_exists_in_base_config() {
        let raw = fs::read_to_string("config.yaml").expect("config.yaml");
        assert!(raw.contains("assistant_soul_prompt_path:"));
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
    fn placeholder_default_allows_missing_optional_env() {
        let value = expand_env_placeholders("token=${FINELOR_TEST_OPTIONAL_ENV:-}")
            .expect("optional placeholder should expand");
        assert_eq!(value, "token=");
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
