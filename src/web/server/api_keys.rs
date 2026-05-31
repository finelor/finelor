#[cfg(feature = "ssr")]
use crate::db::DbPool;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(feature = "ssr")]
use tower_sessions::Session;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ApiKeySummary {
    pub id: i64,
    pub name: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct CreatedApiKeyResponse {
    pub key: ApiKeySummary,
    pub token: String,
}

#[cfg(feature = "ssr")]
fn pool() -> DbPool {
    crate::web::pool::get_pool()
}

#[cfg(feature = "ssr")]
async fn require_session_user_id(session: &Session) -> Result<i64, ServerFnError> {
    let user_id: Option<String> = session
        .get("user_id")
        .await
        .map_err(|e| ServerFnError::new(format!("session get user_id: {}", e)))?;
    let Some(user_id) = user_id else {
        return Err(ServerFnError::new("Not authenticated."));
    };
    user_id
        .parse::<i64>()
        .map_err(|e| ServerFnError::new(format!("Invalid session: {}", e)))
}

#[cfg(feature = "ssr")]
fn summary_from_record(record: crate::db::ApiKey) -> ApiKeySummary {
    ApiKeySummary {
        id: record.id,
        name: record.name,
        key_prefix: record.key_prefix,
        created_at: record.created_at,
        last_used_at: record.last_used_at,
        revoked_at: record.revoked_at,
    }
}

#[server(ListApiKeys, "/_server_fn")]
pub async fn list_api_keys() -> Result<Vec<ApiKeySummary>, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_user_id(&session).await?;

    crate::db::list_visible_api_keys(&pool)
        .await
        .map(|keys| keys.into_iter().map(summary_from_record).collect())
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))
}

#[server(CreateApiKey, "/_server_fn")]
pub async fn create_api_key(name: String) -> Result<CreatedApiKeyResponse, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    let user_id = require_session_user_id(&session).await?;

    let name = name.trim();
    if name.is_empty() {
        return Err(ServerFnError::new("API key name is required."));
    }
    if name.len() > 80 {
        return Err(ServerFnError::new(
            "API key name must be 80 characters or fewer.",
        ));
    }

    let created = crate::db::create_api_key(&pool, name, user_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    Ok(CreatedApiKeyResponse {
        key: summary_from_record(created.record),
        token: created.token,
    })
}

#[server(RevokeApiKey, "/_server_fn")]
pub async fn revoke_api_key(api_key_id: i64) -> Result<(), ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_user_id(&session).await?;

    let revoked = crate::db::revoke_api_key(&pool, api_key_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;
    if revoked {
        Ok(())
    } else {
        Err(ServerFnError::new("API key not found."))
    }
}

#[server(UnrevokeApiKey, "/_server_fn")]
pub async fn unrevoke_api_key(api_key_id: i64) -> Result<(), ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_user_id(&session).await?;

    let unrevoked = crate::db::unrevoke_api_key(&pool, api_key_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;
    if unrevoked {
        Ok(())
    } else {
        Err(ServerFnError::new("API key not found."))
    }
}

#[server(RevealApiKey, "/_server_fn")]
pub async fn reveal_api_key(api_key_id: i64) -> Result<String, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_user_id(&session).await?;

    crate::db::reveal_api_key_token(&pool, api_key_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?
        .ok_or_else(|| ServerFnError::new("API key not found."))
}

#[server(RemoveApiKey, "/_server_fn")]
pub async fn remove_api_key(api_key_id: i64) -> Result<(), ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_user_id(&session).await?;

    let hidden = crate::db::hide_api_key(&pool, api_key_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;
    if hidden {
        Ok(())
    } else {
        Err(ServerFnError::new("API key not found."))
    }
}
