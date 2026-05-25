use std::collections::HashSet;
use std::future::pending;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::{convert::Infallible, pin::Pin, time::Duration};

use anyhow::Context;
use axum::{
    Json, Router,
    body::Body,
    extract::FromRef,
    extract::Path as AxumPath,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{
        Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use finelor::db::DbPool;
use leptos::config::{LeptosOptions, get_configuration};
use serde_json::json;
use sqlx::Row;
use teloxide::prelude::*;
use teloxide::types::Update;
use tokio::signal;
use tokio::sync::Mutex;
use tokio_stream::{Stream, StreamExt, wrappers::BroadcastStream};
use tower_http::services::ServeDir;
use tower_sessions::{
    Expiry, SessionManagerLayer,
    cookie::{Key, SameSite},
    session_store::ExpiredDeletion,
};
use tower_sessions_sqlx_store::SqliteStore;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use finelor::web::pool::{set_ephemeral_store, set_pool};

use finelor::agents::AgentContext;
use finelor::config::{self, AppConfig};
use finelor::db;
use finelor::messaging::AgentGatewayState;
use finelor::orchestration::{JobProcessor, recover_incomplete_jobs};
use finelor::queue::{QueueProducer, create_in_memory_queue};
use finelor::web::events::{AppEvent, AppEventBus};
use finelor::workspace::ensure_single_workspace;

#[derive(Clone)]
struct AppState {
    config: Arc<AppConfig>,
    pool: DbPool,
    queue_producer: QueueProducer,
    ephemeral_store: finelor::kv::EphemeralStore,
    leptos_options: LeptosOptions,
    events: AppEventBus,
    running_agent_sessions: Arc<Mutex<HashSet<String>>>,
    active_document_interaction_sessions: Arc<Mutex<HashSet<String>>>,
}

impl FromRef<AppState> for LeptosOptions {
    fn from_ref(state: &AppState) -> Self {
        state.leptos_options.clone()
    }
}

impl FromRef<AppState> for finelor::api::PublicApiState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            config: state.config.clone(),
            pool: state.pool.clone(),
            queue_producer: state.queue_producer.clone(),
            events: state.events.clone(),
        }
    }
}

fn agent_gateway_state(state: &AppState) -> AgentGatewayState {
    AgentGatewayState {
        config: state.config.clone(),
        pool: state.pool.clone(),
        queue_producer: state.queue_producer.clone(),
        ephemeral_store: state.ephemeral_store.clone(),
        events: state.events.clone(),
        running_sessions: state.running_agent_sessions.clone(),
        active_document_interaction_sessions: state.active_document_interaction_sessions.clone(),
    }
}

