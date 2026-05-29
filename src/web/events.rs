use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppEvent {
    pub id: Uuid,
    #[serde(alias = "workspace_id")]
    pub workspace_id: Uuid,
    pub event_type: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

impl AppEvent {
    pub fn new(workspace_id: Uuid, event_type: impl Into<String>, payload: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            workspace_id,
            event_type: event_type.into(),
            payload,
            created_at: Utc::now(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TelegramConnectStatusKind {
    Pending,
    Connected,
    AlreadyConnected,
    Failed,
    Expired,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TelegramConnectStatus {
    pub connect_id: String,
    pub status: TelegramConnectStatusKind,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TelegramConnectSession {
    pub token: String,
    pub connect_id: String,
    #[serde(alias = "workspace_id")]
    pub workspace_id: Uuid,
    pub user_id: i64,
    pub expires_at: i64,
    pub status: TelegramConnectStatusKind,
    pub message: Option<String>,
}

impl TelegramConnectSession {
    pub fn new(
        token: impl Into<String>,
        connect_id: impl Into<String>,
        workspace_id: Uuid,
        user_id: i64,
        expires_at: i64,
    ) -> Self {
        Self {
            token: token.into(),
            connect_id: connect_id.into(),
            workspace_id,
            user_id,
            expires_at,
            status: TelegramConnectStatusKind::Pending,
            message: None,
        }
    }

    pub fn status_payload(&self) -> TelegramConnectStatus {
        TelegramConnectStatus {
            connect_id: self.connect_id.clone(),
            status: self.status.clone(),
            message: self.message.clone(),
        }
    }
}

impl TelegramConnectStatus {
    pub fn new(connect_id: impl Into<String>, status: TelegramConnectStatusKind) -> Self {
        Self {
            connect_id: connect_id.into(),
            status,
            message: None,
        }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

#[cfg(feature = "ssr")]
#[derive(Clone)]
pub struct AppEventBus {
    sender: tokio::sync::broadcast::Sender<AppEvent>,
}

#[cfg(feature = "ssr")]
impl AppEventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = tokio::sync::broadcast::channel(capacity);
        Self { sender }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AppEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: AppEvent) {
        let _ = self.sender.send(event);
    }
}

#[cfg(feature = "ssr")]
fn telegram_connect_status_key(connect_id: &str) -> String {
    format!("telegram_connect:status:{connect_id}")
}

#[cfg(feature = "ssr")]
fn telegram_connect_token_key(token: &str) -> String {
    format!("telegram_connect:token:{token}")
}

#[cfg(feature = "ssr")]
pub fn new_telegram_connect_token() -> String {
    use base64::Engine;

    let bytes = *Uuid::new_v4().as_bytes();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(feature = "ssr")]
pub async fn create_telegram_connect_session(
    store: &crate::kv::EphemeralStore,
    workspace_id: Uuid,
    user_id: i64,
    ttl_seconds: usize,
) -> crate::error::AppResult<TelegramConnectSession> {
    let token = new_telegram_connect_token();
    let connect_id = crate::integrations::telegram::connect_token_fingerprint(&token);
    let expires_at = chrono::Utc::now().timestamp() + ttl_seconds as i64;
    let session = TelegramConnectSession::new(token, connect_id, workspace_id, user_id, expires_at);
    store_telegram_connect_session(store, &session, ttl_seconds).await?;
    Ok(session)
}

#[cfg(feature = "ssr")]
pub async fn store_telegram_connect_session(
    store: &crate::kv::EphemeralStore,
    session: &TelegramConnectSession,
    ttl_seconds: usize,
) -> crate::error::AppResult<()> {
    let value = serde_json::to_string(session)?;
    let status_value = serde_json::to_string(&session.status_payload())?;
    store
        .set(
            &telegram_connect_token_key(&session.token),
            &value,
            Some(ttl_seconds as u64),
        )
        .await?;
    store
        .set(
            &telegram_connect_status_key(&session.connect_id),
            &status_value,
            Some(ttl_seconds as u64),
        )
        .await?;

    Ok(())
}

#[cfg(feature = "ssr")]
pub async fn get_telegram_connect_session(
    store: &crate::kv::EphemeralStore,
    token: &str,
) -> crate::error::AppResult<Option<TelegramConnectSession>> {
    let value = store.get(&telegram_connect_token_key(token)).await?;

    Ok(value.and_then(|value| serde_json::from_str::<TelegramConnectSession>(&value).ok()))
}

#[cfg(feature = "ssr")]
pub async fn update_telegram_connect_session_status(
    store: &crate::kv::EphemeralStore,
    mut session: TelegramConnectSession,
    status: TelegramConnectStatusKind,
    message: Option<String>,
    ttl_seconds: usize,
) -> crate::error::AppResult<TelegramConnectSession> {
    session.status = status;
    session.message = message;
    store_telegram_connect_session(store, &session, ttl_seconds).await?;
    Ok(session)
}

#[cfg(feature = "ssr")]
pub async fn set_telegram_connect_status(
    store: &crate::kv::EphemeralStore,
    status: &TelegramConnectStatus,
    ttl_seconds: usize,
) -> crate::error::AppResult<()> {
    let value = serde_json::to_string(status)?;
    store
        .set(
            &telegram_connect_status_key(&status.connect_id),
            &value,
            Some(ttl_seconds as u64),
        )
        .await?;

    Ok(())
}

#[cfg(feature = "ssr")]
pub async fn get_telegram_connect_status(
    store: &crate::kv::EphemeralStore,
    connect_id: &str,
) -> crate::error::AppResult<TelegramConnectStatus> {
    let value = store.get(&telegram_connect_status_key(connect_id)).await?;

    Ok(value
        .and_then(|value| serde_json::from_str::<TelegramConnectStatus>(&value).ok())
        .unwrap_or_else(|| {
            TelegramConnectStatus::new(connect_id, TelegramConnectStatusKind::Expired)
        }))
}
