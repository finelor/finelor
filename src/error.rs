use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("configuration error: {0}")]
    Config(#[from] config::ConfigError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("telegram request error: {0}")]
    Telegram(#[from] teloxide::RequestError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("uuid error: {0}")]
    Uuid(#[from] uuid::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ollama api error: {0}")]
    Ollama(String),
    #[error("image processing error: {0}")]
    ImageProcessing(String),
    #[error("agent processing error: {0}")]
    Agent(String),
    #[error("queue error: {0}")]
    Queue(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("zip error: {0}")]
    Zip(String),
    #[error("skill error: {0}")]
    Skill(String),
}

impl From<zip::result::ZipError> for AppError {
    fn from(err: zip::result::ZipError) -> Self {
        AppError::Zip(err.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
