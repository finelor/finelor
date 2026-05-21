use std::path::{Path, PathBuf};

use crate::db::DbPool;
use anyhow::anyhow;
use chrono::NaiveDate;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::agents::review::ApprovalBlockExplanation;
use crate::agents::{
    Agent, AgentContext, ExportAgent, ExportInput, IntakeAgent, IntakeInput,
    build_failed_document_explanation, describe_approval_block,
};
use crate::query::{
    DocumentRef, DocumentSummary, ReviewSummary, document_ref_by_short_ref, document_status_counts,
    document_summary_by_short_ref, document_why_details, latest_document,
    latest_failure_event_for_document, latest_source_media_artifact, list_documents_by_status,
    list_documents_requiring_attention, list_recent_documents, retry_document_by_short_ref,
    review_summary,
};

use super::contracts::{
    ActionButton, DocumentAction, GatewayAttachment, GatewayMessageFormat, GatewayMessageResponse,
    MessageSource, ReviewFieldAction,
};
use super::gateway::AgentGatewayState;
use super::intents::{GatewayIntentArgs, GatewayIntentKind, GatewayIntentResolution};

enum ReviewEligibility {
    Reviewable,
    NotReviewableSystemFailure(String),
    NotReviewableStatus(String),
}

pub async fn execute_gateway_intent(
    state: &AgentGatewayState,
    source: &MessageSource,
    workspace_id: Uuid,
    resolution: &GatewayIntentResolution,
) -> anyhow::Result<GatewayMessageResponse> {
    if resolution.missing_args.iter().any(|arg| arg == "short_ref") {
        return Ok(GatewayMessageResponse::text(format!(
            "Please include a document reference, for example /{} D000123.",
            resolution.intent.as_str()
        )));
    }

    match resolution.intent {
        GatewayIntentKind::Help => Ok(GatewayMessageResponse::text(build_help_message())),
        GatewayIntentKind::Status => Ok(GatewayMessageResponse::text(
            if let Some(short_ref) = resolution.args.short_ref.as_deref() {
                build_document_status_message(&state.pool, workspace_id, short_ref).await?
            } else {
                build_status_message(&state.pool, workspace_id).await?
            },
        )),
        GatewayIntentKind::Documents => Ok(GatewayMessageResponse::text(
            build_all_documents_message(&state.pool, workspace_id).await?,
        )),
        GatewayIntentKind::Pending => Ok(GatewayMessageResponse::text(
            build_attention_required_message(&state.pool, workspace_id).await?,
        )),
        GatewayIntentKind::Ready => Ok(GatewayMessageResponse::text(
            build_document_list_message(
                &state.pool,
                workspace_id,
                "EXPORT_READY",
                "No documents are currently in your export pool.",
                "Documents ready for export",
            )
            .await?,
        )),
        GatewayIntentKind::Last => Ok(GatewayMessageResponse::text(
            build_last_message(&state.pool, workspace_id).await?,
        )),
        GatewayIntentKind::Why => {
            let Some(short_ref) = resolution.args.short_ref.as_deref() else {
                return Ok(GatewayMessageResponse::text("Usage: /why D000123"));
            };
            Ok(GatewayMessageResponse::text(
                describe_document_why(&state.pool, workspace_id, short_ref).await?,
            ))
        }
        GatewayIntentKind::Review => {
            let Some(short_ref) = resolution.args.short_ref.as_deref() else {
                return Ok(GatewayMessageResponse::text("Usage: /review D000123"));
            };
            reopen_review_actions(&state.pool, workspace_id, short_ref).await
        }
        GatewayIntentKind::Retry => {
            let Some(short_ref) = resolution.args.short_ref.as_deref() else {
                return Ok(GatewayMessageResponse::text("Usage: /retry D000123"));
            };
            Ok(GatewayMessageResponse::text(
                retry_document(state, source, workspace_id, short_ref).await?,
            ))
        }
        GatewayIntentKind::Export => {
            export_documents(state, source, workspace_id, &resolution.args).await
        }
        GatewayIntentKind::UploadInstruction => Ok(GatewayMessageResponse::text(
            "Send an invoice or receipt as a file or image in this chat, and I will add it to the company document flow.",
        )),
        GatewayIntentKind::OutOfScope => Ok(GatewayMessageResponse::text(out_of_scope_message())),
        GatewayIntentKind::Unknown | GatewayIntentKind::GeneralAccountingChat => {
            Ok(GatewayMessageResponse::text(build_help_message()))
        }
    }
}

pub fn out_of_scope_message() -> &'static str {
    "I cannot help with that. I can help with invoices, receipts, document status, review, retry, export, and how to upload accounting documents."
}

pub fn build_help_message() -> String {
    "Welcome to Finelor.\n\nCommands:\n/help - Show this help\n/status - Show overall document counts by status\n/documents - List recent company documents\n/pending - List documents that need your attention\n/ready - List documents in the export pool\n/last - Show your latest document\n/why D000123 - Explain why a document is blocked or pending\n/review D000123 - Open review actions for a pending or reviewable failed document\n/retry D000123 - Reprocess a failed or completed document from the start\n/export - Export your current export pool\n/export D000123 - Export one specific document if it is ready\n\nYou can also ask for these in plain language, for example: \"what needs review?\" or \"what is the status of our invoices?\""
        .to_string()
}