fn spawn_gateway_event_dispatcher(bot: Bot, state: AppState) {
    let gateway_state = agent_gateway_state(&state);
    tokio::spawn(async move {
        let dispatcher = finelor::messaging::dispatch::GatewayEventDispatcher::new(gateway_state)
            .with_adapter(
                "TELEGRAM",
                Arc::new(finelor::messaging::adapters::telegram::TelegramOutboundAdapter::new(bot)),
            );

        if let Err(err) = dispatcher.run().await {
            error!(error = %err, "Gateway event dispatcher stopped");
        }
    });
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let config = Arc::new(config::load().context("failed to load configuration")?);
    init_tracing(&config);
    ensure_database_parent_writable(config.database.path.as_str(), "database.path")
        .context("database path parent directory is not writable")?;
    ensure_runtime_storage_writable(config.upload.storage_path.as_str(), "upload.storage_path")
        .context("upload storage path is not writable")?;
    ensure_runtime_storage_writable(config.export.output_path.as_str(), "export.output_path")
        .context("export output path is not writable")?;

    info!(
        application = "Finelor",
        version = %env!("CARGO_PKG_VERSION"),
        upload_storage_path = %config.upload.storage_path,
        export_output_path = %config.export.output_path,
        "starting finelor foundation services"
    );

    let pool = db::create_pool(&config.database)
        .await
        .context("failed to connect to sqlite")?;
    db::run_migrations(&pool)
        .await
        .context("failed to run sqlite migrations")?;
    db::ping(&pool).await.context("database ping failed")?;
    let _ = db::fetch_health_row(&pool).await?;
    ensure_single_workspace(&pool)
        .await
        .context("failed to ensure single-workspace row")?;

    let queue = create_in_memory_queue();
    let ephemeral_store = finelor::kv::EphemeralStore::new();
    let queue_producer = QueueProducer::new(Arc::new(queue));

    info!("database and queue connections verified");
    let events = AppEventBus::new(256);
    let agent_context = AgentContext::new(pool.clone(), config.as_ref().clone(), events.clone());

    set_pool(pool.clone());
    set_ephemeral_store(ephemeral_store.clone());

    let session_store = SqliteStore::new(pool.clone());
    session_store
        .migrate()
        .await
        .context("failed to migrate sqlite session store")?;
    let session_cleanup_interval =
        std::time::Duration::from_secs(config.session.cleanup_interval_seconds);
    let _session_cleanup_task = tokio::spawn(
        session_store
            .clone()
            .continuously_delete_expired(session_cleanup_interval),
    );
    let session_layer = SessionManagerLayer::new(session_store)
        .with_name("finelor.sid")
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(config.session.secure)
        .with_expiry(Expiry::OnInactivity(time::Duration::days(
            config.session.expiry_days,
        )))
        .with_always_save(true)
        .with_signed(session_cookie_key(config.session.secret.as_str())?);

    let app_state = AppState {
        config: config.clone(),
        pool: pool.clone(),
        queue_producer: queue_producer.clone(),
        ephemeral_store,
        leptos_options: build_leptos_options()?,
        events,
        running_agent_sessions: Arc::new(Mutex::new(HashSet::new())),
        active_document_interaction_sessions: Arc::new(Mutex::new(HashSet::new())),
    };

    let api = build_router(&app_state)
        .layer(session_layer)
        .with_state(app_state.clone());
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .context("failed to bind HTTP listener")?;

    let bot_task = if should_disable_bot(config.telegram.bot_token.as_str()) {
        warn!("Telegram bot disabled: placeholder or unresolved TELEGRAM_BOT_TOKEN");
        tokio::spawn(async { pending::<anyhow::Result<()>>().await })
    } else if config
        .telegram
        .webhook_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
    {
        let bot = Bot::new(config.telegram.bot_token.clone());
        finelor::messaging::adapters::telegram::register_telegram_command_menu(&bot).await?;
        spawn_gateway_event_dispatcher(bot.clone(), app_state.clone());
        configure_telegram_webhook(config.as_ref()).await?;
        info!("Telegram webhook delivery configured; long polling disabled");
        tokio::spawn(async { pending::<anyhow::Result<()>>().await })
    } else {
        let bot = Bot::new(config.telegram.bot_token.clone());
        let bot_state = app_state.clone();
        tokio::spawn(run_bot(bot, bot_state))
    };
    let processor_context = agent_context.clone();
    recover_incomplete_jobs(&pool, &queue_producer)
        .await
        .context("failed to recover incomplete document pipeline jobs")?;
    let job_processor_task = tokio::spawn(async move {
        let processor = JobProcessor::new(processor_context, queue_producer.clone());
        processor.run().await.context("job processor failed")
    });
    let api_task = tokio::spawn(async move {
        axum::serve(listener, api)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .context("axum server failed")
    });

    info!("services started successfully");

    tokio::select! {
        result = bot_task => {
            result.context("bot task join failed")??;
        }
        result = api_task => {
            result.context("api task join failed")??;
        }
        result = job_processor_task => {
            result.context("job processor task join failed")??;
        }
    }

    Ok(())
}

fn ensure_runtime_storage_writable(path: &str, label: &str) -> anyhow::Result<()> {
    let dir = Path::new(path);
    std::fs::create_dir_all(dir).with_context(|| format!("create dir {} ({})", path, label))?;

    let probe = dir.join(".finelor_write_probe");
    let mut file =
        std::fs::File::create(&probe).with_context(|| format!("create {}", probe.display()))?;
    file.write_all(b"ok")
        .with_context(|| format!("write {}", probe.display()))?;
    std::fs::remove_file(&probe).with_context(|| format!("remove {}", probe.display()))?;

    Ok(())
}

fn ensure_database_parent_writable(path: &str, label: &str) -> anyhow::Result<()> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == ":memory:" || trimmed == "sqlite::memory:" {
        return Ok(());
    }

    let db_path = if let Some(rest) = trimmed.strip_prefix("sqlite://") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("sqlite:") {
        rest
    } else {
        trimmed
    };

    let path = Path::new(db_path);
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }

    ensure_runtime_storage_writable(&parent.to_string_lossy(), &format!("{}.parent", label))
}

fn init_tracing(config: &AppConfig) {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.logging.level.clone()));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .json()
        .init();
}

fn should_disable_bot(token: &str) -> bool {
    let trimmed = token.trim();
    trimmed.is_empty() || trimmed.contains("placeholder") || trimmed.contains("${")
}

fn telegram_webhook_secret(config: &AppConfig) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(config.telegram.bot_token.as_bytes());
    hasher.update(b":");
    hasher.update(config.session.secret.as_bytes());
    hex::encode(hasher.finalize())
}

