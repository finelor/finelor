use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow, SqliteSynchronous,
};
use sqlx::{Executor, FromRow, SqlitePool};
use std::str::FromStr;

use crate::config::DatabaseConfig;
use crate::error::AppResult;

pub type DbPool = SqlitePool;
pub type DbRow = SqliteRow;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const API_KEY_TOKEN_PREFIX: &str = "finelor_api_";
const MCP_KEY_TOKEN_PREFIX: &str = "finelor_mcp_";
pub const DEFAULT_MCP_CAPABILITIES: &str = r#"["documents:read","documents:explain"]"#;

pub async fn create_pool(config: &DatabaseConfig) -> AppResult<DbPool> {
    let options = SqliteConnectOptions::from_str(&config.url())?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);

    let pool = SqlitePoolOptions::new()
        .max_connections(config.max_connections)
        .connect_with(options)
        .await?;

    Ok(pool)
}

pub async fn ping(pool: &DbPool) -> AppResult<()> {
    pool.execute("SELECT 1").await?;
    Ok(())
}

pub async fn run_migrations(pool: &DbPool) -> AppResult<()> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

pub async fn fetch_health_row(pool: &DbPool) -> AppResult<DbRow> {
    let row = sqlx::query("SELECT 1 AS ok").fetch_one(pool).await?;
    Ok(row)
}