async fn build_status_message(pool: &DbPool, _workspace_id: Uuid) -> anyhow::Result<String> {
    let counts = document_status_counts(pool).await?;
    let last = latest_document(pool).await?;
    let mut lines = vec![
        "Finelor status:".to_string(),
        format!("Processing: {}", counts.processing_count),
        format!("Pending review: {}", counts.pending_count),
        format!("Export ready: {}", counts.ready_count),
        format!("Exported: {}", counts.exported_count),
        format!("Failed: {}", counts.failed_count),
    ];

    if let Some(last) = last {
        lines.push(String::new());
        lines.push(format!("Latest: {} ({})", last.short_ref, last.status));
    }

    Ok(lines.join("\n"))
}

async fn build_document_status_message(
    pool: &DbPool,
    _workspace_id: Uuid,
    short_ref: &str,
) -> anyhow::Result<String> {
    let Some(document) = document_summary_by_short_ref(pool, short_ref).await? else {
        return Ok(format!(
            "Document {} was not found for this company.",
            short_ref
        ));
    };

    let mut lines = vec![
        format!("Document: {}", document.short_ref),
        format!("Status: {}", document.status),
    ];
    if let Some(supplier) = document.supplier_name.as_deref() {
        lines.push(format!("Supplier: {}", supplier));
    }
    if let Some(amount) = document.total_amount.as_deref() {
        lines.push(format!("Amount: {} SEK", amount));
    }
    if let Some(date) = document.invoice_date.as_deref() {
        lines.push(format!("Date: {}", date));
    }
    if let Some(confidence) = document.confidence_score {
        lines.push(format!(
            "Confidence: {:.0}%",
            confidence_to_percent(confidence)
        ));
    }
    if let Some(reason) = document.review_reason.as_deref() {
        lines.push(format!("Why: {}", reason));
    }

    Ok(lines.join("\n"))
}

async fn build_document_list_message(
    pool: &DbPool,
    _workspace_id: Uuid,
    status: &str,
    empty_message: &str,
    title: &str,
) -> anyhow::Result<String> {
    let documents = list_documents_by_status(pool, status, 11).await?;
    if documents.is_empty() {
        return Ok(empty_message.to_string());
    }

    let remaining = documents.len().saturating_sub(10);
    let mut lines = vec![format!("{title}:")];
    for document in documents.iter().take(10) {
        lines.push(format_document_line(document));
    }
    if remaining > 0 {
        lines.push(format!("...and {} more.", remaining));
    }

    Ok(lines.join("\n"))
}

async fn build_attention_required_message(
    pool: &DbPool,
    _workspace_id: Uuid,
) -> anyhow::Result<String> {
    let documents = list_documents_requiring_attention(pool, 11).await?;
    if documents.is_empty() {
        return Ok("No documents currently need your attention.".to_string());
    }

    let remaining = documents.len().saturating_sub(10);
    let mut lines = vec!["Documents requiring attention:".to_string()];
    for document in documents.iter().take(10) {
        lines.push(format_attention_document_line(document));
    }
    if remaining > 0 {
        lines.push(format!("...and {} more.", remaining));
    }

    Ok(lines.join("\n"))
}

async fn build_last_message(pool: &DbPool, _workspace_id: Uuid) -> anyhow::Result<String> {
    let Some(document) = latest_document(pool).await? else {
        return Ok("No documents found for this company yet.".to_string());
    };

    let mut lines = vec![
        format!("Latest document: {}", document.short_ref),
        format!("Status: {}", document.status),
    ];
    if let Some(supplier) = &document.supplier_name {
        lines.push(format!("Supplier: {}", supplier));
    }
    if let Some(amount) = &document.total_amount {
        lines.push(format!("Amount: {} SEK", amount));
    }
    if let Some(date) = &document.invoice_date {
        lines.push(format!("Date: {}", date));
    }
    if let Some(confidence) = document.confidence_score {
        lines.push(format!(
            "Confidence: {:.0}%",
            confidence_to_percent(confidence)
        ));
    }
    if let Some(reason) = &document.review_reason {
        lines.push(format!("Why: {}", reason));
    }

    Ok(lines.join("\n"))
}

async fn build_all_documents_message(pool: &DbPool, _workspace_id: Uuid) -> anyhow::Result<String> {
    let documents = list_recent_documents(pool, 26).await?;
    if documents.is_empty() {
        return Ok("No documents found for this company yet.".to_string());
    }

    let remaining = documents.len().saturating_sub(25);
    let mut lines = vec!["Recent documents:".to_string()];
    for document in documents.iter().take(25) {
        lines.push(format_document_line_with_status(document));
    }
    if remaining > 0 {
        lines.push(format!("...and {} more.", remaining));
    }

    Ok(lines.join("\n"))
}

