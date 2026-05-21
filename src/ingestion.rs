use std::path::PathBuf;

use crate::db::DbPool;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::{Agent, AgentContext, IntakeInput};
use crate::agents::{db_helpers::record_document_event, publish_document_event};
use crate::config::AppConfig;
use crate::orchestration::Orchestrator;
use crate::queue::QueueProducer;
use crate::web::events::AppEventBus;

/// Result of saving or retrieving a document through the common ingestion boundary.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct SavedDocumentRef {
    pub id: i64,
    pub short_ref: String,
}

/// Input for the shared ingestion helper.
///
/// This boundary performs:
/// - file write to disk
/// - SHA-256 hash computation
/// - idempotent upsert into `documents` (ON CONFLICT on `file_hash`)
/// - IntakeAgent processing to queue the pipeline job
#[derive(Debug)]
pub struct IngestionInput {
    pub file_bytes: Vec<u8>,
    pub filename: String,
    pub mime_type: String,
    pub document_type: String,
    pub artifact: DocumentArtifactInput,
}

#[derive(Debug)]
pub struct DocumentArtifactInput {
    pub channel_type: String,
    pub channel_identifier: String,
    pub profile_identifier: Option<String>,
    pub external_artifact_id: Option<String>,
    pub source_timestamp: Option<String>,
    pub original_filename: Option<String>,
    pub metadata: serde_json::Value,
}

/// Ingest a document through the shared boundary.
///
/// On duplicate file hash the existing row is returned without re-enqueuing
/// intake, matching the original Telegram behaviour.
pub async fn ingest_document(
    pool: &DbPool,
    config: &AppConfig,
    queue_producer: &QueueProducer,
    events: Option<&AppEventBus>,
    input: IngestionInput,
) -> anyhow::Result<SavedDocumentRef> {
    let file_hash = calculate_file_hash(&input.file_bytes);
    let file_size = input.file_bytes.len() as i64;

    // Check whether the document already exists by scoped key (active workspace + hash).
    let existing: Option<SavedDocumentRef> =
        sqlx::query_as("SELECT id, short_ref FROM documents WHERE file_hash = $1")
            .bind(&file_hash)
            .fetch_optional(pool)
            .await
            .context("Failed to query existing document by scoped key")?;

    if let Some(saved) = existing {
        insert_document_artifact(pool, saved.id, &input, &file_hash).await?;
        info!(
            document_id = %saved.id,
            short_ref = %saved.short_ref,
            "Document already exists (duplicate scoped key)"
        );
        // Record duplicate detection audit event even when returning existing doc
        let event_id = record_document_event(
            pool,
            saved.id,
            "DUPLICATE_DETECTED",
            serde_json::json!({
                "old_status": null,
                "new_status": "RECEIVED",
                "artifact_channel_type": &input.artifact.channel_type,
                "content_hash": &file_hash,
                "short_ref": &saved.short_ref,
                "filename": &input.filename,
                "existing_document_id": saved.id,
            }),
        )
        .await
        .ok();
        if let (Some(events), Some(event_id)) = (events, event_id) {
            let _ = publish_document_event(pool, events, event_id, saved.id, "DUPLICATE_DETECTED")
                .await;
        }
        return Ok(saved);
    }

    // Save to disk
    let upload_dir = PathBuf::from(&config.upload.storage_path);
    std::fs::create_dir_all(&upload_dir)?;
    let path = upload_dir.join(&input.filename);
    tokio::fs::write(&path, &input.file_bytes)
        .await
        .context("Failed to write file to disk")?;

    let saved: SavedDocumentRef = sqlx::query_as(
        r#"
        INSERT INTO documents (
            status, document_type, filename, original_path,
            file_hash, file_size_bytes, mime_type, priority
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8
        )
        ON CONFLICT (file_hash) DO UPDATE SET
            updated_at = CURRENT_TIMESTAMP
        RETURNING id, short_ref
        "#,
    )
    .bind("RECEIVED")
    .bind(&input.document_type)
    .bind(&input.filename)
    .bind(path.to_str())
    .bind(&file_hash)
    .bind(file_size)
    .bind(&input.mime_type)
    .bind("NORMAL")
    .fetch_one(pool)
    .await
    .context("Failed to save document to database")?;

    insert_document_artifact(pool, saved.id, &input, &file_hash).await?;

    info!(
        document_id = %saved.id,
        short_ref = %saved.short_ref,
        stored_path = %path.display(),
        file_size = file_size,
        "Document saved successfully"
    );

    // Record initial audit event so the pipeline can trace the document from the start
    let event_id = record_document_event(
        pool,
        saved.id,
        "STATUS_CHANGED",
        serde_json::json!({
            "old_status": null,
            "new_status": "RECEIVED",
            "artifact_channel_type": &input.artifact.channel_type,
            "content_hash": &file_hash,
            "short_ref": &saved.short_ref,
            "filename": &input.filename,
        }),
    )
    .await
    .ok();
    if let (Some(events), Some(event_id)) = (events, event_id) {
        let _ = publish_document_event(pool, events, event_id, saved.id, "STATUS_CHANGED").await;
    }

    let events = events.cloned().unwrap_or_else(|| AppEventBus::new(1));
    let agent_context = AgentContext::new(pool.clone(), config.clone(), events);
    let orchestrator = Orchestrator::new(agent_context, queue_producer.clone());
    let intake_agent = orchestrator.get_intake_agent();
    intake_agent
        .process(IntakeInput {
            document_id: saved.id,
            file_path: path.clone(),
            mime_type: input.mime_type.clone(),
            filename: input.filename.clone(),
        })
        .await
        .context("failed to queue document for intake")?;

    Ok(saved)
}

async fn insert_document_artifact(
    pool: &DbPool,
    document_id: i64,
    input: &IngestionInput,
    file_hash: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO document_artifacts (
            document_id, channel_type, channel_identifier,
            profile_identifier, external_artifact_id, source_timestamp,
            original_filename, mime_type, file_hash, metadata
        )
        VALUES (
            $1, $2, $3,
            $4, $5, $6,
            $7, $8, $9, $10
        )
        "#,
    )
    .bind(document_id)
    .bind(input.artifact.channel_type.trim().to_uppercase())
    .bind(&input.artifact.channel_identifier)
    .bind(&input.artifact.profile_identifier)
    .bind(&input.artifact.external_artifact_id)
    .bind(&input.artifact.source_timestamp)
    .bind(&input.artifact.original_filename)
    .bind(&input.mime_type)
    .bind(file_hash)
    .bind(&input.artifact.metadata)
    .execute(pool)
    .await
    .context("Failed to save document artifact")?;

    Ok(())
}

fn calculate_file_hash(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}