#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: i64,
    pub role: String,
    pub email: Option<String>,
    pub password_hash: Option<String>,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct ApiKey {
    pub id: i64,
    pub name: String,
    pub token: String,
    pub key_prefix: String,
    pub created_by_user_id: i64,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub hidden_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CreatedApiKey {
    pub record: ApiKey,
    pub token: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct McpKey {
    pub id: i64,
    pub name: String,
    pub token: String,
    pub key_prefix: String,
    pub key_hash: String,
    pub capabilities: String,
    pub created_by_user_id: i64,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub hidden_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CreatedMcpKey {
    pub record: McpKey,
    pub token: String,
}

pub fn hash_api_key(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn api_key_prefix(token: &str) -> String {
    token.chars().take(16).collect()
}

pub fn generate_api_key_token() -> String {
    use base64::Engine;

    let mut bytes = [0_u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    format!(
        "{}{}",
        API_KEY_TOKEN_PREFIX,
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

pub fn generate_mcp_key_token() -> String {
    use base64::Engine;

    let mut bytes = [0_u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    format!(
        "{}{}",
        MCP_KEY_TOKEN_PREFIX,
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

pub async fn create_api_key(
    pool: &DbPool,
    name: &str,
    created_by_user_id: i64,
) -> Result<CreatedApiKey, sqlx::Error> {
    let token = generate_api_key_token();
    let key_hash = hash_api_key(&token);
    let key_prefix = api_key_prefix(&token);
    let record = sqlx::query_as::<_, ApiKey>(
        r#"
        INSERT INTO api_keys (name, token, key_prefix, key_hash, created_by_user_id)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, name, token, key_prefix, created_by_user_id, last_used_at, revoked_at, hidden_at, created_at, updated_at
        "#,
    )
    .bind(name)
    .bind(&token)
    .bind(&key_prefix)
    .bind(&key_hash)
    .bind(created_by_user_id)
    .fetch_one(pool)
    .await?;

    Ok(CreatedApiKey { record, token })
}

pub async fn list_visible_api_keys(pool: &DbPool) -> Result<Vec<ApiKey>, sqlx::Error> {
    sqlx::query_as::<_, ApiKey>(
        r#"
        SELECT id, name, token, key_prefix, created_by_user_id, last_used_at, revoked_at, hidden_at, created_at, updated_at
        FROM api_keys
        WHERE hidden_at IS NULL
        ORDER BY created_at DESC, id DESC
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn reveal_api_key_token(
    pool: &DbPool,
    api_key_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT token
        FROM api_keys
        WHERE id = $1 AND hidden_at IS NULL
        LIMIT 1
        "#,
    )
    .bind(api_key_id)
    .fetch_optional(pool)
    .await
}

pub async fn revoke_api_key(pool: &DbPool, api_key_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE api_keys
        SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND hidden_at IS NULL
        "#,
    )
    .bind(api_key_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn unrevoke_api_key(pool: &DbPool, api_key_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE api_keys
        SET revoked_at = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND hidden_at IS NULL
        "#,
    )
    .bind(api_key_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn hide_api_key(pool: &DbPool, api_key_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE api_keys
        SET hidden_at = COALESCE(hidden_at, CURRENT_TIMESTAMP),
            revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        "#,
    )
    .bind(api_key_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn authenticate_api_key(
    pool: &DbPool,
    token: &str,
) -> Result<Option<ApiKey>, sqlx::Error> {
    let key_hash = hash_api_key(token);
    let mut tx = pool.begin().await?;
    let key = sqlx::query_as::<_, ApiKey>(
        r#"
        SELECT id, name, token, key_prefix, created_by_user_id, last_used_at, revoked_at, hidden_at, created_at, updated_at
        FROM api_keys
        WHERE key_hash = $1 AND revoked_at IS NULL AND hidden_at IS NULL
        LIMIT 1
        "#,
    )
    .bind(key_hash)
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(ref key) = key {
        sqlx::query(
            r#"
            UPDATE api_keys
            SET last_used_at = CURRENT_TIMESTAMP,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            "#,
        )
        .bind(key.id)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(key)
}

pub async fn create_mcp_key(
    pool: &DbPool,
    name: &str,
    created_by_user_id: i64,
) -> Result<CreatedMcpKey, sqlx::Error> {
    let token = generate_mcp_key_token();
    let key_hash = hash_api_key(&token);
    let key_prefix = api_key_prefix(&token);
    let record = sqlx::query_as::<_, McpKey>(
        r#"
        INSERT INTO mcp_keys (name, token, key_prefix, key_hash, capabilities, created_by_user_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, name, token, key_prefix, key_hash, capabilities, created_by_user_id, last_used_at, revoked_at, hidden_at, created_at, updated_at
        "#,
    )
    .bind(name)
    .bind(&token)
    .bind(&key_prefix)
    .bind(&key_hash)
    .bind(DEFAULT_MCP_CAPABILITIES)
    .bind(created_by_user_id)
    .fetch_one(pool)
    .await?;

    Ok(CreatedMcpKey { record, token })
}

pub async fn list_visible_mcp_keys(pool: &DbPool) -> Result<Vec<McpKey>, sqlx::Error> {
    sqlx::query_as::<_, McpKey>(
        r#"
        SELECT id, name, token, key_prefix, key_hash, capabilities, created_by_user_id, last_used_at, revoked_at, hidden_at, created_at, updated_at
        FROM mcp_keys
        WHERE hidden_at IS NULL
        ORDER BY created_at DESC, id DESC
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn reveal_mcp_key_token(
    pool: &DbPool,
    mcp_key_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT token
        FROM mcp_keys
        WHERE id = $1 AND hidden_at IS NULL
        LIMIT 1
        "#,
    )
    .bind(mcp_key_id)
    .fetch_optional(pool)
    .await
}

pub async fn revoke_mcp_key(pool: &DbPool, mcp_key_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE mcp_keys
        SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND hidden_at IS NULL
        "#,
    )
    .bind(mcp_key_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn unrevoke_mcp_key(pool: &DbPool, mcp_key_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE mcp_keys
        SET revoked_at = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND hidden_at IS NULL
        "#,
    )
    .bind(mcp_key_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn hide_mcp_key(pool: &DbPool, mcp_key_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE mcp_keys
        SET hidden_at = COALESCE(hidden_at, CURRENT_TIMESTAMP),
            revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        "#,
    )
    .bind(mcp_key_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn authenticate_mcp_key(
    pool: &DbPool,
    token: &str,
) -> Result<Option<McpKey>, sqlx::Error> {
    let key_hash = hash_api_key(token);
    let mut tx = pool.begin().await?;
    let key = sqlx::query_as::<_, McpKey>(
        r#"
        SELECT id, name, token, key_prefix, key_hash, capabilities, created_by_user_id, last_used_at, revoked_at, hidden_at, created_at, updated_at
        FROM mcp_keys
        WHERE key_hash = $1 AND revoked_at IS NULL AND hidden_at IS NULL
        LIMIT 1
        "#,
    )
    .bind(key_hash)
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(ref key) = key {
        sqlx::query(
            r#"
            UPDATE mcp_keys
            SET last_used_at = CURRENT_TIMESTAMP,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            "#,
        )
        .bind(key.id)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(key)
}

/// Canonical channel source types supported by Finelor.
///
/// Each variant maps to the string value stored in `channel_identities.channel_type`.
/// Adding a variant here does not automatically register it in the DB — schema
/// and ingestion lookup are the authoritative source of truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChannelType {
    /// Telegram chat — identified by numeric chat ID.
    #[serde(rename = "TELEGRAM")]
    Telegram,
    /// Slack conversation — identified by Slack channel ID.
    #[serde(rename = "SLACK")]
    Slack,
    /// WhatsApp chat — identified by phone number.
    #[serde(rename = "WHATSAPP")]
    Whatsapp,
    /// Generic HTTP webhook.
    #[serde(rename = "WEBHOOK")]
    Webhook,
    /// Generic HTTP API.
    #[serde(rename = "API")]
    Api,
    /// Email ingestion.
    #[serde(rename = "EMAIL")]
    Email,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_api_keys_use_api_prefix() {
        let token = generate_api_key_token();

        assert!(token.starts_with("finelor_api_"));
    }

    #[test]
    fn generated_mcp_keys_keep_mcp_prefix() {
        let token = generate_mcp_key_token();

        assert!(token.starts_with("finelor_mcp_"));
    }
}

impl ChannelType {
    /// Parse a string slice into a `ChannelType`, case-insensitive, trimming whitespace.
    /// Returns `None` for unknown values.
    pub fn parse_canonical(s: &str) -> Option<Self> {
        let normalized = s.trim().to_uppercase();
        match normalized.as_str() {
            "TELEGRAM" => Some(Self::Telegram),
            "SLACK" => Some(Self::Slack),
            "WHATSAPP" => Some(Self::Whatsapp),
            "WEBHOOK" => Some(Self::Webhook),
            "API" => Some(Self::Api),
            "EMAIL" => Some(Self::Email),
            _ => None,
        }
    }

    /// Return the canonical string representation stored in the database.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "TELEGRAM",
            Self::Slack => "SLACK",
            Self::Whatsapp => "WHATSAPP",
            Self::Webhook => "WEBHOOK",
            Self::Api => "API",
            Self::Email => "EMAIL",
        }
    }
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for ChannelType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_canonical(s).ok_or_else(|| format!("unknown channel type: '{}'", s))
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct ChannelIdentity {
    pub id: i64,
    pub channel_type: String,
    pub channel_identifier: String,
    pub metadata: Option<serde_json::Value>,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn insert_channel_identity(
    pool: &DbPool,
    channel_type: &str,
    channel_identifier: &str,
    metadata: Option<serde_json::Value>,
) -> Result<ChannelIdentity, sqlx::Error> {
    let canonical = ChannelType::parse_canonical(channel_type)
        .map(|ct| ct.as_str().to_string())
        .unwrap_or_else(|| channel_type.trim().to_uppercase());

    let row = sqlx::query_as::<_, ChannelIdentity>(
        r#"
        INSERT INTO channel_identities (channel_type, channel_identifier, metadata)
        VALUES ($1, $2, $3)
        RETURNING *
        "#,
    )
    .bind(&canonical)
    .bind(channel_identifier)
    .bind(metadata)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn list_channel_identities(
    pool: &DbPool,
    filter_channel_type: Option<&str>,
) -> Result<Vec<ChannelIdentity>, sqlx::Error> {
    let canonical_channel_type = filter_channel_type.map(|s| {
        ChannelType::parse_canonical(s)
            .map(|ct| ct.as_str().to_string())
            .unwrap_or_else(|| s.trim().to_uppercase())
    });

    let rows = if let Some(channel_type) = canonical_channel_type.as_deref() {
        sqlx::query_as::<_, ChannelIdentity>(
            "SELECT id, channel_type, channel_identifier, metadata, active, created_at, updated_at FROM channel_identities WHERE channel_type = $1 ORDER BY created_at DESC"
        )
        .bind(channel_type)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, ChannelIdentity>(
            "SELECT id, channel_type, channel_identifier, metadata, active, created_at, updated_at FROM channel_identities ORDER BY created_at DESC"
        )
        .fetch_all(pool)
        .await?
    };

    Ok(rows)
}

pub async fn get_channel_identity(
    pool: &DbPool,
    channel_id: i64,
) -> Result<Option<ChannelIdentity>, sqlx::Error> {
    sqlx::query_as::<_, ChannelIdentity>(
        "SELECT id, channel_type, channel_identifier, metadata, active, created_at, updated_at FROM channel_identities WHERE id = $1",
    )
    .bind(channel_id)
    .fetch_optional(pool)
    .await
}

pub async fn delete_channel_identity(pool: &DbPool, channel_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        DELETE FROM channel_identities
        WHERE id = $1
        "#,
    )
    .bind(channel_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

#[derive(Debug, Clone, FromRow)]
pub struct DocumentListRow {
    pub id: i64,
    pub short_ref: String,
    pub intake_status: Option<String>,
    pub accounting_status: Option<String>,
    pub review_reason: Option<String>,
    pub supplier_name: Option<String>,
    pub invoice_date: Option<String>,
    pub total_amount: Option<String>,
    pub received_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
struct DocumentListDbRow {
    pub id: i64,
    pub short_ref: String,
    pub supplier_name: Option<String>,
    pub invoice_date: Option<String>,
    pub total_amount: Option<String>,
    pub received_at: DateTime<Utc>,
    pub intake_status: Option<String>,
    pub accounting_status: Option<String>,
    pub review_reason: Option<String>,
}

pub async fn list_documents(
    pool: &DbPool,
    limit: i64,
) -> Result<Vec<DocumentListRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DocumentListDbRow>(
        r#"
        SELECT
            d.id,
            d.short_ref,
            (
                SELECT parsed_value
                FROM extracted_fields ef
                WHERE ef.document_id = d.id AND ef.field_type = 'supplier_name'
                ORDER BY ef.created_at DESC
                LIMIT 1
            ) AS supplier_name,
            (
                SELECT parsed_value
                FROM extracted_fields ef
                WHERE ef.document_id = d.id AND ef.field_type = 'transaction_date'
                ORDER BY ef.created_at DESC
                LIMIT 1
            ) AS invoice_date,
            (
                SELECT parsed_value
                FROM extracted_fields ef
                WHERE ef.document_id = d.id AND ef.field_type = 'total_amount'
                ORDER BY ef.created_at DESC
                LIMIT 1
            ) AS total_amount,
            d.received_at,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.review_reason AS review_reason
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        WHERE TRUE
        ORDER BY d.received_at DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| DocumentListRow {
            id: row.id,
            short_ref: row.short_ref,
            intake_status: row.intake_status,
            accounting_status: row.accounting_status,
            review_reason: row.review_reason,
            supplier_name: row.supplier_name,
            invoice_date: row.invoice_date,
            total_amount: row.total_amount,
            received_at: row.received_at,
        })
        .collect())
}
