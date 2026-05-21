use chrono::{DateTime, Utc};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow, SqliteSynchronous,
};
use sqlx::{Executor, FromRow, SqlitePool};
use std::str::FromStr;
use uuid::Uuid;

use crate::config::DatabaseConfig;
use crate::error::AppResult;

pub type DbPool = SqlitePool;
pub type DbRow = SqliteRow;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

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
    /// Slack direct message — identified by Slack user ID.
    #[serde(rename = "SLACK_DM")]
    SlackDm,
    /// Slack group/channel — identified by Slack channel ID.
    #[serde(rename = "SLACK_GROUP")]
    SlackGroup,
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

impl ChannelType {
    /// Parse a string slice into a `ChannelType`, case-insensitive, trimming whitespace.
    /// Returns `None` for unknown values.
    pub fn parse_canonical(s: &str) -> Option<Self> {
        let normalized = s.trim().to_uppercase();
        match normalized.as_str() {
            "TELEGRAM" => Some(Self::Telegram),
            "SLACK_DM" | "SLACKDM" | "SLACK DM" => Some(Self::SlackDm),
            "SLACK_GROUP" | "SLACKGROUP" | "SLACK GROUP" => Some(Self::SlackGroup),
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
            Self::SlackDm => "SLACK_DM",
            Self::SlackGroup => "SLACK_GROUP",
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
    _workspace_id: Uuid,
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

pub async fn get_channel_identity_for_workspace(
    pool: &DbPool,
    _workspace_id: Uuid,
    channel_id: i64,
) -> Result<Option<ChannelIdentity>, sqlx::Error> {
    sqlx::query_as::<_, ChannelIdentity>(
        "SELECT id, channel_type, channel_identifier, metadata, active, created_at, updated_at FROM channel_identities WHERE id = $1",
    )
    .bind(channel_id)
    .fetch_optional(pool)
    .await
}

pub async fn delete_channel_identity_for_workspace(
    pool: &DbPool,
    _workspace_id: Uuid,
    channel_id: i64,
) -> Result<bool, sqlx::Error> {
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
    pub status: String,
    pub supplier_name: Option<String>,
    pub invoice_date: Option<String>,
    pub total_amount: Option<String>,
    pub received_at: DateTime<Utc>,
}

pub async fn list_documents_by_workspace(
    pool: &DbPool,
    _workspace_id: Uuid,
    limit: i64,
) -> Result<Vec<DocumentListRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DocumentListRow>(
        r#"
        SELECT
            d.id,
            d.short_ref,
            d.status,
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
            d.received_at
        FROM documents d
        WHERE TRUE
        ORDER BY d.received_at DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}