pub async fn describe_document_why(
    pool: &DbPool,
    _workspace_id: Uuid,
    short_ref: &str,
) -> anyhow::Result<String> {
    let Some(details) = document_why_details(pool, short_ref).await? else {
        return Ok(format!(
            "Document {} was not found for this company.",
            short_ref
        ));
    };

    let failure_event = latest_failure_event_for_document(pool, details.id).await?;
    let mut lines = vec![format!(
        "{} is currently {}.",
        details.short_ref, details.status
    )];
    let mut failed_explanation: Option<ApprovalBlockExplanation> = None;

    if details.status == "FAILED" {
        if let Some(event) = failure_event.as_ref() {
            if is_system_failure_event(&event.payload) {
                lines.extend(build_system_failure_lines(
                    &details.short_ref,
                    extract_failure_stage(&event.payload),
                ));
                return Ok(lines.join("\n"));
            }
            if let Some(reason) = extract_failure_reason(&event.payload) {
                lines.push(format!("Failure reason: {}", reason));
                if let Ok(explanation) =
                    build_failed_document_explanation(pool, details.id, Some(&reason)).await
                {
                    append_explanation_detail_lines(
                        &mut lines,
                        &explanation,
                        explanation.headline_reason != reason,
                    );
                    failed_explanation = Some(explanation);
                }
                lines.push(format!("Action: retry with /retry {}", details.short_ref));
                lines.push("This is a system error, not a human rejection.".to_string());
            }
        } else {
            let headline_reason = details.review_reason.as_deref();
            if let Ok(explanation) =
                build_failed_document_explanation(pool, details.id, headline_reason).await
            {
                lines.push(format!("Reason: {}", explanation.headline_reason));
                append_explanation_detail_lines(&mut lines, &explanation, false);
                failed_explanation = Some(explanation);
            } else if let Some(reason) = headline_reason {
                lines.push(format!("Reason: {}", reason));
            }
        }
    } else if details.status == "EXPORT_READY" || details.status == "PENDING_HUMAN_REVIEW" {
        if let Some(explanation) = describe_approval_block(pool, details.id).await? {
            explanation.append_detail_lines(&mut lines);
        } else if details.status == "EXPORT_READY" {
            lines.push("Reason: this document is healthy and ready for export.".to_string());
        } else if let Some(reason) = details.review_reason.as_deref() {
            lines.push(format!("Reason: {}", reason));
        }
    } else if let Some(reason) = details.review_reason.as_deref() {
        lines.push(format!("Reason: {}", reason));
    }

    if let Some(decision) = details.decision_type.as_deref() {
        if details.status == "FAILED" {
            let explanation_contains_review_state = failed_explanation
                .as_ref()
                .is_some_and(|explanation| explanation_has_label(explanation, "Review state"));
            if !explanation_contains_review_state {
                lines.push(format!("Review state before failure: {}", decision));
            }
        } else {
            lines.push(format!("Review: {}", decision));
        }
    }
    if let Some(confidence) = details.confidence_score {
        lines.push(format!(
            "Confidence: {:.0}%",
            confidence_to_percent(confidence)
        ));
    }
    if let Some(invoice_status) = details.invoice_status.as_deref() {
        let explanation_contains_invoice_status = failed_explanation
            .as_ref()
            .is_some_and(|explanation| explanation_has_label(explanation, "Invoice status"));
        if !explanation_contains_invoice_status {
            lines.push(format!("Invoice status: {}", invoice_status));
        }
    }

    Ok(lines.join("\n"))
}