async fn configure_telegram_webhook(config: &AppConfig) -> anyhow::Result<()> {
    let Some(webhook_url) = config
        .telegram
        .webhook_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        anyhow::bail!("telegram.webhook_url is required for webhook mode");
    };

    let client = reqwest::Client::new();
    let set_url = format!(
        "https://api.telegram.org/bot{}/setWebhook",
        config.telegram.bot_token
    );
    let response = client
        .post(set_url)
        .json(&json!({
            "url": webhook_url,
            "allowed_updates": ["message", "callback_query"],
            "drop_pending_updates": false,
            "secret_token": telegram_webhook_secret(config),
        }))
        .send()
        .await
        .context("Telegram setWebhook request failed")?;
    if !response.status().is_success() {
        anyhow::bail!("Telegram setWebhook returned {}", response.status());
    }

    let actual_url = fetch_telegram_webhook_url(config.telegram.bot_token.as_str()).await?;
    if actual_url != webhook_url {
        anyhow::bail!(
            "Telegram webhook verification failed: expected configured URL, got '{}'",
            if actual_url.is_empty() {
                "<empty>"
            } else {
                "<different URL>"
            }
        );
    }

    info!("Telegram webhook configured and verified");
    Ok(())
}

async fn clear_telegram_webhook_for_polling(bot: &Bot, bot_token: &str) -> anyhow::Result<()> {
    info!("clearing Telegram webhook before starting long polling");
    bot.delete_webhook()
        .send()
        .await
        .context("failed to delete Telegram webhook before polling")?;

    let actual_url = fetch_telegram_webhook_url(bot_token).await?;
    if !actual_url.is_empty() {
        anyhow::bail!("Telegram webhook is still configured after deleteWebhook");
    }

    info!("Telegram webhook cleared and verified; starting long polling");
    Ok(())
}

async fn fetch_telegram_webhook_url(bot_token: &str) -> anyhow::Result<String> {
    #[derive(serde::Deserialize)]
    struct WebhookInfoResponse {
        ok: bool,
        result: Option<WebhookInfo>,
    }

    #[derive(serde::Deserialize)]
    struct WebhookInfo {
        url: String,
    }

    let response = reqwest::Client::new()
        .get(format!(
            "https://api.telegram.org/bot{}/getWebhookInfo",
            bot_token
        ))
        .send()
        .await
        .context("Telegram getWebhookInfo request failed")?;
    if !response.status().is_success() {
        anyhow::bail!("Telegram getWebhookInfo returned {}", response.status());
    }

    let body = response
        .json::<WebhookInfoResponse>()
        .await
        .context("Telegram getWebhookInfo parse failed")?;
    if !body.ok {
        anyhow::bail!("Telegram getWebhookInfo returned ok=false");
    }

    Ok(body.result.map(|info| info.url).unwrap_or_default())
}

fn session_cookie_key(secret: &str) -> anyhow::Result<Key> {
    let secret = secret.trim();
    if secret.is_empty() || secret.contains("${") || secret.len() < 64 {
        anyhow::bail!(
            "SESSION_SECRET must be set to at least 64 random characters for signed session cookies"
        );
    }

    Ok(Key::from(secret.as_bytes()))
}

fn build_router(state: &AppState) -> Router<AppState> {
    let site_root = state.leptos_options.site_root.to_string();
    let site_pkg_dir = state.leptos_options.site_pkg_dir.to_string();
    let pkg_path = std::path::Path::new(&site_root).join(&site_pkg_dir);
    let pkg_route = format!("/{}", site_pkg_dir.trim_matches('/'));

    let router = Router::new()
        .route("/favicon.ico", get(favicon_handler))
        .nest_service(pkg_route.as_str(), ServeDir::new(pkg_path))
        .route("/health", get(health))
        .route("/_events", get(events_handler))
        .route("/webhooks/telegram", post(telegram_webhook_handler))
        .route(
            "/_server_fn/{*fn_name}",
            post(leptos_axum::handle_server_fns),
        )
        .route(
            "/_documents/{short_ref}/image",
            get(get_document_image_handler),
        )
        .merge(finelor::mcp::router(state.pool.clone()).with_state::<AppState>(()))
        .nest("/api/v1", finelor::api::router());

    finelor::web::mount_web_router(router, state)
}

async fn favicon_handler() -> Response {
    const FAVICON: &[u8] = include_bytes!("../public/favicon.ico");
    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "image/x-icon")
        .body(Body::from(FAVICON.to_vec()))
        .unwrap_or_else(|_| axum::response::Response::new(Body::empty()))
}

