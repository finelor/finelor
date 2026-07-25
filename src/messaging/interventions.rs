use crate::db::ChannelType;
use crate::db::DbPool;
use crate::kv::EphemeralStore;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::messaging::commands::reopen_review_actions;
use crate::messaging::contracts::{GatewayMessageFormat, GatewayMessageResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterventionAudience {
    Sender,
    Accounting,
    AuditOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterventionKind {
    SenderClarification,
    SenderDuplicate,
    AccountingReview,
    AccountingNotification,
    AuditOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterventionTarget {
    pub channel_type: String,
    pub channel_identifier: String,
    pub profile_identifier: Option<String>,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentIntervention {
    pub event_id: i64,
    pub workspace_id: Uuid,
    pub document_id: i64,
    pub short_ref: String,
    pub audience: InterventionAudience,
    pub kind: InterventionKind,
    pub target: Option<InterventionTarget>,
    pub response: GatewayMessageResponse,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DocumentEventCandidate {
    pub event_id: i64,
    pub document_id: i64,
    pub short_ref: String,
    pub intake_status: Option<String>,
    pub accounting_status: Option<String>,
    pub accounting_requested_at: Option<String>,
    pub accounting_review_reason: Option<String>,
    pub accounting_export_batch_id: Option<i64>,
    pub latest_accounting_run_kind: Option<String>,
    pub latest_accounting_run_status: Option<String>,
    pub event_type: String,
    pub payload: Option<Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ArtifactTarget {
    channel_type: String,
    channel_identifier: String,
    profile_identifier: Option<String>,
    metadata: Option<Value>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct AccountingTarget {
    channel_type: String,
    channel_identifier: String,
    metadata: Option<Value>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct DocumentEventCandidateRow {
    event_id: i64,
    document_id: i64,
    short_ref: String,
    intake_status: Option<String>,
    accounting_status: Option<String>,
    accounting_requested_at: Option<String>,
    accounting_review_reason: Option<String>,
    accounting_export_batch_id: Option<i64>,
    latest_accounting_run_kind: Option<String>,
    latest_accounting_run_status: Option<String>,
    event_type: String,
    payload: Option<Value>,
    created_at: DateTime<Utc>,
}

fn candidate_from_row(row: DocumentEventCandidateRow) -> DocumentEventCandidate {
    DocumentEventCandidate {
        event_id: row.event_id,
        document_id: row.document_id,
        short_ref: row.short_ref,
        intake_status: row.intake_status,
        accounting_status: row.accounting_status,
        accounting_requested_at: row.accounting_requested_at,
        accounting_review_reason: row.accounting_review_reason,
        accounting_export_batch_id: row.accounting_export_batch_id,
        latest_accounting_run_kind: row.latest_accounting_run_kind,
        latest_accounting_run_status: row.latest_accounting_run_status,
        event_type: row.event_type,
        payload: row.payload,
        created_at: row.created_at,
    }
}

pub async fn pending_document_interventions(
    pool: &DbPool,
    accounting_channel_type: Option<ChannelType>,
    since: DateTime<Utc>,
    limit: i64,
) -> anyhow::Result<Vec<DocumentIntervention>> {
    let rows = pending_document_event_candidates(pool, since, limit).await?;
    let mut interventions = Vec::new();

    for row in rows {
        if let Some(intervention) =
            build_document_intervention(pool, row, accounting_channel_type).await?
        {
            interventions.push(intervention);
        }
    }

    Ok(interventions)
}

pub async fn document_intervention_candidate_by_event_id(
    pool: &DbPool,
    event_id: i64,
) -> anyhow::Result<Option<DocumentEventCandidate>> {
    let row = sqlx::query_as::<_, DocumentEventCandidateRow>(
        r#"
        SELECT
            e.id AS event_id,
            e.document_id,
            d.short_ref,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.requested_at AS accounting_requested_at,
            das.review_reason AS accounting_review_reason,
            das.export_batch_id AS accounting_export_batch_id,
            (
                SELECT dar.run_kind
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_kind,
            (
                SELECT dar.run_status
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_status,
            e.event_type,
            e.payload,
            e.created_at
        FROM document_events e
        JOIN documents d ON d.id = e.document_id
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        WHERE e.id = $1
        "#,
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(candidate_from_row))
}

async fn pending_document_event_candidates(
    pool: &DbPool,
    since: DateTime<Utc>,
    limit: i64,
) -> anyhow::Result<Vec<DocumentEventCandidate>> {
    let rows = sqlx::query_as::<_, DocumentEventCandidateRow>(
        r#"
        SELECT
            e.id AS event_id,
            e.document_id,
            d.short_ref,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.requested_at AS accounting_requested_at,
            das.review_reason AS accounting_review_reason,
            das.export_batch_id AS accounting_export_batch_id,
            (
                SELECT dar.run_kind
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_kind,
            (
                SELECT dar.run_status
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_status,
            e.event_type,
            e.payload,
            e.created_at
        FROM document_events e
        JOIN documents d ON d.id = e.document_id
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        WHERE e.created_at >= $1
          AND e.event_type IN (
            'VISION_COMPLETED',
            'DUPLICATE_DETECTED',
            'REVIEW_HUMAN_REQUESTED',
            'DOCUMENT_FAILED',
            'REVIEW_QUARANTINED',
            'REVIEW_AUTO_APPROVED',
            'STATUS_CHANGED',
            'SENDER_CLARIFICATION_REQUESTED'
          )
        ORDER BY e.created_at ASC, e.id ASC
        LIMIT $2
        "#,
    )
    .bind(since)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(candidate_from_row).collect())
}

pub async fn build_document_intervention(
    pool: &DbPool,
    event: DocumentEventCandidate,
    accounting_channel_type: Option<ChannelType>,
) -> anyhow::Result<Option<DocumentIntervention>> {
    let payload = event.payload.clone().unwrap_or_else(|| json!({}));
    let Some((audience, kind)) = classify_document_event(&event.event_type, &payload) else {
        return Ok(None);
    };

    let target = match audience {
        InterventionAudience::Sender => sender_target(pool, event.document_id).await?,
        InterventionAudience::Accounting => {
            if let Some(channel_type) = accounting_channel_type {
                accounting_target(pool, channel_type).await?
            } else {
                None
            }
        }
        InterventionAudience::AuditOnly => None,
    };

    if audience != InterventionAudience::AuditOnly && target.is_none() {
        return Ok(None);
    }

    let response = build_intervention_response(pool, &event, &payload, kind).await?;

    Ok(Some(DocumentIntervention {
        event_id: event.event_id,
        workspace_id: crate::workspace::active_workspace_id(),
        document_id: event.document_id,
        short_ref: event.short_ref,
        audience,
        kind,
        target,
        response,
    }))
}

fn classify_document_event(
    event_type: &str,
    payload: &Value,
) -> Option<(InterventionAudience, InterventionKind)> {
    if event_type == "SENDER_CLARIFICATION_REQUESTED"
        || payload.get("audience").and_then(Value::as_str) == Some("sender")
    {
        return Some((
            InterventionAudience::Sender,
            InterventionKind::SenderClarification,
        ));
    }

    match event_type {
        "DUPLICATE_DETECTED" => Some((
            InterventionAudience::Sender,
            InterventionKind::SenderDuplicate,
        )),
        "REVIEW_HUMAN_REQUESTED" | "REVIEW_QUARANTINED" => Some((
            InterventionAudience::Accounting,
            InterventionKind::AccountingReview,
        )),
        "DOCUMENT_FAILED" => Some((
            InterventionAudience::Accounting,
            InterventionKind::AccountingNotification,
        )),
        "REVIEW_AUTO_APPROVED" => None,
        "STATUS_CHANGED" if status_changed_marks_export_ready(payload) => Some((
            InterventionAudience::Accounting,
            InterventionKind::AccountingNotification,
        )),
        _ => None,
    }
}

fn status_changed_marks_export_ready(payload: &Value) -> bool {
    payload.get("accounting_status").and_then(Value::as_str) == Some("READY_FOR_EXPORT")
        || payload.get("export_status").and_then(Value::as_str) == Some("READY")
}

async fn build_intervention_response(
    pool: &DbPool,
    event: &DocumentEventCandidate,
    payload: &Value,
    kind: InterventionKind,
) -> anyhow::Result<GatewayMessageResponse> {
    match kind {
        InterventionKind::SenderClarification => Ok(GatewayMessageResponse {
            message: payload
                .get("question")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!(
                        "I need one detail before I can continue with {}. What was this receipt for?",
                        event.short_ref
                    )
                }),
            format: GatewayMessageFormat::PlainText,
            attachments: Vec::new(),
            buttons: None,
        }),
        InterventionKind::SenderDuplicate => Ok(sender_duplicate_response(event)),
        InterventionKind::AccountingReview => reopen_review_actions(pool, &event.short_ref).await,
        InterventionKind::AccountingNotification => {
            Ok(accounting_notification_response(event, payload))
        }
        InterventionKind::AuditOnly => Ok(GatewayMessageResponse::plain_text("")),
    }
}

fn sender_duplicate_response(event: &DocumentEventCandidate) -> GatewayMessageResponse {
    GatewayMessageResponse {
        message: format!(
            "This document is already processed and won't be processed again. Existing ref: {}.",
            event.short_ref
        ),
        format: GatewayMessageFormat::PlainText,
        attachments: Vec::new(),
        buttons: None,
    }
}

fn accounting_notification_response(
    event: &DocumentEventCandidate,
    payload: &Value,
) -> GatewayMessageResponse {
    let message = match event.event_type.as_str() {
        "DOCUMENT_FAILED" => {
            let stage = payload
                .get("stage")
                .and_then(Value::as_str)
                .unwrap_or("processing");
            format!(
                "{} failed during {}. Ask why {} is blocked for details or ask me to retry {} to reprocess it.",
                event.short_ref, stage, event.short_ref, event.short_ref
            )
        }
        "STATUS_CHANGED" => format!(
            "{} is ready for accounting export. Ask me to export {} or ask for documents ready for export.",
            event.short_ref, event.short_ref
        ),
        _ => format!("{} needs accounting attention.", event.short_ref),
    };

    GatewayMessageResponse {
        message,
        format: GatewayMessageFormat::Markdown,
        attachments: Vec::new(),
        buttons: None,
    }
}

async fn sender_target(
    pool: &DbPool,
    document_id: i64,
) -> anyhow::Result<Option<InterventionTarget>> {
    let target: Option<ArtifactTarget> = sqlx::query_as(
        r#"
        SELECT channel_type, channel_identifier, profile_identifier, metadata
        FROM document_artifacts
        WHERE document_id = $1
        ORDER BY created_at ASC
        LIMIT 1
        "#,
    )
    .bind(document_id)
    .fetch_optional(pool)
    .await?;

    Ok(target.map(|target| InterventionTarget {
        channel_type: target.channel_type,
        channel_identifier: target.channel_identifier,
        profile_identifier: target.profile_identifier,
        metadata: target.metadata.unwrap_or_else(|| json!({})),
    }))
}

async fn accounting_target(
    pool: &DbPool,
    channel_type: ChannelType,
) -> anyhow::Result<Option<InterventionTarget>> {
    let target: Option<AccountingTarget> = sqlx::query_as(
        r#"
        SELECT channel_type, channel_identifier, metadata
        FROM channel_identities
        WHERE active = TRUE
          AND channel_type = $1
          AND (
              channel_type != 'SLACK'
              OR json_extract(metadata, '$.connected_by_user_id') IS NOT NULL
          )
        ORDER BY created_at ASC
        LIMIT 1
        "#,
    )
    .bind(channel_type.as_str())
    .fetch_optional(pool)
    .await?;

    Ok(target.map(|target| InterventionTarget {
        channel_type: target.channel_type,
        channel_identifier: target.channel_identifier,
        profile_identifier: None,
        metadata: target.metadata.unwrap_or_else(|| json!({})),
    }))
}

pub async fn claim_intervention_delivery(
    store: &EphemeralStore,
    intervention: &DocumentIntervention,
) -> anyhow::Result<bool> {
    let target = intervention
        .target
        .as_ref()
        .map(|target| {
            format!(
                "{}:{}:{}",
                target.channel_type,
                target.channel_identifier,
                target.profile_identifier.as_deref().unwrap_or("")
            )
        })
        .unwrap_or_else(|| "audit".to_string());
    let key = format!(
        "agent_delivery:{}:{:?}:{}",
        intervention.event_id, intervention.kind, target
    );
    Ok(store.claim(&key, "processing", 86_400).await?)
}

pub async fn release_intervention_delivery_claim(
    store: &EphemeralStore,
    intervention: &DocumentIntervention,
) -> anyhow::Result<()> {
    let target = intervention
        .target
        .as_ref()
        .map(|target| {
            format!(
                "{}:{}:{}",
                target.channel_type,
                target.channel_identifier,
                target.profile_identifier.as_deref().unwrap_or("")
            )
        })
        .unwrap_or_else(|| "audit".to_string());
    let key = format!(
        "agent_delivery:{}:{:?}:{}",
        intervention.event_id, intervention.kind, target
    );
    let _ = store.delete(&key).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    use crate::db::run_migrations;

    #[test]
    fn vision_completed_is_not_a_sender_intervention() {
        assert_eq!(
            classify_document_event("VISION_COMPLETED", &json!({})),
            None
        );
    }

    #[tokio::test]
    async fn duplicate_detected_builds_sender_duplicate_message() {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("connect sqlite");
        run_migrations(&pool).await.expect("run migrations");

        let (document_id, short_ref): (i64, String) = sqlx::query_as(
            r#"
            INSERT INTO documents (filename, file_hash, original_path, mime_type)
            VALUES ('receipt.jpg', 'hash-1', '/tmp/receipt.jpg', 'image/jpeg')
            RETURNING id, short_ref
            "#,
        )
        .fetch_one(&pool)
        .await
        .expect("insert document");
        crate::document_state::seed_document_state_preset(
            &pool,
            document_id,
            crate::document_state::TestDocumentStatePreset::IntakeIngested,
        )
        .await
        .expect("seed document state");

        let event = DocumentEventCandidate {
            event_id: 1,
            document_id,
            short_ref: short_ref.clone(),
            intake_status: Some("INGESTED".to_string()),
            accounting_status: Some("NOT_REQUESTED".to_string()),
            accounting_requested_at: None,
            accounting_review_reason: None,
            accounting_export_batch_id: None,
            latest_accounting_run_kind: None,
            latest_accounting_run_status: None,
            event_type: "DUPLICATE_DETECTED".to_string(),
            payload: None,
            created_at: Utc::now(),
        };

        let response = build_intervention_response(
            &pool,
            &event,
            &json!({}),
            InterventionKind::SenderDuplicate,
        )
        .await
        .expect("build sender duplicate response");

        assert_eq!(
            response.message,
            format!(
                "This document is already processed and won't be processed again. Existing ref: {}.",
                short_ref
            )
        );
    }

    #[tokio::test]
    async fn duplicate_detected_is_pending_sender_intervention() {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("connect sqlite");
        run_migrations(&pool).await.expect("run migrations");

        let document_id = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO documents (filename, file_hash, original_path, mime_type)
            VALUES ('receipt.jpg', 'hash-2', '/tmp/receipt.jpg', 'image/jpeg')
            RETURNING id
            "#,
        )
        .fetch_one(&pool)
        .await
        .expect("insert document");
        crate::document_state::seed_document_state_preset(
            &pool,
            document_id,
            crate::document_state::TestDocumentStatePreset::IntakeIngested,
        )
        .await
        .expect("seed document state");

        sqlx::query(
            r#"
            INSERT INTO document_artifacts (
                document_id,
                channel_type,
                channel_identifier,
                profile_identifier,
                source_timestamp,
                original_filename,
                metadata
            )
            VALUES ($1, 'TELEGRAM', '12345', '12345', NULL, 'receipt.jpg', '{}')
            "#,
        )
        .bind(document_id)
        .execute(&pool)
        .await
        .expect("insert artifact");

        let event_id = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO document_events (document_id, event_type, payload)
            VALUES ($1, 'DUPLICATE_DETECTED', '{"short_ref":"D000001"}')
            RETURNING id
            "#,
        )
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .expect("insert event");

        let candidate = document_intervention_candidate_by_event_id(&pool, event_id)
            .await
            .expect("fetch candidate")
            .expect("candidate exists");
        let intervention = build_document_intervention(&pool, candidate, None)
            .await
            .expect("build intervention")
            .expect("intervention exists");

        assert_eq!(intervention.kind, InterventionKind::SenderDuplicate);
        assert_eq!(intervention.audience, InterventionAudience::Sender);
    }

    #[test]
    fn classifies_sender_clarification_from_explicit_audience() {
        let classified = classify_document_event(
            "REVIEW_HUMAN_REQUESTED",
            &json!({ "audience": "sender", "question": "Was this a business trip?" }),
        );
        assert_eq!(
            classified,
            Some((
                InterventionAudience::Sender,
                InterventionKind::SenderClarification
            ))
        );
    }

    #[test]
    fn classifies_review_request_for_accounting() {
        let classified = classify_document_event("REVIEW_HUMAN_REQUESTED", &json!({}));
        assert_eq!(
            classified,
            Some((
                InterventionAudience::Accounting,
                InterventionKind::AccountingReview
            ))
        );
    }

    #[test]
    fn routine_validation_completion_is_audit_only() {
        assert_eq!(
            classify_document_event("VALIDATION_COMPLETED", &json!({})),
            None
        );
    }

    #[test]
    fn export_batch_created_is_audit_only() {
        assert_eq!(
            classify_document_event("EXPORT_BATCH_CREATED", &json!({ "batch_id": "batch-1" })),
            None
        );
    }

    #[test]
    fn export_ready_status_changed_supports_new_payload_shape() {
        assert_eq!(
            classify_document_event(
                "STATUS_CHANGED",
                &json!({ "accounting_status": "READY_FOR_EXPORT" })
            ),
            Some((
                InterventionAudience::Accounting,
                InterventionKind::AccountingNotification
            ))
        );
    }
}
