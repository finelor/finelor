#![allow(dead_code)]
// Test lane rules:
// - Unit tests: no DB/router.
// - Integration tests: prefer per-test `in_memory_pool()`.
// - Server-fn/router/session tests: use `TestContext::server_fn()` to get
//   per-test isolated DB + router/session harness.
// Runtime code should use `finelor::db::DbPool`; tests may use `sqlx::SqlitePool`
// directly for local fixtures and helper ergonomics.

use async_trait::async_trait;
use finelor::error::{AppError, AppResult};
use finelor::inference::{
    ChatJsonRequest, ImageJsonRequest, InferenceProvider, ModelChatResponse, ModelTextResponse,
    ToolChatRequest,
};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tower_sessions::{
    Expiry, SessionManagerLayer,
    cookie::{Key, SameSite},
};
use tower_sessions_sqlx_store::SqliteStore;

pub async fn in_memory_pool() -> sqlx::SqlitePool {
    let options = SqliteConnectOptions::new()
        .in_memory(true)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("connect in-memory sqlite");

    finelor::db::run_migrations(&pool)
        .await
        .expect("run migrations");

    pool
}

pub struct FileBackedDb {
    path: PathBuf,
    pool: sqlx::SqlitePool,
}

impl FileBackedDb {
    pub async fn new() -> Self {
        let path = std::env::temp_dir().join(format!("finelor-it-{}.db", uuid::Uuid::new_v4()));
        let url = format!("sqlite://{}", path.display());
        let options = SqliteConnectOptions::from_str(&url)
            .expect("parse sqlite url")
            .foreign_keys(true)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .expect("connect file-backed sqlite");

        finelor::db::run_migrations(&pool)
            .await
            .expect("run migrations");

        Self { path, pool }
    }

    pub fn pool(&self) -> sqlx::SqlitePool {
        self.pool.clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn reopen_pool(&self) -> sqlx::SqlitePool {
        let url = format!("sqlite://{}", self.path.display());
        let options = SqliteConnectOptions::from_str(&url)
            .expect("parse sqlite url")
            .foreign_keys(true)
            .create_if_missing(true);
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .expect("reopen file-backed sqlite")
    }

    pub async fn cleanup(self) {
        self.pool.close().await;
        cleanup_sqlite_artifacts(&self.path);
    }
}

fn cleanup_sqlite_artifacts(path: &Path) {
    let _ = std::fs::remove_file(path);
    let wal = format!("{}-wal", path.display());
    let shm = format!("{}-shm", path.display());
    let _ = std::fs::remove_file(wal);
    let _ = std::fs::remove_file(shm);
}

pub fn test_config() -> finelor::config::AppConfig {
    let unique = uuid::Uuid::new_v4();
    let storage_path = std::env::temp_dir()
        .join(format!("finelor-test-uploads-{unique}"))
        .to_string_lossy()
        .to_string();

    finelor::config::AppConfig {
        ollama: finelor::config::OllamaConfig {
            base_url: "http://127.0.0.1:11434".to_string(),
            api_key: None,
            models: finelor::config::OllamaModels {
                vision: "test-vision".to_string(),
                accountant: "test-accountant".to_string(),
                assistant: "test-agent-chat".to_string(),
            },
            vision_prompt_path: "assets/prompts/sweden/vision_agent.md".to_string(),
            accountant_prompt_path: "assets/prompts/sweden/accountant_agent.md".to_string(),
            assistant_soul_prompt_path: "assets/prompts/_shared/assistant_agent_soul.md"
                .to_string(),
            timeout_seconds: 1,
            max_retries: 0,
            initial_backoff_ms: 1,
            max_backoff_seconds: 1,
        },
        database: finelor::config::DatabaseConfig {
            path: ":memory:".to_string(),
            max_connections: 1,
        },
        session: finelor::config::SessionConfig {
            secret: "test-session-secret-must-be-at-least-sixty-four-characters-long-0001"
                .to_string(),
            expiry_days: 30,
            cleanup_interval_seconds: 60,
            secure: false,
        },
        worker: finelor::config::WorkerConfig { max_job_retries: 1 },
        messaging: finelor::config::MessagingConfig {
            provider: finelor::config::MessagingProvider::Telegram,
            telegram: finelor::config::TelegramConfig {
                bot_token: "test-token".to_string(),
                webhook_url: None,
            },
            slack: finelor::config::SlackConfig {
                bot_token: String::new(),
                app_token: String::new(),
            },
        },
        upload: finelor::config::UploadConfig { storage_path },
        export: finelor::config::ExportConfig {
            sie4_encoding: "UTF-8".to_string(),
            download_expiry_hours: 24,
            company_org_nr: "000000-0000".to_string(),
            company_name: "Test Company".to_string(),
            fiscal_year_start: "2026-01-01".to_string(),
            fiscal_year_end: "2026-12-31".to_string(),
            output_path: std::env::temp_dir()
                .join(format!("finelor-test-exports-{unique}"))
                .to_string_lossy()
                .to_string(),
        },
        logging: finelor::config::LoggingConfig {
            level: "info".to_string(),
            format: "json".to_string(),
        },
    }
}

pub fn test_queue() -> Arc<finelor::queue::InMemoryJobQueue> {
    Arc::new(finelor::queue::InMemoryJobQueue::with_poll_interval(
        Duration::from_millis(1),
    ))
}

pub async fn drain_jobs(
    queue: &Arc<finelor::queue::InMemoryJobQueue>,
    batch_size: usize,
) -> Vec<(String, finelor::queue::Job)> {
    use finelor::queue::JobQueue;

    queue
        .poll("test-worker", batch_size)
        .await
        .expect("poll test queue")
}

#[derive(Clone, Default)]
pub struct FakeInferenceProvider {
    image_json: Arc<std::sync::Mutex<VecDeque<AppResult<ModelTextResponse>>>>,
    chat_json: Arc<std::sync::Mutex<VecDeque<AppResult<ModelChatResponse>>>>,
}

impl FakeInferenceProvider {
    pub fn push_image_json(&self, response: serde_json::Value) {
        self.image_json
            .lock()
            .expect("fake inference mutex")
            .push_back(Ok(ModelTextResponse {
                response: response.to_string(),
                eval_count: Some(1),
            }));
    }

