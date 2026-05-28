#[cfg(feature = "ssr")]
use crate::db::DbPool;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(feature = "ssr")]
use tower_sessions::Session;
#[cfg(feature = "ssr")]
use uuid::Uuid;

use crate::web::events::TelegramConnectStatus;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct CompanyChannel {
    pub id: i64,
    pub channel_type: String,
    pub channel_identifier: String,
    pub active: bool,
    pub display_name: Option<String>,
    pub telegram_username: Option<String>,
    pub telegram_chat_type: Option<String>,
    pub connected_by_user_id: Option<i64>,
    pub slack_channel_name: Option<String>,
    pub slack_channel_type: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct MessagingProviderStatus {
    pub active_provider: String,
    pub slack_bot_token_configured: bool,
    pub slack_app_token_configured: bool,
    pub slack_allowed_channel_ids_count: usize,
    pub slack_workspace_name: Option<String>,
    pub slack_workspace_id: Option<String>,
    pub slack_workspace_url: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SlackAllowedChannel {
    pub id: i64,
    pub channel_id: String,
    pub channel_name: String,
    pub channel_type: String,
    pub active: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SlackChannelVerification {
    pub channel_id: String,
    pub channel_name: String,
    pub channel_type: String,
    pub already_allowed: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TelegramConnectLink {
    pub url: String,
    pub bot_username: String,
    pub qr_svg: String,
    pub expires_at: i64,
    pub connect_id: String,
}

#[cfg(feature = "ssr")]
pub trait MessagingProviderService {
    fn active_provider(&self, config: &crate::config::AppConfig) -> &'static str;
}

#[cfg(feature = "ssr")]
pub struct DefaultMessagingProviderService;

#[cfg(feature = "ssr")]
impl MessagingProviderService for DefaultMessagingProviderService {
    fn active_provider(&self, config: &crate::config::AppConfig) -> &'static str {
        config.messaging.provider.as_str()
    }
}

#[cfg(feature = "ssr")]
pub fn pool() -> DbPool {
    crate::web::pool::get_pool()
}

#[cfg(feature = "ssr")]
pub async fn require_session_workspace_id(session: &Session) -> Result<Uuid, ServerFnError> {
    let user_id: Option<String> = session
        .get("user_id")
        .await
        .map_err(|e| ServerFnError::new(format!("session get user_id: {}", e)))?;
    if user_id.is_none() {
        return Err(ServerFnError::new("Not authenticated."));
    }
    Ok(crate::workspace::active_workspace_id())
}

#[cfg(feature = "ssr")]
pub async fn connect_telegram_for_workspace_with_metadata(
    pool: &DbPool,
    workspace_id: Uuid,
    chat_id: i64,
    metadata: Option<serde_json::Value>,
) -> Result<String, ServerFnError> {
    if workspace_id != crate::workspace::active_workspace_id() {
        return Err(ServerFnError::new("Workspace mismatch."));
    }

    let existing = crate::db::list_channel_identities(pool, None)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    if existing.iter().any(|c| {
        c.active && c.channel_type == "TELEGRAM" && c.channel_identifier == chat_id.to_string()
    }) {
        return Ok("Telegram channel already connected.".to_string());
    }

    crate::db::insert_channel_identity(pool, "TELEGRAM", &chat_id.to_string(), metadata)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to connect Telegram channel: {}", e)))?;

    Ok("Telegram channel connected.".to_string())
}

#[cfg(feature = "ssr")]
pub async fn connect_telegram_for_workspace(
    pool: &DbPool,
    workspace_id: Uuid,
    chat_id: i64,
) -> Result<String, ServerFnError> {
    connect_telegram_for_workspace_with_metadata(
        pool,
        workspace_id,
        chat_id,
        Some(serde_json::json!({ "source": "web_dashboard" })),
    )
    .await
}

#[cfg(feature = "ssr")]
fn metadata_string(metadata: Option<&serde_json::Value>, path: &[&str]) -> Option<String> {
    let mut value = metadata?;
    for key in path {
        value = value.get(*key)?;
    }
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(feature = "ssr")]
fn metadata_i64(metadata: Option<&serde_json::Value>, path: &[&str]) -> Option<i64> {
    metadata_string(metadata, path).and_then(|value| value.parse::<i64>().ok())
}

#[cfg(feature = "ssr")]
fn telegram_display_name(
    channel_identifier: &str,
    metadata: Option<&serde_json::Value>,
) -> Option<String> {
    metadata_string(metadata, &["chat", "display_name"])
        .or_else(|| metadata_string(metadata, &["chat", "title"]))
        .or_else(|| {
            let first_name = metadata_string(metadata, &["chat", "first_name"]);
            let last_name = metadata_string(metadata, &["chat", "last_name"]);
            match (first_name, last_name) {
                (Some(first), Some(last)) => Some(format!("{first} {last}")),
                (Some(first), None) => Some(first),
                _ => None,
            }
        })
        .or_else(|| {
            metadata_string(metadata, &["chat", "username"]).map(|value| format!("@{value}"))
        })
        .or_else(|| Some(format!("Telegram · {channel_identifier}")))
}

#[cfg(feature = "ssr")]
fn telegram_username(metadata: Option<&serde_json::Value>) -> Option<String> {
    metadata_string(metadata, &["chat", "username"])
        .or_else(|| metadata_string(metadata, &["user", "username"]))
        .map(|value| value.trim_start_matches('@').to_string())
}

#[server(ListCompanyChannels, "/api")]
pub async fn list_company_channels() -> Result<Vec<CompanyChannel>, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let mut channels = crate::db::list_channel_identities(&pool, None)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;
    let config =
        crate::config::load().map_err(|e| ServerFnError::new(format!("Config error: {}", e)))?;
    let slack_bot_token = config.messaging.slack.bot_token.trim().to_string();
    if !slack_bot_token.is_empty() {
        for channel in &mut channels {
            if channel.channel_type != "SLACK"
                || !channel.active
                || channel.channel_identifier.starts_with('D')
            {
                continue;
            }
            let metadata = channel
                .metadata
                .clone()
                .unwrap_or_else(|| serde_json::json!({}));
            let existing_name = metadata
                .get("channel_name")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if existing_name.is_some() {
                continue;
            }
            let Some(channel_name) = crate::integrations::slack::fetch_channel_name(
                &slack_bot_token,
                &channel.channel_identifier,
            )
            .await?
            else {
                continue;
            };
            let mut merged = metadata;
            if let Some(map) = merged.as_object_mut() {
                map.insert(
                    "channel_name".to_string(),
                    serde_json::Value::String(channel_name),
                );
            }
            sqlx::query("UPDATE channel_identities SET metadata = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1")
                .bind(channel.id)
                .bind(&merged)
                .execute(&pool)
                .await
                .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;
            channel.metadata = Some(merged);
        }
    }

    Ok(channels
        .into_iter()
        .map(|c| CompanyChannel {
            display_name: if c.channel_type == "TELEGRAM" {
                telegram_display_name(&c.channel_identifier, c.metadata.as_ref())
            } else {
                None
            },
            telegram_username: telegram_username(c.metadata.as_ref()),
            telegram_chat_type: metadata_string(c.metadata.as_ref(), &["chat", "type"]),
            connected_by_user_id: metadata_i64(c.metadata.as_ref(), &["connected_by_user_id"]),
            slack_channel_name: metadata_string(c.metadata.as_ref(), &["channel_name"]),
            slack_channel_type: metadata_string(c.metadata.as_ref(), &["channel_type"]),
            id: c.id,
            channel_type: c.channel_type,
            channel_identifier: c.channel_identifier,
            active: c.active,
        })
        .collect())
}

#[server(GetMessagingProviderStatus, "/api")]
pub async fn get_messaging_provider_status() -> Result<MessagingProviderStatus, ServerFnError> {
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;
    let config =
        crate::config::load().map_err(|e| ServerFnError::new(format!("Config error: {}", e)))?;
    let pool = pool();
    let provider_service = DefaultMessagingProviderService;
    let slack_bot_token_configured = !config.messaging.slack.bot_token.trim().is_empty();
    let slack_allowed_channel_ids_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM slack_allowed_channels WHERE active = TRUE",
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?
        as usize;
    let (slack_workspace_name, slack_workspace_id, slack_workspace_url) =
        if slack_bot_token_configured {
            crate::integrations::slack::fetch_workspace_info(&config.messaging.slack.bot_token)
                .await
                .unwrap_or((None, None, None))
        } else {
            (None, None, None)
        };

    Ok(MessagingProviderStatus {
        active_provider: provider_service.active_provider(&config).to_string(),
        slack_bot_token_configured,
        slack_app_token_configured: !config.messaging.slack.app_token.trim().is_empty(),
        slack_allowed_channel_ids_count,
        slack_workspace_name,
        slack_workspace_id,
        slack_workspace_url,
    })
}

#[server(ListSlackAllowedChannels, "/api")]
pub async fn list_slack_allowed_channels() -> Result<Vec<SlackAllowedChannel>, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let rows = sqlx::query_as::<_, (i64, String, String, String, bool)>(
        r#"
        SELECT id, channel_id, channel_name, channel_type, active
        FROM slack_allowed_channels
        WHERE active = TRUE
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    Ok(rows
        .into_iter()
        .map(
            |(id, channel_id, channel_name, channel_type, active)| SlackAllowedChannel {
                id,
                channel_id,
                channel_name,
                channel_type,
                active,
            },
        )
        .collect())
}

#[server(AddSlackAllowedChannel, "/api")]
pub async fn add_slack_allowed_channel(
    channel_name: String,
) -> Result<SlackAllowedChannel, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let normalized_name = channel_name.trim().trim_start_matches('#').to_string();
    if normalized_name.is_empty() {
        return Err(ServerFnError::new("Channel name is required."));
    }

    let config =
        crate::config::load().map_err(|e| ServerFnError::new(format!("Config error: {}", e)))?;
    let bot_token = config.messaging.slack.bot_token.trim().to_string();
    if bot_token.is_empty() {
        return Err(ServerFnError::new("Slack bot token is not configured."));
    }

    let Some(resolved) =
        crate::integrations::slack::resolve_channel_by_name(&bot_token, &normalized_name).await?
    else {
        return Err(ServerFnError::new(
            "Could not find that Slack channel, or the app does not have access to it.",
        ));
    };

    let row = sqlx::query_as::<_, (i64, String, String, String, bool)>(
        r#"
        INSERT INTO slack_allowed_channels (channel_id, channel_name, channel_type, team_id, active)
        VALUES ($1, $2, $3, $4, TRUE)
        ON CONFLICT(channel_id) DO UPDATE SET
          channel_name = excluded.channel_name,
          channel_type = excluded.channel_type,
          team_id = excluded.team_id,
          active = TRUE,
          updated_at = CURRENT_TIMESTAMP
        RETURNING id, channel_id, channel_name, channel_type, active
        "#,
    )
    .bind(&resolved.channel_id)
    .bind(&resolved.channel_name)
    .bind(&resolved.channel_type)
    .bind(resolved.team_id)
    .fetch_one(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    Ok(SlackAllowedChannel {
        id: row.0,
        channel_id: row.1,
        channel_name: row.2,
        channel_type: row.3,
        active: row.4,
    })
}

#[server(VerifySlackAllowedChannel, "/api")]
pub async fn verify_slack_allowed_channel(
    channel_name: String,
) -> Result<SlackChannelVerification, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let normalized_name = channel_name.trim().trim_start_matches('#').to_string();
    if normalized_name.is_empty() {
        return Err(ServerFnError::new("Channel name is required."));
    }

    let config =
        crate::config::load().map_err(|e| ServerFnError::new(format!("Config error: {}", e)))?;
    let bot_token = config.messaging.slack.bot_token.trim().to_string();
    if bot_token.is_empty() {
        return Err(ServerFnError::new("Slack bot token is not configured."));
    }

    let Some(resolved) =
        crate::integrations::slack::resolve_channel_by_name(&bot_token, &normalized_name).await?
    else {
        return Err(ServerFnError::new(
            "Could not find that Slack channel, or the app does not have access to it.",
        ));
    };

    let already_allowed = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(1) FROM slack_allowed_channels WHERE channel_id = $1 AND active = TRUE",
    )
    .bind(&resolved.channel_id)
    .fetch_one(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?
        > 0;

    Ok(SlackChannelVerification {
        channel_id: resolved.channel_id,
        channel_name: resolved.channel_name,
        channel_type: resolved.channel_type,
        already_allowed,
    })
}

#[server(RemoveSlackAllowedChannel, "/api")]
pub async fn remove_slack_allowed_channel(channel_id: String) -> Result<(), ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let trimmed = channel_id.trim();
    if trimmed.is_empty() {
        return Err(ServerFnError::new("Channel ID is required."));
    }

    let rows = sqlx::query(
        "UPDATE slack_allowed_channels SET active = FALSE, updated_at = CURRENT_TIMESTAMP WHERE channel_id = $1",
    )
    .bind(trimmed)
    .execute(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?
    .rows_affected();

    if rows == 0 {
        return Err(ServerFnError::new("Slack channel not found."));
    }
    Ok(())
}

#[server(GetTelegramChannelAvatar, "/api")]
pub async fn get_telegram_channel_avatar(channel_id: i64) -> Result<Option<String>, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let channel = crate::db::get_channel_identity(&pool, channel_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;
    let Some(channel) = channel else {
        return Ok(None);
    };
    if !channel.active || channel.channel_type != "TELEGRAM" {
        return Ok(None);
    }

    let app_config =
        crate::config::load().map_err(|e| ServerFnError::new(format!("Config error: {}", e)))?;
    let cache_key = format!("telegram_channel_avatar:{}", channel.id);
    let store = crate::web::pool::get_ephemeral_store();
    if let Some(cached) = store
        .get(&cache_key)
        .await
        .map_err(|e| ServerFnError::new(format!("Cache get error: {}", e)))?
    {
        return Ok(Some(cached));
    }

    let bot_token = configured_telegram_bot_token(&app_config)?;
    let Some(data_url) = crate::integrations::telegram::fetch_channel_avatar_data_url(
        &bot_token,
        &channel.channel_identifier,
        channel.metadata.as_ref(),
    )
    .await?
    else {
        return Ok(None);
    };

    store
        .set(&cache_key, &data_url, Some(60 * 60))
        .await
        .map_err(|e| ServerFnError::new(format!("Cache set error: {}", e)))?;
    Ok(Some(data_url))
}

#[server(DeleteCompanyChannel, "/api")]
pub async fn delete_company_channel(channel_id: i64) -> Result<(), ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let deleted = crate::db::delete_channel_identity(&pool, channel_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    if deleted {
        Ok(())
    } else {
        Err(ServerFnError::new("Channel connection not found."))
    }
}

#[server(ConnectTelegramChannel, "/api")]
pub async fn connect_telegram_channel(chat_id: String) -> Result<String, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    let workspace_id = require_session_workspace_id(&session).await?;

    let trimmed = chat_id.trim();
    if trimmed.is_empty() {
        return Err(ServerFnError::new("Telegram chat ID is required."));
    }
    let parsed = trimmed
        .parse::<i64>()
        .map_err(|_| ServerFnError::new("Telegram chat ID must be a valid integer."))?;

    connect_telegram_for_workspace(&pool, workspace_id, parsed).await
}

