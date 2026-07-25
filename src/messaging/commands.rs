use std::path::{Path, PathBuf};

use crate::db::{ChannelType, DbPool};
use anyhow::anyhow;
use chrono::NaiveDate;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use super::contracts::{
    ActionButton, DocumentAction, GatewayAttachment, GatewayMessageFormat, GatewayMessageResponse,
    MessageSource, ReviewFieldAction,
};
use super::gateway::AgentGatewayState;
use super::intents::GatewayIntentArgs;
use crate::agents::review::ApprovalBlockExplanation;
use crate::agents::{
    Agent, AgentContext, ExportAgent, ExportInput, IntakeAgent, IntakeInput,
    build_failed_document_explanation, describe_approval_block,
};
use crate::document_state;
#[cfg(test)]
use crate::query::DocumentSummary;
use crate::query::{
    AccountingProcessingCandidate, DocumentRef, DocumentWhyDetails, ReviewSummary,
    accounting_eligible_candidates, accounting_processing_candidate_by_short_ref,
    accounting_processing_candidates_by_short_refs, document_ref_by_short_ref,
    document_why_details, latest_failure_event_for_document, latest_source_media_artifact,
    retry_document_by_short_ref, review_summary,
};
use crate::queue::{Job, JobType, QueueProducer};

enum ReviewEligibility {
    Reviewable,
    NotReviewableSystemFailure(String),
    NotReviewableStatus(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AccountingProcessingSelection {
    One(String),
    Many(Vec<String>),
    AllEligible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountingProcessingSkip {
    pub short_ref: Option<String>,
    pub status: Option<String>,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountingProcessingPreview {
    pub eligible_documents: Vec<AccountingProcessingCandidate>,
    pub skipped: Vec<AccountingProcessingSkip>,
    pub all_eligible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountingProcessingExecution {
    pub queued_documents: Vec<AccountingProcessingCandidate>,
}

pub fn build_help_message() -> String {
    "Welcome to Finelor.\n\nUse Finelor in plain language. You can ask for document status, why a document is blocked, what needs attention, what is ready for export, and what to retry, review, or export. You can also upload an invoice or receipt directly in chat as a file or image.\n\nExamples:\n- \"What is the status now?\"\n- \"Why is D000123 blocked?\"\n- \"Show documents that need attention\"\n- \"Open review for D000123\"\n- \"Retry D000123\"\n- \"Export ready documents\"\n- \"I want to upload a receipt\"\n\nSlash commands are no longer needed for operations. /help is available if you want this guide again."
        .to_string()
}

pub async fn describe_document_why(pool: &DbPool, short_ref: &str) -> anyhow::Result<String> {
    let Some(details) = document_why_details(pool, short_ref).await? else {
        return Ok(format!(
            "Document {} was not found for this company.",
            short_ref
        ));
    };

    let failure_event = latest_failure_event_for_document(pool, details.id).await?;
    let mut lines = vec![format!(
        "{} is currently {}.",
        details.short_ref,
        document_why_state_text(&details)
    )];
    let mut failed_explanation: Option<ApprovalBlockExplanation> = None;
    let has_failure = has_domain_failure(
        details.intake_status.as_deref(),
        details.accounting_status.as_deref(),
    );
    let pending_review = matches!(details.accounting_status.as_deref(), Some("PENDING_REVIEW"));
    let export_ready = matches!(
        details.accounting_status.as_deref(),
        Some("READY_FOR_EXPORT")
    );

    if has_failure {
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
                lines.push(format!("Action: ask me to retry {}", details.short_ref));
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
    } else if export_ready || pending_review {
        if let Some(explanation) = describe_approval_block(pool, details.id).await? {
            explanation.append_detail_lines(&mut lines);
        } else if export_ready {
            lines.push("Reason: this document is healthy and ready for export.".to_string());
        } else if let Some(reason) = details.review_reason.as_deref() {
            lines.push(format!("Reason: {}", reason));
        }
    } else if let Some(reason) = details.review_reason.as_deref() {
        lines.push(format!("Reason: {}", reason));
    }

    if let Some(decision) = details.decision_type.as_deref() {
        if has_failure {
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
    short_ref: &str,
) -> anyhow::Result<String> {
    let Some(document) = retry_document_by_short_ref(&state.pool, short_ref).await? else {
        return Ok(format!(
            "Document {} was not found for this company.",
            short_ref
        ));
    };

    if !retry_is_allowed(&document) {
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
    let previous_status = retry_document_state_text(&document);

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
        SET updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        "#,
    )
    .bind(document.id)
    .execute(&mut *tx)
    .await?;
    document_state::reset_document_state_in_tx(&mut tx, document.id).await?;
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

pub(crate) async fn preview_accounting_processing_selection(
    pool: &DbPool,
    selection: AccountingProcessingSelection,
) -> anyhow::Result<AccountingProcessingPreview> {
    match selection {
        AccountingProcessingSelection::One(short_ref) => {
            let candidate = accounting_processing_candidate_by_short_ref(pool, &short_ref).await?;
            Ok(AccountingProcessingPreview {
                eligible_documents: candidate
                    .iter()
                    .filter(|candidate| candidate_is_accounting_eligible(candidate))
                    .cloned()
                    .collect(),
                skipped: match candidate {
                    Some(candidate) => candidate_skip_reason(&candidate).into_iter().collect(),
                    None => vec![AccountingProcessingSkip {
                        short_ref: Some(short_ref),
                        status: None,
                        reason: "not found",
                    }],
                },
                all_eligible: false,
            })
        }
        AccountingProcessingSelection::Many(short_refs) => {
            let candidates =
                accounting_processing_candidates_by_short_refs(pool, &short_refs).await?;
            let mut candidate_by_ref = std::collections::HashMap::new();
            for candidate in candidates {
                candidate_by_ref.insert(candidate.short_ref.clone(), candidate);
            }

            let mut eligible_documents = Vec::new();
            let mut skipped = Vec::new();

            for short_ref in short_refs {
                match candidate_by_ref.remove(&short_ref) {
                    Some(candidate) if candidate_is_accounting_eligible(&candidate) => {
                        eligible_documents.push(candidate);
                    }
                    Some(candidate) => {
                        if let Some(skip) = candidate_skip_reason(&candidate) {
                            skipped.push(skip);
                        }
                    }
                    None => skipped.push(AccountingProcessingSkip {
                        short_ref: Some(short_ref),
                        status: None,
                        reason: "not found",
                    }),
                }
            }

            Ok(AccountingProcessingPreview {
                eligible_documents,
                skipped,
                all_eligible: false,
            })
        }
        AccountingProcessingSelection::AllEligible => Ok(AccountingProcessingPreview {
            eligible_documents: accounting_eligible_candidates(pool).await?,
            skipped: Vec::new(),
            all_eligible: true,
        }),
    }
}

pub(crate) async fn execute_accounting_processing_request(
    pool: &DbPool,
    queue_producer: &QueueProducer,
    documents: &[AccountingProcessingCandidate],
) -> anyhow::Result<AccountingProcessingExecution> {
    if documents.is_empty() {
        return Ok(AccountingProcessingExecution {
            queued_documents: Vec::new(),
        });
    }

    let mut tx = pool.begin().await?;

    for document in documents {
        document_state::request_accounting_in_tx(&mut tx, document.id).await?;

        crate::agents::db_helpers::record_document_event_in_tx(
            &mut tx,
            document.id,
            "ACCOUNTING_PROCESSING_REQUESTED",
            json!({ "document_short_ref": document.short_ref }),
        )
        .await?;
    }

    tx.commit().await?;

    for document in documents {
        queue_producer
            .enqueue(&Job::new(JobType::Accountant, document.id, 0))
            .await?;
        crate::agents::db_helpers::record_document_event(
            pool,
            document.id,
            "ACCOUNTING_PROCESSING_QUEUED",
            json!({ "document_short_ref": document.short_ref }),
        )
        .await?;
    }

    Ok(AccountingProcessingExecution {
        queued_documents: documents.to_vec(),
    })
}

pub(crate) fn describe_accounting_processing_preview(
    preview: &AccountingProcessingPreview,
) -> String {
    if preview.eligible_documents.is_empty() {
        let skipped = describe_accounting_processing_skips(&preview.skipped);
        return if skipped.is_empty() {
            "No ingested documents are currently eligible for accounting processing.".to_string()
        } else {
            format!(
                "No selected documents are eligible for accounting processing.\n\n{}",
                skipped
            )
        };
    }

    let eligible_refs = preview
        .eligible_documents
        .iter()
        .map(|document| document.short_ref.clone())
        .collect::<Vec<_>>();

    let lead = if preview.all_eligible {
        format!(
            "Found {} ingested document(s) that can be sent to accounting: {}.",
            preview.eligible_documents.len(),
            eligible_refs.join(", ")
        )
    } else {
        format!(
            "{} selected document(s) can be sent to accounting: {}.",
            preview.eligible_documents.len(),
            eligible_refs.join(", ")
        )
    };

    if preview.skipped.is_empty() {
        format!("{lead}\n\nConfirm to start accounting processing.")
    } else {
        format!(
            "{lead}\n\n{}\n\nConfirm to process only the eligible documents.",
            describe_accounting_processing_skips(&preview.skipped)
        )
    }
}

pub(crate) fn describe_accounting_processing_execution(
    execution: &AccountingProcessingExecution,
) -> String {
    let refs = execution
        .queued_documents
        .iter()
        .map(|document| document.short_ref.clone())
        .collect::<Vec<_>>();
    format!(
        "Queued {} ingested document(s) for accounting: {}.",
        execution.queued_documents.len(),
        refs.join(", ")
    )
}

fn candidate_is_accounting_eligible(candidate: &AccountingProcessingCandidate) -> bool {
    candidate.intake_status == "INGESTED" && candidate.accounting_status == "NOT_REQUESTED"
}

fn has_domain_failure(intake_status: Option<&str>, accounting_status: Option<&str>) -> bool {
    matches!(intake_status, Some("FAILED")) || matches!(accounting_status, Some("FAILED"))
}

fn retry_is_allowed(document: &crate::query::RetryDocument) -> bool {
    has_domain_failure(
        document.intake_status.as_deref(),
        document.accounting_status.as_deref(),
    ) || matches!(
        document.accounting_status.as_deref(),
        Some("PENDING_REVIEW" | "READY_FOR_EXPORT" | "EXPORTED")
    )
}

fn candidate_skip_reason(
    candidate: &AccountingProcessingCandidate,
) -> Option<AccountingProcessingSkip> {
    if candidate_is_accounting_eligible(candidate) {
        return None;
    }

    let reason = if candidate.accounting_status == "REQUESTED"
        || candidate.accounting_requested_at.is_some()
    {
        "already requested"
    } else if matches!(candidate.intake_status.as_str(), "RECEIVED" | "PROCESSING") {
        "not ready yet"
    } else {
        "already processed or in progress"
    };

    Some(AccountingProcessingSkip {
        short_ref: Some(candidate.short_ref.clone()),
        status: Some(summarize_document_state(
            Some(candidate.intake_status.as_str()),
            Some(candidate.accounting_status.as_str()),
        )),
        reason,
    })
}

fn describe_accounting_processing_skips(skipped: &[AccountingProcessingSkip]) -> String {
    if skipped.is_empty() {
        return String::new();
    }

    let mut lines = vec!["These documents cannot be processed right now:".to_string()];
    for skip in skipped {
        match (&skip.short_ref, &skip.status) {
            (Some(short_ref), Some(status)) => {
                lines.push(format!("- {}: {} ({})", short_ref, skip.reason, status));
            }
            (Some(short_ref), None) => lines.push(format!("- {}: {}", short_ref, skip.reason)),
            (None, Some(status)) => lines.push(format!("- {} ({})", skip.reason, status)),
            (None, None) => lines.push(format!("- {}", skip.reason)),
        }
    }
    lines.join("\n")
}

pub(crate) async fn reopen_review_actions(
    pool: &DbPool,
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

    let attachments = source_media_attachment(pool, document.id).await?;

    Ok(GatewayMessageResponse {
        message: build_review_message(&summary),
        format: GatewayMessageFormat::Markdown,
        attachments,
        buttons: Some(build_review_buttons(summary.id)),
    })
}

async fn source_media_attachment(
    pool: &DbPool,
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
    if let Some(short_ref) = args.document_short_ref.as_deref()
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
            short_refs: args.document_short_ref.clone().map(|value| vec![value]),
        })
        .await
    {
        Ok(output) => output,
        Err(err) => {
            let error_text = err.to_string();
            let response = if error_text.contains("No documents ready for export") {
                if let Some(short_ref) = args.document_short_ref.as_deref() {
                    match document_ref_by_short_ref(&state.pool, short_ref).await? {
                        Some(document) => format!(
                            "{} is not currently in your export pool. Current status: {}.",
                            document.short_ref,
                            document_ref_state_text(&document)
                        ),
                        None => format!("Document {} was not found for this company.", short_ref),
                    }
                } else {
                    "No documents are currently ready in your export pool.".to_string()
                }
            } else if error_text.contains("No exportable documents found") {
                if let Some(short_ref) = args.document_short_ref.as_deref() {
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

    let headline = if let Some(short_ref) = args.document_short_ref.as_deref() {
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

    let metadata: Option<serde_json::Value> = row
        .and_then(|r| r.try_get::<Option<serde_json::Value>, _>("metadata").ok())
        .flatten();

    let connected_by_user_id = match metadata
        .as_ref()
        .and_then(|metadata| metadata.get("connected_by_user_id"))
        .and_then(|v| {
            v.as_str()
                .map(ToOwned::to_owned)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        }) {
        Some(connected_by) => connected_by.parse::<i64>().map_err(|_| {
            anyhow!(
                "Connected channel owner metadata is invalid. Reconnect the channel from Settings > Channels as admin, then ask me to export again."
            )
        })?,
        None if source.channel == ChannelType::Slack => fallback_export_user_id(pool).await?,
        None => {
            return Err(anyhow!(
                "Connected channel owner is missing from metadata. Reconnect the channel from Settings > Channels as admin, then ask me to export again."
            ));
        }
    };

    let user_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
        .bind(connected_by_user_id)
        .fetch_one(pool)
        .await?;

    if !user_exists {
        return Err(anyhow!(
            "Connected channel owner no longer exists. Reconnect the channel from Settings > Channels while logged in as admin, then ask me to export again."
        ));
    }

    Ok(connected_by_user_id)
}

async fn fallback_export_user_id(pool: &DbPool) -> anyhow::Result<i64> {
    sqlx::query_scalar(
        r#"
        SELECT id
        FROM users
        ORDER BY CASE WHEN role = 'admin' THEN 0 ELSE 1 END, id ASC
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        anyhow!(
            "No user exists to attribute the export. Create an admin user, then ask me to export again."
        )
    })
}

async fn determine_review_eligibility(
    pool: &DbPool,
    document: &DocumentRef,
) -> anyhow::Result<ReviewEligibility> {
    if matches!(
        document.accounting_status.as_deref(),
        Some("PENDING_REVIEW")
    ) {
        return Ok(ReviewEligibility::Reviewable);
    }

    if has_domain_failure(
        document.intake_status.as_deref(),
        document.accounting_status.as_deref(),
    ) {
        let failure_event = latest_failure_event_for_document(pool, document.id).await?;
        if let Some(event) = failure_event.as_ref()
            && is_system_failure_event(&event.payload)
        {
            return Ok(ReviewEligibility::NotReviewableSystemFailure(format!(
                "{} failed due to a temporary system issue, so the review actions cannot be reopened directly. Ask me to retry {} to restart processing, or ask why {} is blocked for more detail.",
                document.short_ref, document.short_ref, document.short_ref
            )));
        }

        return Ok(ReviewEligibility::Reviewable);
    }

    let message = if matches!(
        document.accounting_status.as_deref(),
        Some("READY_FOR_EXPORT")
    ) {
        format!(
            "{} is already in the export pool. Ask why {} is blocked to inspect any remaining blockers, or ask me to export {} if you want to export just this document.",
            document.short_ref, document.short_ref, document.short_ref
        )
    } else if matches!(document.accounting_status.as_deref(), Some("EXPORTED")) {
        format!(
            "{} has already been exported. Ask me to retry {} if you need to reprocess it from the start.",
            document.short_ref, document.short_ref
        )
    } else {
        format!(
            "{} is not currently waiting for human review. Current status: {}. Ask why {} is blocked if you want more detail.",
            document.short_ref,
            document_ref_state_text(document),
            document.short_ref
        )
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

fn summarize_document_state(
    intake_status: Option<&str>,
    accounting_status: Option<&str>,
) -> String {
    if matches!(intake_status, Some("FAILED")) {
        return "intake failed".to_string();
    }
    if matches!(accounting_status, Some("FAILED")) {
        return "accounting failed".to_string();
    }
    if matches!(accounting_status, Some("EXPORTED")) {
        return "exported".to_string();
    }
    if matches!(accounting_status, Some("EXPORTING")) {
        return "exporting".to_string();
    }
    if matches!(accounting_status, Some("READY_FOR_EXPORT")) {
        return "ready for export".to_string();
    }
    if matches!(accounting_status, Some("PENDING_REVIEW")) {
        return "pending review".to_string();
    }
    if matches!(accounting_status, Some("VALIDATING")) {
        return "validating".to_string();
    }
    if matches!(accounting_status, Some("ACCOUNTING")) {
        return "accounting processing".to_string();
    }
    if matches!(accounting_status, Some("REQUESTED")) {
        return "accounting requested".to_string();
    }
    if matches!(intake_status, Some("INGESTED")) {
        return "ingested".to_string();
    }
    if matches!(intake_status, Some("PROCESSING")) {
        return "intake processing".to_string();
    }
    "received".to_string()
}

fn document_ref_state_text(document: &DocumentRef) -> String {
    summarize_document_state(
        document.intake_status.as_deref(),
        document.accounting_status.as_deref(),
    )
}

fn retry_document_state_text(document: &crate::query::RetryDocument) -> String {
    summarize_document_state(
        document.intake_status.as_deref(),
        document.accounting_status.as_deref(),
    )
}

fn document_why_state_text(document: &DocumentWhyDetails) -> String {
    summarize_document_state(
        document.intake_status.as_deref(),
        document.accounting_status.as_deref(),
    )
}

#[cfg(test)]
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

    if matches!(
        document.accounting_status.as_deref(),
        Some("READY_FOR_EXPORT")
    ) {
        if let Some(confidence) = document.confidence_score {
            line.push_str(&format!(" - {:.0}%", confidence_to_percent(confidence)));
        }
    } else if let Some(reason) = document.review_reason.as_deref() {
        line.push_str(&format!(" - {}", reason));
    }

    line
}

#[cfg(test)]
fn format_attention_document_line(document: &DocumentSummary) -> String {
    let action = if matches!(
        document.accounting_status.as_deref(),
        Some("PENDING_REVIEW")
    ) {
        "review needed"
    } else if has_domain_failure(
        document.intake_status.as_deref(),
        document.accounting_status.as_deref(),
    ) {
        "failed"
    } else {
        "needs attention"
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
    } else if has_domain_failure(
        document.intake_status.as_deref(),
        document.accounting_status.as_deref(),
    ) {
        line.push_str(&format!(
            " - ask why {} is blocked for details",
            document.short_ref
        ));
    }

    line
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

    lines.push(format!("Action: ask me to retry {}", short_ref));
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
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    #[test]
    fn help_message_lists_intent_commands() {
        let help = build_help_message();
        assert!(!help.contains("/start"));
        assert!(help.contains("/help"));
        assert!(!help.contains("/status"));
        assert!(!help.contains("/documents"));
        assert!(!help.contains("/pending"));
        assert!(!help.contains("/ready"));
        assert!(!help.contains("/last"));
        assert!(!help.contains("/why D000123"));
        assert!(!help.contains("/review D000123"));
        assert!(!help.contains("/retry D000123"));
        assert!(!help.contains("/export D000123"));
        assert!(help.contains("upload an invoice or receipt directly in chat"));
        assert!(help.contains("plain language"));
    }

    #[test]
    fn attention_line_for_failed_without_reason_points_to_why() {
        let document = DocumentSummary {
            id: 0,
            short_ref: "D000123".to_string(),
            intake_status: Some("FAILED".to_string()),
            accounting_status: Some("NOT_REQUESTED".to_string()),
            supplier_name: Some("Test Supplier".to_string()),
            invoice_date: Some("2026-04-20".to_string()),
            total_amount: Some("149.00".to_string()),
            confidence_score: None,
            review_reason: None,
        };

        let line = format_attention_document_line(&document);
        assert_eq!(
            line,
            "D000123 - failed - Test Supplier - 149.00 SEK - 2026-04-20 - ask why D000123 is blocked for details"
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
                "Action: ask me to retry D000008".to_string(),
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
            intake_status: Some("INGESTED".to_string()),
            accounting_status: Some("READY_FOR_EXPORT".to_string()),
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
            intake_status: Some("INGESTED".to_string()),
            accounting_status: Some("PENDING_REVIEW".to_string()),
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
    fn retry_is_allowed_uses_domain_state_not_flat_status() {
        let reviewable = crate::query::RetryDocument {
            id: 1,
            short_ref: "D000010".to_string(),
            intake_status: Some("INGESTED".to_string()),
            accounting_status: Some("PENDING_REVIEW".to_string()),
            review_reason: Some("Missing VAT".to_string()),
            original_path: Some("/tmp/doc.pdf".to_string()),
            mime_type: Some("application/pdf".to_string()),
            filename: Some("doc.pdf".to_string()),
        };
        assert!(retry_is_allowed(&reviewable));

        let in_flight = crate::query::RetryDocument {
            accounting_status: Some("VALIDATING".to_string()),
            review_reason: None,
            ..reviewable
        };
        assert!(!retry_is_allowed(&in_flight));
    }

    #[test]
    fn has_domain_failure_checks_domain_tables_directly() {
        assert!(has_domain_failure(Some("FAILED"), Some("NOT_REQUESTED")));
        assert!(has_domain_failure(Some("INGESTED"), Some("FAILED")));
        assert!(!has_domain_failure(
            Some("INGESTED"),
            Some("READY_FOR_EXPORT")
        ));
    }

    #[tokio::test]
    async fn preview_accounting_processing_selection_reports_eligible_and_skipped_documents() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .in_memory(true)
                    .foreign_keys(true),
            )
            .await
            .expect("connect");
        crate::db::run_migrations(&pool).await.expect("migrate");

        for preset in [
            crate::document_state::TestDocumentStatePreset::IntakeIngested,
            crate::document_state::TestDocumentStatePreset::AccountingRequested,
            crate::document_state::TestDocumentStatePreset::IntakeProcessing,
        ] {
            let document_id: i64 = sqlx::query_scalar(
                r#"
                INSERT INTO documents (filename, file_hash, original_path, mime_type)
                VALUES ('doc.pdf', $1, '/tmp/doc.pdf', 'application/pdf')
                RETURNING id
                "#,
            )
            .bind(format!("preview-{}", uuid::Uuid::new_v4()))
            .fetch_one(&pool)
            .await
            .expect("insert document");
            crate::document_state::seed_document_state_preset(&pool, document_id, preset)
                .await
                .expect("seed document status");
        }

        let refs: Vec<String> =
            sqlx::query_scalar("SELECT short_ref FROM documents ORDER BY id ASC")
                .fetch_all(&pool)
                .await
                .expect("refs");

        let preview = preview_accounting_processing_selection(
            &pool,
            AccountingProcessingSelection::Many(refs),
        )
        .await
        .expect("preview");

        assert_eq!(preview.eligible_documents.len(), 1);
        assert_eq!(preview.skipped.len(), 2);
        assert!(
            preview
                .skipped
                .iter()
                .any(|skip| skip.reason == "already requested")
        );
        assert!(
            preview
                .skipped
                .iter()
                .any(|skip| skip.reason == "not ready yet")
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
        let pool = DbPool::connect("sqlite::memory:")
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
        let pool = DbPool::connect("sqlite::memory:")
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
        let pool = DbPool::connect("sqlite::memory:")
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