    pub fn push_chat_json(&self, response: serde_json::Value) {
        self.chat_json
            .lock()
            .expect("fake inference mutex")
            .push_back(Ok(ModelChatResponse {
                content: response.to_string(),
                thinking: Some("test thinking".to_string()),
                tool_calls: None,
                eval_count: Some(1),
            }));
    }

    pub fn push_image_error(&self, error: &str) {
        self.image_json
            .lock()
            .expect("fake inference mutex")
            .push_back(Err(AppError::Ollama(error.to_string())));
    }
}

#[async_trait]
impl InferenceProvider for FakeInferenceProvider {
    async fn generate_image_json(
        &self,
        _request: ImageJsonRequest,
    ) -> AppResult<ModelTextResponse> {
        self.image_json
            .lock()
            .expect("fake inference mutex")
            .pop_front()
            .unwrap_or_else(|| Err(AppError::Ollama("missing fake image response".to_string())))
    }

    async fn chat_json(&self, _request: ChatJsonRequest) -> AppResult<ModelChatResponse> {
        self.chat_json
            .lock()
            .expect("fake inference mutex")
            .pop_front()
            .unwrap_or_else(|| Err(AppError::Ollama("missing fake chat response".to_string())))
    }

    async fn chat_tools(&self, _request: ToolChatRequest) -> AppResult<ModelChatResponse> {
        Err(AppError::Ollama("unexpected fake tool call".to_string()))
    }
}

pub struct TestContext {
    pool: sqlx::SqlitePool,
    _pool_override: finelor::web::pool::TestPoolOverrideGuard,
}

impl TestContext {
    pub async fn server_fn() -> Self {
        let pool = in_memory_pool().await;
        Self::server_fn_from_pool(pool).await
    }

    pub async fn server_fn_from_pool(pool: sqlx::SqlitePool) -> Self {
        let pool_override = finelor::web::pool::override_pool_for_test(pool.clone()).await;
        assert_clean_signup_state(&pool).await;
        Self {
            pool,
            _pool_override: pool_override,
        }
    }

    pub fn pool(&self) -> sqlx::SqlitePool {
        self.pool.clone()
    }

    pub async fn spawn_server_fn_app(
        &self,
        session_secret: &str,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let store = SqliteStore::new(self.pool.clone());
        store
            .migrate()
            .await
            .expect("migrate sqlite session store for test context");

        let session_layer = SessionManagerLayer::new(store)
            .with_name("finelor.sid")
            .with_http_only(true)
            .with_same_site(SameSite::Lax)
            .with_secure(false)
            .with_expiry(Expiry::OnInactivity(time::Duration::days(30)))
            .with_always_save(true)
            .with_signed(Key::from(session_secret.as_bytes()));
        let app = axum::Router::new()
            .route(
                "/api/{*fn_name}",
                axum::routing::post(leptos_axum::handle_server_fns),
            )
            .layer(session_layer);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (addr, server)
    }
}

// TODO: remove after all legacy callers migrate to `TestContext::server_fn()`.
#[allow(dead_code)]
pub async fn server_fn_pool() -> TestContext {
    TestContext::server_fn().await
}

pub async fn assert_clean_signup_state(pool: &sqlx::SqlitePool) {
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .expect("assert clean signup state: count users");
    assert_eq!(
        user_count, 0,
        "signup tests require empty users table before request"
    );

    let row = sqlx::query(
        "SELECT display_name, org_nr, jurisdiction FROM company_profile WHERE singleton = TRUE",
    )
    .fetch_one(pool)
    .await
    .expect("assert clean signup state: company_profile singleton");

    let display_name: Option<String> = row
        .try_get("display_name")
        .expect("assert clean signup state: display_name");
    let org_nr: Option<String> = row
        .try_get("org_nr")
        .expect("assert clean signup state: org_nr");
    let jurisdiction: String = row
        .try_get("jurisdiction")
        .expect("assert clean signup state: jurisdiction");

    assert!(
        display_name.is_none(),
        "company_profile.display_name must be NULL at test start"
    );
    assert!(
        org_nr.is_none(),
        "company_profile.org_nr must be NULL at test start"
    );
    assert_eq!(
        jurisdiction, "SE",
        "company_profile.jurisdiction must be reset to SE at test start"
    );
}
