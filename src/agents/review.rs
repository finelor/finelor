//! Review Agent for Finelor
//!
//! Decision logic for document review:
//! - Auto-approve >= 0.75 confidence
//! - Human review 0.50-0.75 confidence
//! - Quarantine < 0.50 confidence

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqliteConnection;
use tracing::info;

use crate::agents::export::{ExportAgent, summarize_block_reasons};
use crate::agents::{Agent, AgentContext, DocumentStatus, FieldLoadMode, db_helpers};
use crate::confidence::{ReviewAction, ReviewThresholds};
use crate::db::DbPool;
use crate::error::{AppError, AppResult};
use crate::queue::{Job, JobType, QueueProducer};

/// Input for the ReviewAgent
#[derive(Debug, Clone)]
pub struct ReviewInput {
    pub document_id: i64,
    pub force_human_review: bool,
}

/// Output from the ReviewAgent  
#[derive(Debug, Clone)]
pub struct ReviewOutput {
    pub document_id: i64,
    pub decision: ReviewDecision,
    pub action_taken: ReviewAction,
}

/// The review decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewDecision {
    pub action: ReviewAction,
    pub confidence_score: f64,
    pub requires_human_review: bool,
    pub review_reason: Option<String>,
    pub human_decision: Option<HumanDecision>,
}

/// Human review decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanDecision {
    pub approved: bool,
    pub corrections: Option<FieldCorrections>,
    pub reviewed_by: String,
    pub reviewed_at: chrono::DateTime<chrono::Utc>,
}

/// Field corrections from human review
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FieldCorrections {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supplier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kontonummer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vat: Option<String>,
}

impl FieldCorrections {
    pub fn has_corrections(&self) -> bool {
        self.supplier.is_some()
            || self.amount.is_some()
            || self.date.is_some()
            || self.kontonummer.is_some()
            || self.vat.is_some()
    }
}

/// ReviewAgent implements review decision logic
pub struct ReviewAgent {
    context: AgentContext,
    // TODO: used by future Telegram review flow
    #[allow(dead_code)]
    queue_producer: QueueProducer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockingStage {
    Accounting,
    Validation,
    Review,
    Export,
    System,
    Unknown,
}

impl BlockingStage {
    pub fn label(self) -> &'static str {
        match self {
            BlockingStage::Accounting => "Accounting",
            BlockingStage::Validation => "Validation",
            BlockingStage::Review => "Review",
            BlockingStage::Export => "Export",
            BlockingStage::System => "System",
            BlockingStage::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawBlockCause {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct ApprovalBlockExplanation {
    pub short_ref: String,
    pub blocking_stage: BlockingStage,
    pub headline_reason: String,
    pub raw_causes: Vec<RawBlockCause>,
}

#[derive(Debug, Clone)]
pub struct HumanReviewCallbackResult {
    pub callback_text: String,
    pub chat_message: String,
    pub keyboard: Option<HumanReviewKeyboard>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanReviewKeyboard {
    pub rows: Vec<Vec<HumanReviewButton>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanReviewButton {
    pub label: String,
    pub callback_data: String,
}

impl ApprovalBlockExplanation {
    fn detail_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("Blocking stage: {}", self.blocking_stage.label()),
            format!("Reason: {}", self.headline_reason),
        ];

        if !self.raw_causes.is_empty() {
            lines.push(String::new());
            lines.push("Raw causes:".to_string());
            lines.extend(
                self.raw_causes
                    .iter()
                    .map(|cause| format!("- {}: {}", cause.label, cause.value)),
            );
        }

        lines
    }

    pub fn as_approval_message(&self) -> String {
        let mut lines = vec![
            format!("{} cannot be approved yet.", self.short_ref),
            String::new(),
        ];
        lines.extend(self.detail_lines());
        lines.join("\n")
    }

    pub fn append_detail_lines(&self, lines: &mut Vec<String>) {
        lines.extend(self.detail_lines());
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ApprovalBlockContext {
    short_ref: String,
    review_reason: Option<String>,
    decision_type: Option<String>,
    invoice_status: Option<String>,
    accountant_reasoning_text: Option<String>,
    validation_errors: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewField {
    Supplier,
    Amount,
    Date,
    Kontonummer,
    Vat,
}

impl ReviewField {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewField::Supplier => "supplier",
            ReviewField::Amount => "amount",
            ReviewField::Date => "date",
            ReviewField::Kontonummer => "kontonummer",
            ReviewField::Vat => "vat",
        }
    }

    pub fn prompt_label(self) -> &'static str {
        match self {
            ReviewField::Supplier => "supplier",
            ReviewField::Amount => "total amount",
            ReviewField::Date => "date",
            ReviewField::Kontonummer => "account code",
            ReviewField::Vat => "VAT amount",
        }
    }
}

impl std::str::FromStr for ReviewField {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "supplier" => Ok(Self::Supplier),
            "amount" => Ok(Self::Amount),
            "date" => Ok(Self::Date),
            "kontonummer" => Ok(Self::Kontonummer),
            "vat" => Ok(Self::Vat),
            other => Err(AppError::Agent(format!("Unknown review field: {other}"))),
        }
    }
}

impl ReviewAgent {
    pub fn new(context: AgentContext, queue_producer: QueueProducer) -> Self {
        Self {
            context,
            queue_producer,
        }
    }