async fn events_handler(
    State(state): State<AppState>,
    session: tower_sessions::Session,
) -> Result<impl axum::response::IntoResponse, (StatusCode, String)> {
    require_authenticated_session(&session).await?;
    info!("SSE client connected");
    let sync = AppEvent::new(finelor::workspace::active_workspace_id(), "sync", json!({}));
    let initial = tokio_stream::iter([Ok(sse_event(sync))]);
    let company_events =
        BroadcastStream::new(state.events.subscribe()).filter_map(move |event| match event {
            Ok(event) => Some(Ok(sse_event(event))),
            _ => None,
        });
    let stream: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(initial.chain(company_events));

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

fn sse_event(event: AppEvent) -> Event {
    Event::default()
        .id(event.id.to_string())
        .json_data(event)
        .unwrap_or_else(|_| {
            Event::default()
                .event("error")
                .data("event serialization failed")
        })
}

async fn telegram_webhook_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(update): Json<Update>,
) -> Result<StatusCode, (StatusCode, String)> {
    verify_telegram_webhook_secret(&state, &headers)?;

    let bot = Bot::new(state.config.telegram.bot_token.clone());
    finelor::messaging::adapters::telegram::handle_telegram_update(
        bot,
        update,
        agent_gateway_state(&state),
    )
    .await
    .map_err(|err| {
        error!(error = %err, "Telegram webhook update handling failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Telegram update handling failed".to_string(),
        )
    })?;

    Ok(StatusCode::OK)
}

fn verify_telegram_webhook_secret(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, String)> {
    let expected = telegram_webhook_secret(state.config.as_ref());
    let actual = headers
        .get("X-Telegram-Bot-Api-Secret-Token")
        .and_then(|value| value.to_str().ok());

    if actual == Some(expected.as_str()) {
        Ok(())
    } else {
        warn!("Rejected Telegram webhook request with invalid secret");
        Err((
            StatusCode::UNAUTHORIZED,
            "Invalid Telegram webhook secret".to_string(),
        ))
    }
}

fn build_leptos_options() -> anyhow::Result<LeptosOptions> {
    Ok(get_configuration(None)
        .context("failed to load Leptos configuration")?
        .leptos_options)
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let db_status = match db::ping(&state.pool).await {
        Ok(_) => "ok",
        Err(_) => "error",
    };

    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "database": db_status,
        "service": "Finelor",
    }))
}

async fn get_document_image_handler(
    State(state): State<AppState>,
    AxumPath(short_ref): AxumPath<String>,
    session: tower_sessions::Session,
) -> Result<Response, (axum::http::StatusCode, String)> {
    require_authenticated_session(&session).await?;
    let row =
        sqlx::query("SELECT original_path, mime_type FROM documents WHERE short_ref = $1 LIMIT 1")
            .bind(short_ref.trim())
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some(row) = row else {
        return Err((
            axum::http::StatusCode::NOT_FOUND,
            "Document not found".to_string(),
        ));
    };

    let original_path: String = row
        .try_get("original_path")
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mime_type: Option<String> = row.try_get("mime_type").ok();

    let upload_root = std::path::PathBuf::from(&state.config.upload.storage_path);
    let upload_root = std::fs::canonicalize(&upload_root).map_err(|_| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Invalid upload root".to_string(),
        )
    })?;
    let file_path = std::fs::canonicalize(&original_path).map_err(|_| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Image file not found".to_string(),
        )
    })?;

    if !file_path.starts_with(&upload_root) {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            "Access denied".to_string(),
        ));
    }

    let content = tokio::fs::read(&file_path).await.map_err(|_| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Image file not found".to_string(),
        )
    })?;

    let content_type = mime_type
        .filter(|m| m.starts_with("image/"))
        .unwrap_or_else(|| "application/octet-stream".to_string());

    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .body(Body::from(content))
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn require_authenticated_session(
    session: &tower_sessions::Session,
) -> Result<(), (StatusCode, String)> {
    let user_id: Option<String> = session
        .get("user_id")
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if user_id.is_some() {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "Not authenticated".to_string()))
    }
}

async fn run_bot(bot: Bot, state: AppState) -> anyhow::Result<()> {
    use teloxide::dispatching::{Dispatcher, UpdateFilterExt};

    clear_telegram_webhook_for_polling(&bot, state.config.telegram.bot_token.as_str()).await?;
    finelor::messaging::adapters::telegram::register_telegram_command_menu(&bot).await?;
    spawn_gateway_event_dispatcher(bot.clone(), state.clone());

    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .endpoint(finelor::messaging::adapters::telegram::handle_telegram_message),
        )
        .branch(
            Update::filter_callback_query()
                .endpoint(finelor::messaging::adapters::telegram::handle_telegram_callback_query),
        );

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![agent_gateway_state(&state)])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

async fn shutdown_signal() {
    if let Err(error_value) = signal::ctrl_c().await {
        error!(error = %error_value, "failed to listen for shutdown signal");
    }
}