pub(crate) async fn retry_document(
    state: &AgentGatewayState,
    source: &MessageSource,
    _workspace_id: Uuid,
    short_ref: &str,
) -> anyhow::Result<String> {
    let Some(document) = retry_document_by_short_ref(&state.pool, short_ref).await? else {
        return Ok(format!(
            "Document {} was not found for this company.",
            short_ref
        ));
    };

    if !matches!(
        document.status.as_str(),
        "FAILED" | "PENDING_HUMAN_REVIEW" | "EXPORT_READY" | "EXPORTED"
    ) {
        return Ok(format!(
            "Document {} is already being processed and cannot be retried right now.",
            document.short_ref
        ));
    }

    let file_path = document
        .original_path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Retry failed. The source file path is missing."))?;
    if !Path::new(&file_path).exists() {
        return Ok(format!(
            "Retry failed for {} because the source file is no longer available.",
            document.short_ref
        ));
    }

    let mime_type = document
        .mime_type
        .clone()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let filename = document
        .filename
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let previous_status = document.status.clone();

    let mut tx = state.pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE document_interactions
        SET completed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
          AND completed_at IS NULL
        "#,
    )
    .bind(document.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM review_decisions WHERE document_id = $1")
        .bind(document.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM validation_results WHERE document_id = $1")
        .bind(document.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        r#"
        DELETE FROM account_assignments
        WHERE invoice_id IN (
            SELECT id
            FROM invoices
            WHERE document_id = $1
        )
        "#,
    )
    .bind(document.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM accounting_decisions WHERE document_id = $1")
        .bind(document.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM extracted_fields WHERE document_id = $1")
        .bind(document.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM invoices WHERE document_id = $1")
        .bind(document.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "DELETE FROM document_events WHERE document_id = $1 AND event_type NOT IN ('DOCUMENT_RECEIVED', 'DOCUMENT_REPROCESS_REQUESTED')",
    )
    .bind(document.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE documents
        SET status = 'RECEIVED',
            vision_started_at = NULL,
            vision_completed_at = NULL,
            accountant_reviewed_at = NULL,
            validated_at = NULL,
            review_completed_at = NULL,
            exported_at = NULL,
            exported_in_batch = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        "#,
    )
    .bind(document.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO document_events (document_id, event_type, payload)
        VALUES ($1, 'DOCUMENT_REPROCESS_REQUESTED', $2)
        "#,
    )
    .bind(document.id)
    .bind(json!({
        "short_ref": document.short_ref,
        "previous_status": previous_status,
        "trigger": "gateway_command",
        "channel_type": source.channel.as_str(),
        "channel_identifier": source.channel_identifier,
        "profile_identifier": source.profile_identifier,
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let context = AgentContext::new(
        state.pool.clone(),
        state.config.as_ref().clone(),
        state.events.clone(),
    );
    let intake_agent = IntakeAgent::new(context, state.queue_producer.clone());
    intake_agent
        .process(IntakeInput {
            document_id: document.id,
            file_path: file_path.into(),
            mime_type,
            filename,
        })
        .await?;

    Ok(format!(
        "Requeued {} for full reprocessing from {}.",
        document.short_ref, previous_status
    ))
}

pub(crate) async fn reopen_review_actions(
    pool: &DbPool,
    workspace_id: Uuid,
    short_ref: &str,
) -> anyhow::Result<GatewayMessageResponse> {
    let Some(document) = document_ref_by_short_ref(pool, short_ref).await? else {
        return Ok(GatewayMessageResponse::text(format!(
            "Document {} was not found for this company.",
            short_ref
        )));
    };

    match determine_review_eligibility(pool, &document).await? {
        ReviewEligibility::Reviewable => {}
        ReviewEligibility::NotReviewableSystemFailure(message)
        | ReviewEligibility::NotReviewableStatus(message) => {
            return Ok(GatewayMessageResponse::text(message));
        }
    }

    let summary = review_summary(pool, document.id).await?;
    crate::agents::db_helpers::record_document_event(
        pool,
        document.id,
        "HUMAN_REVIEW_REOPENED",
        json!({ "trigger": "gateway_command" }),
    )
    .await?;

    let attachments = source_media_attachment(pool, workspace_id, document.id).await?;

    Ok(GatewayMessageResponse {
        message: build_review_message(&summary),
        format: GatewayMessageFormat::Markdown,
        attachments,
        buttons: Some(build_review_buttons(summary.id)),
    })
}

async fn source_media_attachment(
    pool: &DbPool,
    _workspace_id: Uuid,
    document_id: i64,
) -> anyhow::Result<Vec<GatewayAttachment>> {
    let Some(artifact) = latest_source_media_artifact(pool, document_id).await? else {
        return Ok(Vec::new());
    };
    let Some(file_id) = artifact.external_file_id else {
        return Ok(Vec::new());
    };
    let Some(mime_type) = artifact.mime_type else {
        return Ok(Vec::new());
    };

    Ok(vec![GatewayAttachment::StoredMedia {
        channel_type: artifact.channel_type,
        external_file_id: file_id,
        mime_type,
        filename: artifact.filename,
    }])
}

pub(crate) async fn export_documents(
    state: &AgentGatewayState,
    source: &MessageSource,
    workspace_id: Uuid,
    args: &GatewayIntentArgs,
) -> anyhow::Result<GatewayMessageResponse> {
    if let Some(short_ref) = args.short_ref.as_deref()
        && document_ref_by_short_ref(&state.pool, short_ref)
            .await?
            .is_none()
    {
        return Ok(GatewayMessageResponse::text(format!(
            "Document {} was not found for this company.",
            short_ref
        )));
    }

    let context = AgentContext::new(
        state.pool.clone(),
        state.config.as_ref().clone(),
        state.events.clone(),
    );
    let agent = ExportAgent::new(context);
    let export_user_id = match resolve_export_user_id(&state.pool, source).await {
        Ok(user_id) => user_id,
        Err(err) => return Ok(GatewayMessageResponse::text(err.to_string())),
    };
    let output = match agent
        .process(ExportInput {
            user_id: export_user_id,
            workspace_id: Some(workspace_id),
            date_from: parse_date(args.date_from.as_deref()),
            date_to: parse_date(args.date_to.as_deref()),
            document_types: args.document_types.clone(),
            confidence_min: args.confidence_min,
            short_refs: args.short_ref.clone().map(|value| vec![value]),
        })
        .await
    {
        Ok(output) => output,
        Err(err) => {
            let error_text = err.to_string();
            let response = if error_text.contains("No documents ready for export") {
                if let Some(short_ref) = args.short_ref.as_deref() {
                    match document_ref_by_short_ref(&state.pool, short_ref).await? {
                        Some(document) => format!(
                            "{} is not currently in your export pool. Current status: {}.",
                            document.short_ref, document.status
                        ),
                        None => format!("Document {} was not found for this company.", short_ref),
                    }
                } else {
                    "No documents are currently ready in your export pool.".to_string()
                }
            } else if error_text.contains("No exportable documents found") {
                if let Some(short_ref) = args.short_ref.as_deref() {
                    format!(
                        "{} is in your export pool but cannot be exported yet because its accounting data is incomplete.",
                        short_ref
                    )
                } else {
                    "One or more documents are in your export pool but cannot be exported yet because accounting data is incomplete.".to_string()
                }
            } else {
                "Export failed. Please try again in a moment.".to_string()
            };
            return Ok(GatewayMessageResponse::text(response));
        }
    };

    let headline = if let Some(short_ref) = args.short_ref.as_deref() {
        format!("Export complete for {}.", short_ref)
    } else {
        "Export complete.".to_string()
    };

    Ok(GatewayMessageResponse {
        message: format!(
            "{}\n\nDocuments: {}\nSkipped: {}\nBatch: {}\nSending ZIP bundle now.",
            headline, output.document_count, output.skipped_documents, output.batch_id
        ),
        format: GatewayMessageFormat::Markdown,
        attachments: vec![GatewayAttachment::LocalFile {
            path: PathBuf::from(output.zip_path),
        }],
        buttons: None,
    })
}

fn parse_date(value: Option<&str>) -> Option<NaiveDate> {
    value.and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
}

async fn resolve_export_user_id(pool: &DbPool, source: &MessageSource) -> anyhow::Result<i64> {
    let channel_type = source.channel.to_string();
    let row = sqlx::query(
        r#"
        SELECT metadata
        FROM channel_identities
        WHERE channel_type = $1
          AND channel_identifier = $2
          AND active = TRUE
        ORDER BY updated_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(channel_type)
    .bind(&source.channel_identifier)
    .fetch_optional(pool)
    .await?;

    let metadata: serde_json::Value = row
        .and_then(|r| r.try_get::<Option<serde_json::Value>, _>("metadata").ok())
        .flatten()
        .ok_or_else(|| anyhow!("Telegram channel is connected without owner metadata. Reconnect it from Settings > Channels while logged in as admin, then try /export again."))?;

    let connected_by = metadata
        .get("connected_by_user_id")
        .and_then(|v| v.as_str().map(ToOwned::to_owned).or_else(|| v.as_i64().map(|n| n.to_string())))
        .ok_or_else(|| anyhow!("Telegram channel owner is missing from metadata. Reconnect the channel from Settings > Channels as admin, then try /export again."))?;

    let connected_by_user_id = connected_by.parse::<i64>().map_err(|_| {
        anyhow!(
            "Telegram channel owner metadata is invalid. Reconnect the channel from Settings > Channels as admin, then try /export again."
        )
    })?;

    let user_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
        .bind(connected_by_user_id)
        .fetch_one(pool)
        .await?;

    if !user_exists {
        return Err(anyhow!(
            "Telegram channel owner no longer exists. Reconnect the channel from Settings > Channels while logged in as admin, then try /export again."
        ));
    }

    Ok(connected_by_user_id)
}

async fn determine_review_eligibility(
    pool: &DbPool,
    document: &DocumentRef,
) -> anyhow::Result<ReviewEligibility> {
    if document.status == "PENDING_HUMAN_REVIEW" {
        return Ok(ReviewEligibility::Reviewable);
    }

    if document.status == "FAILED" {
        let failure_event = latest_failure_event_for_document(pool, document.id).await?;
        if let Some(event) = failure_event.as_ref()
            && is_system_failure_event(&event.payload)
        {
            return Ok(ReviewEligibility::NotReviewableSystemFailure(format!(
                "{} failed due to a temporary system issue, so the review actions cannot be reopened directly. Use /retry {} to restart processing, or /why {} for more detail.",
                document.short_ref, document.short_ref, document.short_ref
            )));
        }

        return Ok(ReviewEligibility::Reviewable);
    }

    let message = match document.status.as_str() {
        "EXPORT_READY" => format!(
            "{} is already in the export pool. Use /why {} to inspect any remaining blockers, or /export {} to try exporting just this document.",
            document.short_ref, document.short_ref, document.short_ref
        ),
        "EXPORTED" => format!(
            "{} has already been exported. Use /retry {} if you need to reprocess it from the start.",
            document.short_ref, document.short_ref
        ),
        _ => format!(
            "{} is not currently waiting for human review. Current status: {}. Use /why {} to inspect it.",
            document.short_ref, document.status, document.short_ref
        ),
    };

    Ok(ReviewEligibility::NotReviewableStatus(message))
}

fn build_review_message(summary: &ReviewSummary) -> String {
    let confidence_percent = summary
        .confidence_score
        .map(confidence_to_percent)
        .unwrap_or(0.0);
    [
        "Human review required".to_string(),
        String::new(),
        format!("Reference: {}", summary.short_ref),
        format!("Document ID: {}", summary.id),
        format!(
            "Supplier: {}",
            summary.supplier_name.as_deref().unwrap_or("N/A")
        ),
        format!(
            "Invoice #: {}",
            summary.invoice_number.as_deref().unwrap_or("N/A")
        ),
        format!("Date: {}", summary.invoice_date.as_deref().unwrap_or("N/A")),
        format!(
            "Total: {} SEK",
            summary.total_amount.as_deref().unwrap_or("N/A")
        ),
        format!(
            "VAT: {} SEK",
            summary.vat_amount.as_deref().unwrap_or("N/A")
        ),
        format!(
            "Account code: {}",
            summary.account_code.as_deref().unwrap_or("N/A")
        ),
        format!("Confidence: {:.0}%", confidence_percent),
        String::new(),
        "Use the actions below to edit, rerun accounting, approve, or reject.".to_string(),
    ]
    .join("\n")
}

fn build_review_buttons(document_id: i64) -> Vec<Vec<ActionButton>> {
    vec![
        vec![
            ActionButton {
                label: "Edit supplier".to_string(),
                action: DocumentAction::EditField {
                    document_id,
                    field: ReviewFieldAction::Supplier,
                },
            },
            ActionButton {
                label: "Edit amount".to_string(),
                action: DocumentAction::EditField {
                    document_id,
                    field: ReviewFieldAction::Amount,
                },
            },
        ],
        vec![
            ActionButton {
                label: "Edit date".to_string(),
                action: DocumentAction::EditField {
                    document_id,
                    field: ReviewFieldAction::Date,
                },
            },
            ActionButton {
                label: "Edit account".to_string(),
                action: DocumentAction::EditField {
                    document_id,
                    field: ReviewFieldAction::Kontonummer,
                },
            },
        ],
        vec![ActionButton {
            label: "Edit VAT".to_string(),
            action: DocumentAction::EditField {
                document_id,
                field: ReviewFieldAction::Vat,
            },
        }],
        vec![ActionButton {
            label: "Re-run accounting".to_string(),
            action: DocumentAction::RerunAccounting { document_id },
        }],
        vec![
            ActionButton {
                label: "Approve".to_string(),
                action: DocumentAction::Approve { document_id },
            },
            ActionButton {
                label: "Reject".to_string(),
                action: DocumentAction::Reject { document_id },
            },
        ],
    ]
}

fn confidence_to_percent(value: f64) -> f64 {
    value * 100.0
}

fn format_document_line(document: &DocumentSummary) -> String {
    let supplier = document
        .supplier_name
        .as_deref()
        .unwrap_or("Unknown supplier");
    let amount = document.total_amount.as_deref().unwrap_or("N/A");
    let date = document.invoice_date.as_deref().unwrap_or("N/A");
    let mut line = format!(
        "{} - {} - {} SEK - {}",
        document.short_ref, supplier, amount, date
    );

    if document.status == "EXPORT_READY" {
        if let Some(confidence) = document.confidence_score {
            line.push_str(&format!(" - {:.0}%", confidence_to_percent(confidence)));
        }
    } else if let Some(reason) = document.review_reason.as_deref() {
        line.push_str(&format!(" - {}", reason));
    }

    line
}

fn format_attention_document_line(document: &DocumentSummary) -> String {
    let action = match document.status.as_str() {
        "PENDING_HUMAN_REVIEW" => "review needed",
        "FAILED" => "failed",
        _ => "needs attention",
    };
    let supplier = document
        .supplier_name
        .as_deref()
        .unwrap_or("Unknown supplier");
    let amount = document.total_amount.as_deref().unwrap_or("N/A");
    let date = document.invoice_date.as_deref().unwrap_or("N/A");

    let mut line = format!(
        "{} - {} - {} - {} SEK - {}",
        document.short_ref, action, supplier, amount, date
    );

    if let Some(reason) = document.review_reason.as_deref() {
        line.push_str(&format!(" - {}", reason));
    } else if document.status == "FAILED" {
        line.push_str(&format!(" - use /why {} for details", document.short_ref));
    }

    line
}

fn format_document_line_with_status(document: &DocumentSummary) -> String {
    let supplier = document
        .supplier_name
        .as_deref()
        .unwrap_or("Unknown supplier");
    let amount = document.total_amount.as_deref().unwrap_or("N/A");
    let date = document.invoice_date.as_deref().unwrap_or("N/A");
    format!(
        "{} - {} - {} - {} SEK - {}",
        document.short_ref, document.status, supplier, amount, date
    )
}

fn extract_failure_reason(payload: &serde_json::Value) -> Option<String> {
    let job_type = payload.get("job_type").and_then(|value| value.as_str());
    let error = payload.get("error").and_then(|value| value.as_str());

    match (job_type, error) {
        (Some(job_type), Some(error)) => {
            Some(format!("{} failed: {}", job_type.to_lowercase(), error))
        }
        (None, Some(error)) => Some(error.to_string()),
        _ => None,
    }
}

fn is_system_failure_event(payload: &serde_json::Value) -> bool {
    payload
        .get("kind")
        .and_then(|value| value.as_str())
        .is_some_and(|kind| kind.eq_ignore_ascii_case("SYSTEM"))
        || payload
            .get("error")
            .and_then(|value| value.as_str())
            .is_some()
}

fn extract_failure_stage(payload: &serde_json::Value) -> Option<&str> {
    payload
        .get("stage")
        .and_then(|value| value.as_str())
        .or_else(|| payload.get("job_type").and_then(|value| value.as_str()))
}

fn humanize_failure_stage(stage: &str) -> &str {
    match stage {
        "VISION" => "Vision",
        "ACCOUNTANT" => "Accounting",
        "VALIDATOR" => "Validation",
        "REVIEW" => "Review",
        "EXPORT" => "Export",
        "QUEUE" => "Queue",
        "SYSTEM" => "System",
        _ => "Processing",
    }
}

fn build_system_failure_lines(short_ref: &str, stage: Option<&str>) -> Vec<String> {
    let mut lines = vec![
        "Reason: Processing could not be completed because of a temporary system issue."
            .to_string(),
    ];

    if let Some(stage) = stage {
        lines.push(format!("Stage: {}", humanize_failure_stage(stage)));
    }

    lines.push(format!("Action: retry with /retry {}", short_ref));
    lines
}

fn append_explanation_detail_lines(
    lines: &mut Vec<String>,
    explanation: &ApprovalBlockExplanation,
    include_reason: bool,
) {
    lines.push(format!(
        "Blocking stage: {}",
        explanation.blocking_stage.label()
    ));
    if include_reason {
        lines.push(format!("Reason: {}", explanation.headline_reason));
    }

    if !explanation.raw_causes.is_empty() {
        lines.push(String::new());
        lines.push("Raw causes:".to_string());
        lines.extend(
            explanation
                .raw_causes
                .iter()
                .map(|cause| format!("- {}: {}", cause.label, cause.value)),
        );
    }
}

fn explanation_has_label(explanation: &ApprovalBlockExplanation, label: &str) -> bool {
    explanation
        .raw_causes
        .iter()
        .any(|cause| cause.label == label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::review::{BlockingStage, RawBlockCause};
    use crate::db::ChannelType;

    #[test]
    fn help_message_lists_intent_commands() {
        let help = build_help_message();
        assert!(!help.contains("/start"));
        assert!(help.contains("/help"));
        assert!(help.contains("/status"));
        assert!(help.contains("/documents"));
        assert!(help.contains("/pending"));
        assert!(help.contains("/ready"));
        assert!(help.contains("/last"));
        assert!(help.contains("/why D000123"));
        assert!(help.contains("/review D000123"));
        assert!(help.contains("/retry D000123"));
        assert!(help.contains("/export D000123"));
        assert!(help.contains("plain language"));
    }

    #[test]
    fn attention_line_for_failed_without_reason_points_to_why() {
        let document = DocumentSummary {
            id: 0,
            short_ref: "D000123".to_string(),
            status: "FAILED".to_string(),
            supplier_name: Some("Test Supplier".to_string()),
            invoice_date: Some("2026-04-20".to_string()),
            total_amount: Some("149.00".to_string()),
            confidence_score: None,
            review_reason: None,
        };

        let line = format_attention_document_line(&document);
        assert_eq!(
            line,
            "D000123 - failed - Test Supplier - 149.00 SEK - 2026-04-20 - use /why D000123 for details"
        );
    }

    #[test]
    fn review_buttons_are_gateway_actions() {
        let document_id = 7_i64;
        let buttons = build_review_buttons(document_id);
        assert!(
            buttons
                .iter()
                .flatten()
                .any(|button| button.label == "Re-run accounting")
        );
    }

    #[test]
    fn system_failure_lines_use_safe_message() {
        let lines = build_system_failure_lines("D000008", Some("ACCOUNTANT"));

        assert_eq!(
            lines,
            vec![
                "Reason: Processing could not be completed because of a temporary system issue."
                    .to_string(),
                "Stage: Accounting".to_string(),
                "Action: retry with /retry D000008".to_string(),
            ]
        );
    }

    #[test]
    fn append_explanation_detail_lines_can_skip_duplicate_reason() {
        let explanation = ApprovalBlockExplanation {
            short_ref: "D000008".to_string(),
            blocking_stage: BlockingStage::Accounting,
            headline_reason: "Invalid account".to_string(),
            raw_causes: vec![RawBlockCause {
                label: "Review state".to_string(),
                value: "PENDING_HUMAN_REVIEW".to_string(),
            }],
        };

        let mut lines = Vec::new();
        append_explanation_detail_lines(&mut lines, &explanation, false);

        assert!(lines.contains(&"Blocking stage: Accounting".to_string()));
        assert!(!lines.contains(&"Reason: Invalid account".to_string()));
        assert!(explanation_has_label(&explanation, "Review state"));
    }

    #[test]
    fn parse_date_accepts_iso_dates_only() {
        assert_eq!(
            parse_date(Some("2026-05-19")),
            Some(chrono::NaiveDate::from_ymd_opt(2026, 5, 19).unwrap())
        );
        assert_eq!(parse_date(Some("19/05/2026")), None);
        assert_eq!(parse_date(None), None);
    }

    #[test]
    fn confidence_to_percent_preserves_fractional_precision() {
        assert_eq!(confidence_to_percent(0.875), 87.5);
    }

    #[test]
    fn document_line_formats_ready_confidence_and_review_reason() {
        let ready = DocumentSummary {
            id: 1,
            short_ref: "D000001".to_string(),
            status: "EXPORT_READY".to_string(),
            supplier_name: Some("Supplier AB".to_string()),
            invoice_date: Some("2026-05-19".to_string()),
            total_amount: Some("123.45".to_string()),
            confidence_score: Some(0.91),
            review_reason: Some("ignored for ready rows".to_string()),
        };
        assert_eq!(
            format_document_line(&ready),
            "D000001 - Supplier AB - 123.45 SEK - 2026-05-19 - 91%"
        );

        let pending = DocumentSummary {
            id: 2,
            short_ref: "D000002".to_string(),
            status: "PENDING_HUMAN_REVIEW".to_string(),
            supplier_name: None,
            invoice_date: None,
            total_amount: None,
            confidence_score: Some(0.1),
            review_reason: Some("Missing VAT".to_string()),
        };
        assert_eq!(
            format_document_line(&pending),
            "D000002 - Unknown supplier - N/A SEK - N/A - Missing VAT"
        );
    }

    #[test]
    fn failure_reason_and_stage_are_extracted_from_payload() {
        let payload = serde_json::json!({
            "job_type": "ACCOUNTANT",
            "stage": "VALIDATOR",
            "error": "model unavailable"
        });

        assert_eq!(
            extract_failure_reason(&payload),
            Some("accountant failed: model unavailable".to_string())
        );
        assert!(is_system_failure_event(&payload));
        assert_eq!(extract_failure_stage(&payload), Some("VALIDATOR"));
        assert_eq!(humanize_failure_stage("VALIDATOR"), "Validation");
        assert_eq!(humanize_failure_stage("UNKNOWN"), "Processing");
    }

    #[tokio::test]
    async fn resolve_export_user_id_reads_connected_by_user_id_from_channel_metadata() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("sqlite memory pool");
        sqlx::query(
            r#"
            CREATE TABLE users (
              id INTEGER PRIMARY KEY,
              role TEXT NOT NULL,
              email TEXT NOT NULL UNIQUE,
              password_hash TEXT NOT NULL,
              display_name TEXT,
              created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
              updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create users table");
        sqlx::query(
            "INSERT INTO users (id, role, email, password_hash) VALUES (7, 'admin', 'owner@example.com', 'hash')",
        )
        .execute(&pool)
        .await
        .expect("insert owner user");
        sqlx::query(
            r#"
            CREATE TABLE channel_identities (
              id INTEGER PRIMARY KEY,
              channel_type TEXT NOT NULL,
              channel_identifier TEXT NOT NULL,
              metadata TEXT,
              active INTEGER NOT NULL DEFAULT 1,
              created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
              updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create table");
        sqlx::query(
            "INSERT INTO channel_identities (channel_type, channel_identifier, metadata, active) VALUES ('TELEGRAM', '12345', $1, 1)",
        )
        .bind(serde_json::json!({ "connected_by_user_id": "7" }))
        .execute(&pool)
        .await
        .expect("insert channel identity");

        let source = MessageSource {
            channel: ChannelType::Telegram,
            channel_identifier: "12345".to_string(),
            profile_identifier: None,
            message_id: None,
            source_timestamp: None,
            metadata: serde_json::json!({}),
        };

        let user_id = resolve_export_user_id(&pool, &source)
            .await
            .expect("resolve export user id");
        assert_eq!(user_id, 7);
    }

    #[tokio::test]
    async fn resolve_export_user_id_fails_when_owner_metadata_missing() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("sqlite memory pool");
        sqlx::query(
            r#"
            CREATE TABLE channel_identities (
              id INTEGER PRIMARY KEY,
              channel_type TEXT NOT NULL,
              channel_identifier TEXT NOT NULL,
              metadata TEXT,
              active INTEGER NOT NULL DEFAULT 1,
              created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
              updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create table");
        sqlx::query(
            "INSERT INTO channel_identities (channel_type, channel_identifier, metadata, active) VALUES ('TELEGRAM', '12345', '{}', 1)",
        )
        .execute(&pool)
        .await
        .expect("insert channel identity");

        let source = MessageSource {
            channel: ChannelType::Telegram,
            channel_identifier: "12345".to_string(),
            profile_identifier: None,
            message_id: None,
            source_timestamp: None,
            metadata: serde_json::json!({}),
        };

        let err = resolve_export_user_id(&pool, &source)
            .await
            .expect_err("missing owner metadata should fail");
        assert!(err.to_string().contains("Reconnect the channel"));
    }

    #[tokio::test]
    async fn resolve_export_user_id_fails_when_owner_user_no_longer_exists() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("sqlite memory pool");
        sqlx::query(
            r#"
            CREATE TABLE users (
              id INTEGER PRIMARY KEY,
              role TEXT NOT NULL,
              email TEXT NOT NULL UNIQUE,
              password_hash TEXT NOT NULL,
              display_name TEXT,
              created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
              updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create users table");
        sqlx::query(
            r#"
            CREATE TABLE channel_identities (
              id INTEGER PRIMARY KEY,
              channel_type TEXT NOT NULL,
              channel_identifier TEXT NOT NULL,
              metadata TEXT,
              active INTEGER NOT NULL DEFAULT 1,
              created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
              updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create channel_identities table");
        sqlx::query(
            "INSERT INTO channel_identities (channel_type, channel_identifier, metadata, active) VALUES ('TELEGRAM', '12345', $1, 1)",
        )
        .bind(serde_json::json!({ "connected_by_user_id": "77" }))
        .execute(&pool)
        .await
        .expect("insert channel identity");

        let source = MessageSource {
            channel: ChannelType::Telegram,
            channel_identifier: "12345".to_string(),
            profile_identifier: None,
            message_id: None,
            source_timestamp: None,
            metadata: serde_json::json!({}),
        };

        let err = resolve_export_user_id(&pool, &source)
            .await
            .expect_err("stale owner should fail");
        assert!(err.to_string().contains("owner no longer exists"));
    }
}