    /// Apply human corrections to document
    // TODO: used by future Telegram review flow
    #[allow(dead_code)]
    async fn apply_corrections(
        &self,
        document_id: i64,
        corrections: &FieldCorrections,
    ) -> AppResult<()> {
        let mut tx = self.context.pool.begin().await?;

        if let Some(ref supplier) = corrections.supplier {
            sqlx::query(
                r#"
                UPDATE extracted_fields 
                SET parsed_value = $1, updated_at = CURRENT_TIMESTAMP
                WHERE document_id = $2 AND field_type = 'supplier_name'
                "#,
            )
            .bind(supplier)
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
        }

        if let Some(ref amount) = corrections.amount {
            sqlx::query(
                r#"
                UPDATE extracted_fields 
                SET parsed_value = $1, updated_at = CURRENT_TIMESTAMP
                WHERE document_id = $2 AND field_type = 'total_amount'
                "#,
            )
            .bind(amount)
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
        }

        if let Some(ref date) = corrections.date {
            sqlx::query(
                r#"
                UPDATE extracted_fields 
                SET parsed_value = $1, updated_at = CURRENT_TIMESTAMP
                WHERE document_id = $2 AND field_type = 'transaction_date'
                "#,
            )
            .bind(date)
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
        }

        if let Some(ref kontonummer) = corrections.kontonummer {
            sqlx::query(
                r#"
                UPDATE accounting_decisions 
                SET assigned_account_code = $1
                WHERE document_id = $2
                "#,
            )
            .bind(kontonummer)
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
        }

        if let Some(ref vat) = corrections.vat {
            sqlx::query(
                r#"
                UPDATE extracted_fields 
                SET parsed_value = $1, updated_at = CURRENT_TIMESTAMP
                WHERE document_id = $2 AND field_type = 'vat_amount'
                "#,
            )
            .bind(vat)
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        info!(
            document_id = %document_id,
            "Applied human corrections"
        );

        Ok(())
    }

    /// Process quarantine - document failed validation
    async fn process_quarantine(
        &self,
        document_id: i64,
        reason: &str,
    ) -> AppResult<ReviewDecision> {
        // Update document status to Failed
        sqlx::query(
            r#"
            UPDATE documents
            SET status = 'FAILED', updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            "#,
        )
        .bind(document_id)
        .execute(&self.context.pool)
        .await?;

        // Store review decision
        sqlx::query(
            r#"
            UPDATE review_decisions
            SET decision_type = 'QUARANTINE', 
                review_reason = $2,
                human_review_required = TRUE,
                reviewed_at = CURRENT_TIMESTAMP
            WHERE document_id = $1
            "#,
        )
        .bind(document_id)
        .bind(reason)
        .execute(&self.context.pool)
        .await?;

        self.context
            .record_document_event(
                document_id,
                "REVIEW_QUARANTINED",
                serde_json::json!({ "reason": reason }),
            )
            .await?;

        Ok(ReviewDecision {
            action: ReviewAction::Quarantine,
            confidence_score: 0.0,
            requires_human_review: true,
            review_reason: Some(reason.to_string()),
            human_decision: None,
        })
    }
}

fn push_raw_cause(
    raw_causes: &mut Vec<RawBlockCause>,
    label: impl Into<String>,
    value: impl Into<String>,
) {
    let cause = RawBlockCause {
        label: label.into(),
        value: value.into(),
    };

    if !cause.value.trim().is_empty()
        && !raw_causes
            .iter()
            .any(|existing| existing.label == cause.label && existing.value == cause.value)
    {
        raw_causes.push(cause);
    }
}

fn push_validation_causes(raw_causes: &mut Vec<RawBlockCause>, validation_errors: &Value) {
    if let Some(errors) = validation_errors.as_array() {
        for error in errors {
            let message = error.get("message").and_then(Value::as_str);
            let severity = error.get("severity").and_then(Value::as_str);
            let field = error.get("field").and_then(Value::as_str);

            if let Some(message) = message {
                let label = match (severity, field) {
                    (Some(severity), Some(field)) => {
                        format!("Validation {severity} ({field})")
                    }
                    (Some(severity), None) => format!("Validation {severity}"),
                    (None, Some(field)) => format!("Validation issue ({field})"),
                    (None, None) => "Validation issue".to_string(),
                };
                push_raw_cause(raw_causes, label, message);
            }
        }
    }
}

fn push_event_payload_causes(raw_causes: &mut Vec<RawBlockCause>, payload: &Value) {
    if let Some(reasons) = payload.get("reasons").and_then(Value::as_array) {
        for reason in reasons {
            if let Some(reason_code) = reason.as_str() {
                push_raw_cause(
                    raw_causes,
                    "Recent block reason",
                    humanize_export_reason_code(reason_code),
                );
            }
        }
    }

    if let Some(reason) = payload.get("reason").and_then(Value::as_str) {
        push_raw_cause(raw_causes, "Recent block reason", reason);
    }

    if let Some(message) = payload.get("message").and_then(Value::as_str) {
        push_raw_cause(raw_causes, "Recent block message", message);
    }
}

fn humanize_export_reason_code(reason_code: &str) -> String {
    use crate::agents::export::ExportBlockReason;

    match reason_code {
        "missing_accounting_decision" => ExportBlockReason::MissingAccountingDecision
            .user_message()
            .to_string(),
        "missing_assigned_account" => ExportBlockReason::MissingAssignedAccount
            .user_message()
            .to_string(),
        "placeholder_account" => ExportBlockReason::PlaceholderAccount
            .user_message()
            .to_string(),
        "incomplete_accounting_reasoning" => ExportBlockReason::IncompleteAccountingReasoning
            .user_message()
            .to_string(),
        "invoice_pending_analysis" => ExportBlockReason::InvoicePendingAnalysis
            .user_message()
            .to_string(),
        "missing_transaction_date" => ExportBlockReason::MissingTransactionDate
            .user_message()
            .to_string(),
        "invalid_transaction_date" => ExportBlockReason::InvalidTransactionDate
            .user_message()
            .to_string(),
        "missing_supplier_name" => ExportBlockReason::MissingSupplierName
            .user_message()
            .to_string(),
        "missing_total_amount" => ExportBlockReason::MissingTotalAmount
            .user_message()
            .to_string(),
        "invalid_total_amount" => ExportBlockReason::InvalidTotalAmount
            .user_message()
            .to_string(),
        other => other.replace('_', " "),
    }
}

async fn fetch_recent_block_payloads(pool: &DbPool, document_id: i64) -> AppResult<Vec<Value>> {
    sqlx::query_scalar(
        r#"
        SELECT payload
        FROM document_events
        WHERE document_id = $1
          AND event_type IN ('EXPORT_BLOCKED', 'REVIEW_HUMAN_REQUESTED')
        ORDER BY created_at DESC
        LIMIT 3
        "#,
    )
    .bind(document_id)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)
}

fn blocking_stage_from_reasons(
    reasons: &[crate::agents::export::ExportBlockReason],
) -> BlockingStage {
    use crate::agents::export::ExportBlockReason;

    if reasons.iter().any(|reason| {
        matches!(
            reason,
            ExportBlockReason::MissingAccountingDecision
                | ExportBlockReason::MissingAssignedAccount
                | ExportBlockReason::PlaceholderAccount
                | ExportBlockReason::IncompleteAccountingReasoning
                | ExportBlockReason::InvoicePendingAnalysis
        )
    }) {
        BlockingStage::Accounting
    } else if reasons.iter().any(|reason| {
        matches!(
            reason,
            ExportBlockReason::MissingTransactionDate
                | ExportBlockReason::InvalidTransactionDate
                | ExportBlockReason::MissingSupplierName
                | ExportBlockReason::MissingTotalAmount
                | ExportBlockReason::InvalidTotalAmount
        )
    }) {
        BlockingStage::Validation
    } else {
        BlockingStage::Unknown
    }
}

fn primary_reason_from_context(
    stage: BlockingStage,
    reasons: &[crate::agents::export::ExportBlockReason],
    invoice_status: Option<&str>,
) -> String {
    use crate::agents::export::ExportBlockReason;

    if reasons
        .iter()
        .any(|reason| matches!(reason, ExportBlockReason::InvoicePendingAnalysis))
        || matches!(invoice_status, Some("PENDING_ANALYSIS"))
    {
        return "Invoice analysis is still incomplete.".to_string();
    }

    match stage {
        BlockingStage::Accounting => {
            if reasons
                .iter()
                .any(|reason| matches!(reason, ExportBlockReason::MissingAccountingDecision))
            {
                "No accounting decision has been recorded yet.".to_string()
            } else if reasons
                .iter()
                .any(|reason| matches!(reason, ExportBlockReason::MissingAssignedAccount))
            {
                "The document is missing an assigned account.".to_string()
            } else if reasons
                .iter()
                .any(|reason| matches!(reason, ExportBlockReason::PlaceholderAccount))
            {
                "The assigned accounting account still needs review.".to_string()
            } else if reasons
                .iter()
                .any(|reason| matches!(reason, ExportBlockReason::IncompleteAccountingReasoning))
            {
                "Accounting data is still incomplete.".to_string()
            } else {
                summarize_block_reasons(reasons)
            }
        }
        BlockingStage::Validation => {
            if reasons
                .iter()
                .any(|reason| matches!(reason, ExportBlockReason::MissingTransactionDate))
            {
                "Transaction date is missing.".to_string()
            } else if reasons
                .iter()
                .any(|reason| matches!(reason, ExportBlockReason::InvalidTransactionDate))
            {
                "Transaction date is invalid.".to_string()
            } else if reasons
                .iter()
                .any(|reason| matches!(reason, ExportBlockReason::MissingSupplierName))
            {
                "Supplier name is missing.".to_string()
            } else if reasons
                .iter()
                .any(|reason| matches!(reason, ExportBlockReason::MissingTotalAmount))
            {
                "Total amount is missing.".to_string()
            } else if reasons
                .iter()
                .any(|reason| matches!(reason, ExportBlockReason::InvalidTotalAmount))
            {
                "Total amount is invalid.".to_string()
            } else {
                summarize_block_reasons(reasons)
            }
        }
        _ => summarize_block_reasons(reasons),
    }
}

async fn fetch_approval_block_context(
    pool: &DbPool,
    document_id: i64,
) -> AppResult<ApprovalBlockContext> {
    sqlx::query_as(
        r#"
        SELECT
            d.short_ref,
            (
                SELECT decision_type
                FROM review_decisions rd
                WHERE rd.document_id = d.id
                ORDER BY reviewed_at DESC, created_at DESC
                LIMIT 1
            ) AS decision_type,
            (
                SELECT review_reason
                FROM review_decisions rd
                WHERE rd.document_id = d.id
                ORDER BY reviewed_at DESC, created_at DESC
                LIMIT 1
            ) AS review_reason,
            (
                SELECT status
                FROM invoices i
                WHERE i.document_id = d.id
                ORDER BY updated_at DESC
                LIMIT 1
            ) AS invoice_status,
            (
                SELECT reasoning_text
                FROM accounting_decisions ad
                WHERE ad.document_id = d.id
                ORDER BY created_at DESC
                LIMIT 1
            ) AS accountant_reasoning_text,
            (
                SELECT validation_errors
                FROM validation_results vr
                WHERE vr.document_id = d.id
                ORDER BY checked_at DESC
                LIMIT 1
            ) AS validation_errors
        FROM documents d
        WHERE d.id = $1
        "#,
    )
    .bind(document_id)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)
}

pub async fn build_approval_block_explanation_for_readiness(
    pool: &DbPool,
    document_id: i64,
    readiness: &crate::agents::export::ExportReadiness,
) -> AppResult<ApprovalBlockExplanation> {
    let context = fetch_approval_block_context(pool, document_id).await?;
    let recent_payloads = fetch_recent_block_payloads(pool, document_id).await?;
    let blocking_stage = blocking_stage_from_reasons(&readiness.reasons);
    let headline_reason = primary_reason_from_context(
        blocking_stage,
        &readiness.reasons,
        context.invoice_status.as_deref(),
    );

    let mut raw_causes = Vec::new();

    if let Some(invoice_status) = context.invoice_status.as_deref() {
        push_raw_cause(&mut raw_causes, "Invoice status", invoice_status);
    }

    if let Some(review_state) = context.decision_type.as_deref() {
        push_raw_cause(&mut raw_causes, "Review state", review_state);
    }

    if let Some(review_reason) = context.review_reason.as_deref() {
        push_raw_cause(&mut raw_causes, "Review reason", review_reason);
    }

    for reason in &readiness.reasons {
        push_raw_cause(
            &mut raw_causes,
            "Export readiness",
            format!("{} ({})", reason.user_message(), reason.as_code()),
        );
    }

    if let Some(reasoning_text) = context.accountant_reasoning_text.as_deref() {
        push_raw_cause(&mut raw_causes, "Accounting reasoning", reasoning_text);
    }

    if let Some(validation_errors) = context.validation_errors.as_ref() {
        push_validation_causes(&mut raw_causes, validation_errors);
    }

    for payload in recent_payloads {
        push_event_payload_causes(&mut raw_causes, &payload);
    }

    Ok(ApprovalBlockExplanation {
        short_ref: context.short_ref.clone(),
        blocking_stage,
        headline_reason,
        raw_causes,
    })
}

pub async fn build_failed_document_explanation(
    pool: &DbPool,
    document_id: i64,
    headline_reason: Option<&str>,
) -> AppResult<ApprovalBlockExplanation> {
    let context = fetch_approval_block_context(pool, document_id).await?;
    let recent_payloads = fetch_recent_block_payloads(pool, document_id).await?;
    let mut raw_causes = Vec::new();

    if let Some(invoice_status) = context.invoice_status.as_deref() {
        push_raw_cause(&mut raw_causes, "Invoice status", invoice_status);
    }

    if let Some(review_state) = context.decision_type.as_deref() {
        push_raw_cause(&mut raw_causes, "Review state", review_state);
    }

    if let Some(review_reason) = context.review_reason.as_deref() {
        push_raw_cause(&mut raw_causes, "Review reason", review_reason);
    }

    if let Some(reasoning_text) = context.accountant_reasoning_text.as_deref() {
        push_raw_cause(&mut raw_causes, "Accounting reasoning", reasoning_text);
    }

    if let Some(validation_errors) = context.validation_errors.as_ref() {
        push_validation_causes(&mut raw_causes, validation_errors);
    }

    for payload in recent_payloads {
        push_event_payload_causes(&mut raw_causes, &payload);
    }

    let blocking_stage = if raw_causes
        .iter()
        .any(|cause| cause.label.starts_with("Validation"))
    {
        BlockingStage::Validation
    } else if context.invoice_status.as_deref() == Some("PENDING_ANALYSIS")
        || context.accountant_reasoning_text.is_some()
    {
        BlockingStage::Accounting
    } else {
        BlockingStage::Unknown
    };

    let headline_reason = headline_reason
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| context.review_reason.clone())
        .unwrap_or_else(|| "Document processing failed.".to_string());

