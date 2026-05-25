#[cfg(feature = "ssr")]
use crate::db::DbPool;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(feature = "ssr")]
use tower_sessions::Session;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct McpKeySummary {
    pub id: i64,
    pub name: String,
    pub key_prefix: String,
    pub capabilities: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct CreatedMcpKeyResponse {
    pub key: McpKeySummary,
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
fn capabilities_from_json(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

#[cfg(feature = "ssr")]
fn summary_from_record(record: crate::db::McpKey) -> McpKeySummary {
    McpKeySummary {
        id: record.id,
        name: record.name,
        key_prefix: record.key_prefix,
        capabilities: capabilities_from_json(&record.capabilities),
        created_at: record.created_at,
        last_used_at: record.last_used_at,
        revoked_at: record.revoked_at,
    }
}

#[server(ListMcpKeys, "/_server_fn")]
pub async fn list_mcp_keys() -> Result<Vec<McpKeySummary>, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_user_id(&session).await?;

    crate::db::list_visible_mcp_keys(&pool)
        .await
        .map(|keys| keys.into_iter().map(summary_from_record).collect())
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))
}

#[server(CreateMcpKey, "/_server_fn")]
pub async fn create_mcp_key(name: String) -> Result<CreatedMcpKeyResponse, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    let user_id = require_session_user_id(&session).await?;

    let name = name.trim();
    if name.is_empty() {
        return Err(ServerFnError::new("MCP key name is required."));
    }
    if name.len() > 80 {
        return Err(ServerFnError::new(
            "MCP key name must be 80 characters or fewer.",
        ));
    }

    let created = crate::db::create_mcp_key(&pool, name, user_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    Ok(CreatedMcpKeyResponse {
        key: summary_from_record(created.record),
        token: created.token,
    })
}

#[server(RevokeMcpKey, "/_server_fn")]
pub async fn revoke_mcp_key(mcp_key_id: i64) -> Result<(), ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_user_id(&session).await?;

    let revoked = crate::db::revoke_mcp_key(&pool, mcp_key_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;
    if revoked {
        Ok(())
    } else {
        Err(ServerFnError::new("MCP key not found."))
    }
}

#[server(UnrevokeMcpKey, "/_server_fn")]
pub async fn unrevoke_mcp_key(mcp_key_id: i64) -> Result<(), ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_user_id(&session).await?;

    let unrevoked = crate::db::unrevoke_mcp_key(&pool, mcp_key_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;
    if unrevoked {
        Ok(())
    } else {
        Err(ServerFnError::new("MCP key not found."))
    }
}

#[server(RevealMcpKey, "/_server_fn")]
pub async fn reveal_mcp_key(mcp_key_id: i64) -> Result<String, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_user_id(&session).await?;

    crate::db::reveal_mcp_key_token(&pool, mcp_key_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?
        .ok_or_else(|| ServerFnError::new("MCP key not found."))
}

#[server(RemoveMcpKey, "/_server_fn")]
pub async fn remove_mcp_key(mcp_key_id: i64) -> Result<(), ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_user_id(&session).await?;

    let hidden = crate::db::hide_mcp_key(&pool, mcp_key_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;
    if hidden {
        Ok(())
    } else {
        Err(ServerFnError::new("MCP key not found."))
    }
}