#[server(CreateTelegramConnectLink, "/api")]
pub async fn create_telegram_connect_link() -> Result<TelegramConnectLink, ServerFnError> {
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    let workspace_id = require_session_workspace_id(&session).await?;
    let user_id: Option<String> = session
        .get("user_id")
        .await
        .map_err(|e| ServerFnError::new(format!("session get user_id: {}", e)))?;
    let user_id = user_id
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| ServerFnError::new("Not authenticated."))?;

    let app_config =
        crate::config::load().map_err(|e| ServerFnError::new(format!("Config error: {}", e)))?;
    let bot_token = configured_telegram_bot_token(&app_config)?;
    crate::integrations::telegram::verify_delivery_ready(
        &bot_token,
        app_config.messaging.telegram.webhook_url.as_deref(),
    )
    .await?;

    let ttl_seconds = 15 * 60;
    let store = crate::web::pool::get_ephemeral_store();
    let connect_session = crate::web::events::create_telegram_connect_session(
        &store,
        workspace_id,
        user_id,
        ttl_seconds as usize,
    )
    .await
    .map_err(|e| ServerFnError::new(format!("Telegram connect session error: {}", e)))?;
    let username = crate::integrations::telegram::fetch_bot_username(&bot_token).await?;
    let expires_at = chrono::Utc::now().timestamp() + ttl_seconds;
    let url = format!("https://t.me/{}?start={}", username, connect_session.token);
    let qr_svg = qrcode::QrCode::new(url.as_bytes())
        .map_err(|e| ServerFnError::new(format!("QR code generation failed: {}", e)))?
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(192, 192)
        .quiet_zone(true)
        .dark_color(qrcode::render::svg::Color("#0f172a"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();

    Ok(TelegramConnectLink {
        url,
        bot_username: username,
        qr_svg,
        expires_at,
        connect_id: connect_session.connect_id,
    })
}

#[server(GetTelegramConnectStatus, "/api")]
pub async fn get_telegram_connect_status(
    connect_id: String,
) -> Result<TelegramConnectStatus, ServerFnError> {
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;
    let store = crate::web::pool::get_ephemeral_store();
    crate::web::events::get_telegram_connect_status(&store, &connect_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Telegram connection status error: {}", e)))
}

#[cfg(feature = "ssr")]
fn configured_telegram_bot_token(
    app_config: &crate::config::AppConfig,
) -> Result<String, ServerFnError> {
    let env_token = std::env::var("MESSAGING_TELEGRAM_BOT_TOKEN").unwrap_or_default();
    let token = if env_token.trim().is_empty() {
        app_config.messaging.telegram.bot_token.clone()
    } else {
        env_token
    };
    if token.trim().is_empty() || token.contains("placeholder") || token.contains("${") {
        Err(ServerFnError::new("Telegram bot token is not configured."))
    } else {
        Ok(token)
    }
}