    Ok(ApprovalBlockExplanation {
        short_ref: context.short_ref,
        blocking_stage,
        headline_reason,
        raw_causes,
    })
}

pub async fn describe_approval_block(
    pool: &DbPool,
    document_id: i64,
) -> AppResult<Option<ApprovalBlockExplanation>> {
    let readiness = ExportAgent::evaluate_document_readiness(pool, document_id).await?;
    if readiness.ready {
        Ok(None)
    } else {
        build_approval_block_explanation_for_readiness(pool, document_id, &readiness)
            .await
            .map(Some)
    }
}

#[async_trait::async_trait]
impl Agent for ReviewAgent {
    type Input = ReviewInput;
    type Output = ReviewOutput;
    type Error = AppError;

    fn name(&self) -> &'static str {
        "ReviewAgent"
    }

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        info!(
            document_id = %input.document_id,
            force_human_review = input.force_human_review,
            "Starting review decision"
        );

        // Get confidence score from review_decisions
        let _row = sqlx::query(
            "SELECT confidence_score FROM review_decisions WHERE document_id = $1 ORDER BY reviewed_at DESC LIMIT 1"
        )
        .bind(input.document_id)
        .fetch_optional(&self.context.pool)
        .await?;

        let confidence_score_bd: Option<f64> = sqlx::query_scalar(
            "SELECT confidence_score FROM review_decisions WHERE document_id = $1 ORDER BY reviewed_at DESC LIMIT 1"
        )
        .bind(input.document_id)
        .fetch_optional(&self.context.pool)
        .await?;

        let confidence_score: f64 = confidence_score_bd.unwrap_or(0.0);
        let action = ReviewThresholds::determine_action(confidence_score);

        // Override if force human review
        let action = if input.force_human_review {
            ReviewAction::HumanReview
        } else {
            action
        };

        let decision = match action {
            ReviewAction::AutoApprove => {
                info!(
                    document_id = %input.document_id,
                    confidence = confidence_score,
                    "Auto-approving document"
                );

                let readiness =
                    ExportAgent::evaluate_document_readiness(&self.context.pool, input.document_id)
                        .await?;
                if !readiness.ready {
                    let review_reason = format!(
                        "Export readiness blocked: {}",
                        summarize_block_reasons(&readiness.reasons)
                    );

                    info!(
                        document_id = %input.document_id,
                        reasons = %summarize_block_reasons(&readiness.reasons),
                        "Escalating high-confidence document to human review because export readiness failed"
                    );

                    sqlx::query(
                        r#"
                        UPDATE documents
                        SET status = 'PENDING_HUMAN_REVIEW', updated_at = CURRENT_TIMESTAMP
                        WHERE id = $1
                        "#,
                    )
                    .bind(input.document_id)
                    .execute(&self.context.pool)
                    .await?;

                    sqlx::query(
                        r#"
                        UPDATE review_decisions
                        SET decision_type = 'PENDING_HUMAN_REVIEW',
                            human_review_required = TRUE,
                            review_reason = $2
                        WHERE document_id = $1
                        "#,
                    )
                    .bind(input.document_id)
                    .bind(&review_reason)
                    .execute(&self.context.pool)
                    .await?;

                    self.context
                        .record_document_event(
                            input.document_id,
                            "REVIEW_HUMAN_REQUESTED",
                            serde_json::json!({
                                "confidence": confidence_score,
                                "source": "export_readiness_gate",
                                "reasons": readiness.reasons.iter().map(|reason| reason.as_code()).collect::<Vec<_>>(),
                            }),
                        )
                    .await?;

                    return Ok(ReviewOutput {
                        document_id: input.document_id,
                        decision: ReviewDecision {
                            action: ReviewAction::HumanReview,
                            confidence_score,
                            requires_human_review: true,
                            review_reason: Some(review_reason),
                            human_decision: None,
                        },
                        action_taken: ReviewAction::HumanReview,
                    });
                }

                self.context
                    .update_document_status(input.document_id, DocumentStatus::ExportReady)
                    .await?;
                sqlx::query(
                    r#"
                    UPDATE documents
                    SET review_completed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
                    WHERE id = $1
                    "#,
                )
                .bind(input.document_id)
                .execute(&self.context.pool)
                .await?;

                // Update review decision
                sqlx::query(
                    r#"
                    UPDATE review_decisions
                    SET decision_type = 'AUTO_APPROVED', human_review_required = FALSE
                    WHERE document_id = $1
                    "#,
                )
                .bind(input.document_id)
                .execute(&self.context.pool)
                .await?;
                self.context
                    .record_document_event(
                        input.document_id,
                        "REVIEW_AUTO_APPROVED",
                        serde_json::json!({ "confidence": confidence_score }),
                    )
                    .await?;

                ReviewDecision {
                    action: ReviewAction::AutoApprove,
                    confidence_score,
                    requires_human_review: false,
                    review_reason: None,
                    human_decision: None,
                }
            }

            ReviewAction::HumanReview => {
                info!(
                    document_id = %input.document_id,
                    confidence = confidence_score,
                    "Requesting human review"
                );

                // Update review decision to pending human review
                sqlx::query(
                    r#"
                    UPDATE documents
                    SET status = 'PENDING_HUMAN_REVIEW', updated_at = CURRENT_TIMESTAMP
                    WHERE id = $1
                    "#,
                )
                .bind(input.document_id)
                .execute(&self.context.pool)
                .await?;

                sqlx::query(
                    r#"
                    UPDATE review_decisions
                    SET decision_type = 'PENDING_HUMAN_REVIEW', human_review_required = TRUE
                    WHERE document_id = $1
                    "#,
                )
                .bind(input.document_id)
                .execute(&self.context.pool)
                .await?;

                self.context
                    .record_document_event(
                        input.document_id,
                        "REVIEW_HUMAN_REQUESTED",
                        serde_json::json!({ "confidence": confidence_score }),
                    )
                    .await?;

                ReviewDecision {
                    action: ReviewAction::HumanReview,
                    confidence_score,
                    requires_human_review: true,
                    review_reason: Some(
                        "Low confidence score, requires human verification".to_string(),
                    ),
                    human_decision: None,
                }
            }

            ReviewAction::Quarantine => {
                info!(
                    document_id = %input.document_id,
                    confidence = confidence_score,
                    "Quarantining document"
                );

                self.process_quarantine(
                    input.document_id,
                    "Validation failed - critical errors detected",
                )
                .await?
            }
        };

        Ok(ReviewOutput {
            document_id: input.document_id,
            decision: decision.clone(),
            action_taken: decision.action,
        })
    }
}

