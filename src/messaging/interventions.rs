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
    SenderProgress,
    SenderClarification,
    AccountingReview,
    AccountingNotification,
    AuditOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterventionTarget {
    pub channel_type: String,
    pub channel_identifier: String,
    pub profile_identifier: Option<String>,
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
    pub status: String,
    pub event_type: String,
    pub payload: Option<Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ArtifactTarget {
    channel_type: String,
    channel_identifier: String,
    profile_identifier: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct AccountingTarget {
    channel_type: String,
    channel_identifier: String,
}

pub async fn pending_document_interventions(
    pool: &DbPool,
    since: DateTime<Utc>,
    limit: i64,
) -> anyhow::Result<Vec<DocumentIntervention>> {
    let rows = pending_document_event_candidates(pool, since, limit).await?;
    let mut interventions = Vec::new();

    for row in rows {
        if let Some(intervention) = build_document_intervention(pool, row).await? {
            interventions.push(intervention);
        }
    }

    Ok(interventions)
}

pub async fn document_intervention_candidate_by_event_id(
    pool: &DbPool,
    event_id: i64,
) -> anyhow::Result<Option<DocumentEventCandidate>> {
    let row = sqlx::query_as(
        r#"
        SELECT
            e.id AS event_id,
            e.document_id,
            d.short_ref,
            d.status,
            e.event_type,
            e.payload,
            e.created_at
        FROM document_events e
        JOIN documents d ON d.id = e.document_id
        WHERE e.id = $1
        "#,
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

async fn pending_document_event_candidates(
    pool: &DbPool,
    since: DateTime<Utc>,
    limit: i64,
) -> anyhow::Result<Vec<DocumentEventCandidate>> {
    let rows = sqlx::query_as(
        r#"
        SELECT
            e.id AS event_id,
            e.document_id,
            d.short_ref,
            d.status,
            e.event_type,
            e.payload,
            e.created_at
        FROM document_events e
        JOIN documents d ON d.id = e.document_id
        WHERE e.created_at >= $1
          AND e.event_type IN (
            'VISION_COMPLETED',
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

    Ok(rows)
}

pub async fn build_document_intervention(
    pool: &DbPool,
    event: DocumentEventCandidate,
) -> anyhow::Result<Option<DocumentIntervention>> {
    let payload = event.payload.clone().unwrap_or_else(|| json!({}));
    let Some((audience, kind)) = classify_document_event(&event.event_type, &payload) else {
        return Ok(None);
    };

    let target = match audience {
        InterventionAudience::Sender => sender_target(pool, event.document_id).await?,
        InterventionAudience::Accounting => accounting_target(pool).await?,
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
        "VISION_COMPLETED" => Some((
            InterventionAudience::Sender,
            InterventionKind::SenderProgress,
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
        "STATUS_CHANGED"
            if payload.get("status").and_then(Value::as_str) == Some("EXPORT_READY") =>
        {
            Some((
                InterventionAudience::Accounting,
                InterventionKind::AccountingNotification,
            ))
        }
        _ => None,
    }
}

async fn build_intervention_response(
    pool: &DbPool,
    event: &DocumentEventCandidate,
    payload: &Value,
    kind: InterventionKind,
) -> anyhow::Result<GatewayMessageResponse> {
    match kind {
        InterventionKind::SenderProgress => sender_progress_response(pool, event).await,
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
        InterventionKind::AccountingReview => {
            reopen_review_actions(pool, crate::workspace::active_workspace_id(), &event.short_ref)
                .await
        }
        InterventionKind::AccountingNotification => {
            Ok(accounting_notification_response(event, payload))
        }
        InterventionKind::AuditOnly => Ok(GatewayMessageResponse::plain_text("")),
    }
}

async fn sender_progress_response(
    pool: &DbPool,
    event: &DocumentEventCandidate,
) -> anyhow::Result<GatewayMessageResponse> {
    let details = document_brief_details(pool, event.document_id).await?;
    let mut message = format!("Recorded {}.", event.short_ref);

    if let Some(summary) = details {
        message = format!("Recorded {}. {}", event.short_ref, summary);
    }

    message.push_str(" Forwarding to accounting.");

    Ok(GatewayMessageResponse {
        message,
        format: GatewayMessageFormat::PlainText,
        attachments: Vec::new(),
        buttons: None,
    })
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
                "{} failed during {}. Use /why {} for details or /retry {} to reprocess it.",
                event.short_ref, stage, event.short_ref, event.short_ref
            )
        }
        "STATUS_CHANGED" => format!(
            "{} is ready for accounting export. Use /export {} to export it, or /ready to see all ready documents.",
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

async fn document_brief_details(pool: &DbPool, document_id: i64) -> anyhow::Result<Option<String>> {
    let supplier: Option<String> = sqlx::query_scalar(
        r#"
        SELECT COALESCE(parsed_value, raw_value)
        FROM extracted_fields
        WHERE document_id = $1
          AND field_type IN ('supplier_name', 'supplier', 'merchant_name')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(document_id)
    .fetch_optional(pool)
    .await?;

    let amount: Option<String> = sqlx::query_scalar(
        r#"
        SELECT COALESCE(parsed_value, raw_value)
        FROM extracted_fields
        WHERE document_id = $1
          AND field_type IN ('total_amount', 'amount', 'gross_amount')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(document_id)
    .fetch_optional(pool)
    .await?;

    let mut parts = Vec::new();
    if let Some(supplier) = supplier.filter(|value| !value.trim().is_empty()) {
        parts.push(supplier);
    }
    if let Some(amount) = amount.filter(|value| !value.trim().is_empty()) {
        parts.push(amount);
    }

    if parts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parts.join(", ")))
    }
}

async fn sender_target(
    pool: &DbPool,
    document_id: i64,
) -> anyhow::Result<Option<InterventionTarget>> {
    let target: Option<ArtifactTarget> = sqlx::query_as(
        r#"
        SELECT channel_type, channel_identifier, profile_identifier
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
    }))
}

async fn accounting_target(pool: &DbPool) -> anyhow::Result<Option<InterventionTarget>> {
    let target: Option<AccountingTarget> = sqlx::query_as(
        r#"
        SELECT channel_type, channel_identifier
        FROM channel_identities
        WHERE active = TRUE
        ORDER BY
          CASE WHEN channel_type = 'TELEGRAM' THEN 0 ELSE 1 END,
          created_at ASC
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(target.map(|target| InterventionTarget {
        channel_type: target.channel_type,
        channel_identifier: target.channel_identifier,
        profile_identifier: None,
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

    #[test]
    fn classifies_sender_progress_from_vision_completion() {
        let classified = classify_document_event("VISION_COMPLETED", &json!({}));
        assert_eq!(
            classified,
            Some((
                InterventionAudience::Sender,
                InterventionKind::SenderProgress
            ))
        );
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
}