/// Process a human review callback action against document state.
pub async fn process_human_review_callback(
    pool: &DbPool,
    queue_producer: &QueueProducer,
    callback_data: &str,
    reviewed_by: &str,
) -> AppResult<HumanReviewCallbackResult> {
    let parts: Vec<&str> = callback_data.split(':').collect();

    if parts.len() < 2 {
        return Ok(HumanReviewCallbackResult {
            callback_text: "Invalid action".to_string(),
            chat_message: "Invalid callback data".to_string(),
            keyboard: None,
        });
    }

    let action = parts[0];
    let (field, document_id) = match action {
        "edit" if parts.len() >= 3 => (
            Some(parts[1].parse::<ReviewField>()?),
            parts[2]
                .parse::<i64>()
                .map_err(|e| AppError::Serialization(format!("Invalid document id: {}", e)))?,
        ),
        _ => (
            None,
            parts[1]
                .parse::<i64>()
                .map_err(|e| AppError::Serialization(format!("Invalid document id: {}", e)))?,
        ),
    };

    match action {
        "approve" => {
            let readiness = ExportAgent::evaluate_document_readiness(pool, document_id).await?;
            if !readiness.ready {
                let explanation =
                    build_approval_block_explanation_for_readiness(pool, document_id, &readiness)
                        .await?;
                sqlx::query(
                    r#"
                    UPDATE review_decisions
                    SET decision_type = 'PENDING_HUMAN_REVIEW',
                        human_review_required = TRUE,
                        review_reason = $2
                    WHERE document_id = $1
                    "#,
                )
                .bind(document_id)
                .bind(format!(
                    "Export readiness blocked: {}",
                    summarize_block_reasons(&readiness.reasons)
                ))
                .execute(pool)
                .await?;
                db_helpers::record_document_event(
                    pool,
                    document_id,
                    "REVIEW_HUMAN_REQUESTED",
                    serde_json::json!({
                        "source": "human_approval_gate",
                        "reasons": readiness.reasons.iter().map(|reason| reason.as_code()).collect::<Vec<_>>(),
                    }),
                )
                .await?;

                return Ok(HumanReviewCallbackResult {
                    callback_text: "Approval blocked".to_string(),
                    chat_message: explanation.as_approval_message(),
                    keyboard: Some(build_review_keyboard(document_id)),
                });
            }

            // Approve the document
            sqlx::query(
                r#"
                UPDATE documents
                SET status = 'EXPORT_READY', review_completed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
                WHERE id = $1
                "#,
            )
            .bind(document_id)
            .execute(pool)
            .await?;

            sqlx::query(
                r#"
                UPDATE review_decisions
                SET decision_type = 'HUMAN_APPROVED',
                    human_review_required = FALSE,
                    reviewed_at = CURRENT_TIMESTAMP,
                    reviewed_by = $2
                WHERE document_id = $1
                "#,
            )
            .bind(document_id)
            .bind(reviewed_by)
            .execute(pool)
            .await?;
            db_helpers::record_document_event(
                pool,
                document_id,
                "HUMAN_APPROVED",
                serde_json::json!({ "reviewed_by": reviewed_by }),
            )
            .await?;

            Ok(HumanReviewCallbackResult {
                callback_text: "Document approved".to_string(),
                chat_message: "Document approved and added to the export pool.".to_string(),
                keyboard: None,
            })
        }

        "reject" => {
            // Reject the document
            sqlx::query(
                r#"
                UPDATE documents
                SET status = 'FAILED', updated_at = CURRENT_TIMESTAMP
                WHERE id = $1
                "#,
            )
            .bind(document_id)
            .execute(pool)
            .await?;

            sqlx::query(
                r#"
                UPDATE review_decisions
                SET decision_type = 'HUMAN_REJECTED',
                    human_review_required = FALSE,
                    review_reason = 'Rejected by human reviewer',
                    reviewed_at = CURRENT_TIMESTAMP,
                    reviewed_by = $2
                WHERE document_id = $1
                "#,
            )
            .bind(document_id)
            .bind(reviewed_by)
            .execute(pool)
            .await?;
            db_helpers::record_document_event(
                pool,
                document_id,
                "HUMAN_REJECTED",
                serde_json::json!({ "reviewed_by": reviewed_by }),
            )
            .await?;

            Ok(HumanReviewCallbackResult {
                callback_text: "Document rejected".to_string(),
                chat_message: "Document rejected and quarantined.".to_string(),
                keyboard: None,
            })
        }

        "edit" => {
            let field = field.expect("field set for edit");
            Ok(HumanReviewCallbackResult {
                callback_text: "Edit recorded".to_string(),
                chat_message: format!(
                    "Send the corrected {} as your next message.",
                    field.prompt_label()
                ),
                keyboard: None,
            })
        }

        "rerun_accounting" => {
            let mut tx = pool.begin().await?;
            sqlx::query(
                r#"
                UPDATE documents
                SET status = 'PROCESSING_ACCOUNTANT', updated_at = CURRENT_TIMESTAMP
                WHERE id = $1
                "#,
            )
            .bind(document_id)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                r#"
                UPDATE review_decisions
                SET decision_type = 'PENDING_HUMAN_REVIEW',
                    human_review_required = TRUE,
                    review_reason = 'Accounting rerun requested by human reviewer',
                    reviewed_at = CURRENT_TIMESTAMP,
                    reviewed_by = $2
                WHERE document_id = $1
                "#,
            )
            .bind(document_id)
            .bind(reviewed_by)
            .execute(&mut *tx)
            .await?;

            db_helpers::record_document_event_in_tx(
                &mut tx,
                document_id,
                "ACCOUNTING_RERUN_REQUESTED",
                serde_json::json!({ "reviewed_by": reviewed_by }),
            )
            .await?;
            tx.commit().await?;

            queue_producer
                .enqueue(&Job::new_with_mode(
                    JobType::Accountant,
                    document_id,
                    0,
                    FieldLoadMode::EffectiveWithCorrections,
                ))
                .await
                .map_err(|err| AppError::Queue(err.to_string()))?;

            Ok(HumanReviewCallbackResult {
                callback_text: "Accounting re-queued".to_string(),
                chat_message:
                    "Accounting has been re-queued. The document will go through accounting, validation, and review again."
                        .to_string(),
                keyboard: None,
            })
        }

        _ => Ok(HumanReviewCallbackResult {
            callback_text: "Unknown action".to_string(),
            chat_message: "Unknown action".to_string(),
            keyboard: None,
        }),
    }
}

pub async fn process_human_review_text_input(
    pool: &DbPool,
    queue_producer: &QueueProducer,
    document_id: i64,
    field: ReviewField,
    reviewed_by: &str,
    text: &str,
) -> AppResult<String> {
    let mut tx = pool.begin().await?;
    apply_correction_value(&mut tx, document_id, field, text).await?;
    upsert_review_correction(&mut tx, document_id, field, text, reviewed_by).await?;

    sqlx::query(
        r#"
        UPDATE documents
        SET status = 'PROCESSING_VALIDATOR', updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        "#,
    )
    .bind(document_id)
    .execute(&mut *tx)
    .await?;
    db_helpers::record_document_event_in_tx(
        &mut tx,
        document_id,
        "HUMAN_CORRECTION_APPLIED",
        serde_json::json!({
            "field": field.as_str(),
            "value": text,
            "reviewed_by": reviewed_by,
        }),
    )
    .await?;
    tx.commit().await?;

    queue_producer
        .enqueue(&Job::new_with_mode(
            JobType::Validator,
            document_id,
            0,
            FieldLoadMode::EffectiveWithCorrections,
        ))
        .await
        .map_err(|err| AppError::Queue(err.to_string()))?;

    Ok(format!(
        "Updated {}. The document has been queued for re-validation.",
        field.prompt_label()
    ))
}

async fn apply_correction_value(
    tx: &mut SqliteConnection,
    document_id: i64,
    field: ReviewField,
    value: &str,
) -> AppResult<()> {
    match field {
        ReviewField::Supplier => {
            let result = sqlx::query(
                r#"
                UPDATE extracted_fields
                SET parsed_value = $1, raw_value = $1, source = 'USER_CORRECTION', is_user_edited = TRUE, updated_at = CURRENT_TIMESTAMP
                WHERE document_id = $2 AND field_type = 'supplier_name'
                "#,
            )
            .bind(value)
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                sqlx::query(
                    r#"
                    INSERT INTO extracted_fields
                    (document_id, field_type, raw_value, parsed_value, source, is_user_edited, created_at, updated_at)
                    VALUES ($1, 'supplier_name', $2, $2, 'USER_CORRECTION', TRUE, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                    "#,
                )
                .bind(document_id)
                .bind(value)
                .execute(&mut *tx)
                .await?;
            }
        }
        ReviewField::Amount => {
            let result = sqlx::query(
                r#"
                UPDATE extracted_fields
                SET parsed_value = $1, raw_value = $1, source = 'USER_CORRECTION', is_user_edited = TRUE, updated_at = CURRENT_TIMESTAMP
                WHERE document_id = $2 AND field_type = 'total_amount'
                "#,
            )
            .bind(value)
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                sqlx::query(
                    r#"
                    INSERT INTO extracted_fields
                    (document_id, field_type, raw_value, parsed_value, source, is_user_edited, created_at, updated_at)
                    VALUES ($1, 'total_amount', $2, $2, 'USER_CORRECTION', TRUE, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                    "#,
                )
                .bind(document_id)
                .bind(value)
                .execute(&mut *tx)
                .await?;
            }
        }
        ReviewField::Date => {
            let result = sqlx::query(
                r#"
                UPDATE extracted_fields
                SET parsed_value = $1, raw_value = $1, source = 'USER_CORRECTION', is_user_edited = TRUE, updated_at = CURRENT_TIMESTAMP
                WHERE document_id = $2 AND field_type = 'transaction_date'
                "#,
            )
            .bind(value)
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                sqlx::query(
                    r#"
                    INSERT INTO extracted_fields
                    (document_id, field_type, raw_value, parsed_value, source, is_user_edited, created_at, updated_at)
                    VALUES ($1, 'transaction_date', $2, $2, 'USER_CORRECTION', TRUE, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                    "#,
                )
                .bind(document_id)
                .bind(value)
                .execute(&mut *tx)
                .await?;
            }
        }
        ReviewField::Kontonummer => {
            let result = sqlx::query(
                r#"
                UPDATE accounting_decisions
                SET assigned_account_code = $1
                WHERE document_id = $2
                "#,
            )
            .bind(value)
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                sqlx::query(
                    r#"
                    INSERT INTO accounting_decisions
                    (document_id, assigned_account_code, created_at)
                    VALUES ($1, $2, CURRENT_TIMESTAMP)
                    "#,
                )
                .bind(document_id)
                .bind(value)
                .execute(&mut *tx)
                .await?;
            }
        }
        ReviewField::Vat => {
            let result = sqlx::query(
                r#"
                UPDATE extracted_fields
                SET parsed_value = $1, raw_value = $1, source = 'USER_CORRECTION', is_user_edited = TRUE, updated_at = CURRENT_TIMESTAMP
                WHERE document_id = $2 AND field_type = 'vat_amount'
                "#,
            )
            .bind(value)
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                sqlx::query(
                    r#"
                    INSERT INTO extracted_fields
                    (document_id, field_type, raw_value, parsed_value, source, is_user_edited, created_at, updated_at)
                    VALUES ($1, 'vat_amount', $2, $2, 'USER_CORRECTION', TRUE, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                    "#,
                )
                .bind(document_id)
                .bind(value)
                .execute(&mut *tx)
                .await?;
            }
        }
    }

    Ok(())
}

async fn upsert_review_correction(
    tx: &mut SqliteConnection,
    document_id: i64,
    field: ReviewField,
    value: &str,
    reviewed_by: &str,
) -> AppResult<()> {
    let existing: Option<serde_json::Value> = sqlx::query_scalar::<_, Option<serde_json::Value>>(
        "SELECT corrections FROM review_decisions WHERE document_id = $1",
    )
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let mut corrections = existing.unwrap_or_else(|| serde_json::json!({}));
    corrections[field.as_str()] = serde_json::Value::String(value.to_string());

    sqlx::query(
        r#"
        UPDATE review_decisions
        SET corrections = $2, reviewed_by = $3, reviewed_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .bind(corrections)
    .bind(reviewed_by)
    .execute(&mut *tx)
    .await?;

    Ok(())
}

fn build_review_keyboard(document_id: i64) -> HumanReviewKeyboard {
    let callback = |label: &str, callback_data: String| HumanReviewButton {
        label: label.to_string(),
        callback_data,
    };

    HumanReviewKeyboard {
        rows: vec![
            vec![
                callback("✏ Edit Supplier", format!("edit:supplier:{}", document_id)),
                callback("💰 Edit Amount", format!("edit:amount:{}", document_id)),
            ],
            vec![
                callback("📅 Edit Date", format!("edit:date:{}", document_id)),
                callback(
                    "📋 Edit Account",
                    format!("edit:kontonummer:{}", document_id),
                ),
            ],
            vec![callback("🧾 Edit VAT", format!("edit:vat:{}", document_id))],
            vec![callback(
                "🔄 Re-run Accounting",
                format!("rerun_accounting:{}", document_id),
            )],
            vec![
                callback("✅ Approve", format!("approve:{}", document_id)),
                callback("❌ Reject", format!("reject:{}", document_id)),
            ],
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::export::ExportBlockReason;

    #[test]
    fn test_review_keyboard_includes_rerun_accounting_action() {
        let document_id = 7_i64;
        let keyboard = build_review_keyboard(document_id);
        let debug = format!("{keyboard:?}");

        assert!(debug.contains("Re-run Accounting"));
        assert!(debug.contains("rerun_accounting:7"));
    }

    #[test]
    fn review_agent_has_no_channel_delivery_dependency() {
        let source = include_str!("review.rs");
        let forbidden_a = ["Telegram", "Notifier"].concat();
        let forbidden_b = ["tel", "oxide"].concat();
        let forbidden_c = ["send", "_", "review", "_", "message"].concat();
        let forbidden_d = ["document", "_", "artifacts"].concat();
        let forbidden_e = ["document", "_", "interactions"].concat();
        let forbidden_f = ["human", "_", "review", "_", "sessions"].concat();
        let forbidden_g = ["interaction", "_", "key"].concat();

        assert!(!source.contains(&forbidden_a));
        assert!(!source.contains(&forbidden_b));
        assert!(!source.contains(&forbidden_c));
        assert!(!source.contains(&forbidden_d));
        assert!(!source.contains(&forbidden_e));
        assert!(!source.contains(&forbidden_f));
        assert!(!source.contains(&forbidden_g));
    }

    #[test]
    fn test_primary_reason_prefers_invoice_pending_analysis() {
        let reasons = vec![ExportBlockReason::InvoicePendingAnalysis];
        let reason = primary_reason_from_context(
            BlockingStage::Accounting,
            &reasons,
            Some("PENDING_ANALYSIS"),
        );

        assert_eq!(reason, "Invoice analysis is still incomplete.");
    }

    #[test]
    fn test_approval_block_message_includes_all_raw_causes() {
        let explanation = ApprovalBlockExplanation {
            short_ref: "D000004".to_string(),
            blocking_stage: BlockingStage::Accounting,
            headline_reason: "Invoice analysis is still incomplete.".to_string(),
            raw_causes: vec![
                RawBlockCause {
                    label: "Invoice status".to_string(),
                    value: "PENDING_ANALYSIS".to_string(),
                },
                RawBlockCause {
                    label: "Accounting reasoning".to_string(),
                    value: "warnings: Supplier organization number is unknown".to_string(),
                },
            ],
        };

        let message = explanation.as_approval_message();
        assert!(message.contains("D000004 cannot be approved yet."));
        assert!(message.contains("Blocking stage: Accounting"));
        assert!(message.contains("Raw causes:"));
        assert!(message.contains("- Invoice status: PENDING_ANALYSIS"));
        assert!(
            message.contains(
                "- Accounting reasoning: warnings: Supplier organization number is unknown"
            )
        );
    }

    #[test]
    fn test_push_raw_cause_deduplicates_same_label_and_value() {
        let mut raw_causes = Vec::new();
        push_raw_cause(&mut raw_causes, "Invoice status", "PENDING_ANALYSIS");
        push_raw_cause(&mut raw_causes, "Invoice status", "PENDING_ANALYSIS");

        assert_eq!(raw_causes.len(), 1);
    }

    #[test]
    fn test_push_validation_causes_uses_messages_instead_of_raw_json() {
        let mut raw_causes = Vec::new();
        let validation_errors = serde_json::json!([
            {
                "field": "org_nr",
                "message": "Supplier organization number is missing",
                "severity": "Critical"
            },
            {
                "field": "vat_rate",
                "message": "VAT rate is missing",
                "severity": "Error"
            }
        ]);

        push_validation_causes(&mut raw_causes, &validation_errors);

        assert_eq!(raw_causes.len(), 2);
        assert_eq!(raw_causes[0].label, "Validation Critical (org_nr)");
        assert_eq!(
            raw_causes[0].value,
            "Supplier organization number is missing"
        );
        assert_eq!(raw_causes[1].label, "Validation Error (vat_rate)");
        assert_eq!(raw_causes[1].value, "VAT rate is missing");
    }

    #[test]
    fn test_push_event_payload_causes_uses_reason_messages_not_raw_json() {
        let mut raw_causes = Vec::new();
        let payload = serde_json::json!({
            "reasons": ["invoice_pending_analysis"],
            "source": "human_approval_gate"
        });

        push_event_payload_causes(&mut raw_causes, &payload);

        assert_eq!(raw_causes.len(), 1);
        assert_eq!(raw_causes[0].label, "Recent block reason");
        assert_eq!(raw_causes[0].value, "invoice is still pending analysis");
    }

    #[test]
    fn test_append_detail_lines_keeps_pending_review_shape() {
        let explanation = ApprovalBlockExplanation {
            short_ref: "D000007".to_string(),
            blocking_stage: BlockingStage::Accounting,
            headline_reason: "Invoice analysis is still incomplete.".to_string(),
            raw_causes: vec![
                RawBlockCause {
                    label: "Invoice status".to_string(),
                    value: "PENDING_ANALYSIS".to_string(),
                },
                RawBlockCause {
                    label: "Review reason".to_string(),
                    value: "Export readiness blocked: invoice is still pending analysis"
                        .to_string(),
                },
            ],
        };

        let mut lines = Vec::new();
        explanation.append_detail_lines(&mut lines);

        assert_eq!(lines[0], "Blocking stage: Accounting");
        assert_eq!(lines[1], "Reason: Invoice analysis is still incomplete.");
        assert!(lines.contains(&"Raw causes:".to_string()));
        assert!(lines.contains(&"- Invoice status: PENDING_ANALYSIS".to_string()));
    }
}
