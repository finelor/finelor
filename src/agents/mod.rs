use std::collections::HashMap;
use std::fmt::Display;

use crate::db::DbPool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::web::events::{AppEvent, AppEventBus};

pub mod accountant;
pub mod export;
pub mod intake;
pub mod review;
pub mod validation;
pub mod validator;
pub mod vision;

pub use accountant::{AccountAssignment, AccountantAgent, AccountantInput, AccountantOutput};
pub use export::{ExportAgent, ExportInput, ExportManifest, ExportOutput};
pub use intake::{IntakeAgent, IntakeInput, IntakeOutput};
pub use review::{
    FieldCorrections, HumanReviewCallbackResult, ReviewAgent, ReviewDecision, ReviewInput,
    ReviewOutput, build_failed_document_explanation, describe_approval_block,
    process_human_review_callback, process_human_review_text_input,
};
pub use validation::{ValidationResult, ValidationStatus, ValidatorInput, ValidatorOutput};
pub use validator::ValidatorAgent;
pub use vision::{ExtractedFields, VisionAgent, VisionInput, VisionOutput};

/// Trait defining the interface for all agents in the Finelor system.
#[async_trait::async_trait]
pub trait Agent {
    type Input;
    type Output;
    type Error: Display;

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error>;
    fn name(&self) -> &'static str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FieldLoadMode {
    #[default]
    OriginalOnly,
    EffectiveWithCorrections,
}

/// Document status in the processing pipeline
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentStatus {
    Received,
    ProcessingVision,
    VisionComplete,
    ProcessingAccountant,
    AccountantReviewed,
    ProcessingValidator,
    Validated,
    PendingHumanReview,
    ExportReady,
    GeneratingSie4,
    ReviewCompleted,
    Exported,
    Archived,
    Failed,
}

impl DocumentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocumentStatus::Received => "RECEIVED",
            DocumentStatus::ProcessingVision => "PROCESSING_VISION",
            DocumentStatus::VisionComplete => "VISION_COMPLETE",
            DocumentStatus::ProcessingAccountant => "PROCESSING_ACCOUNTANT",
            DocumentStatus::AccountantReviewed => "ACCOUNTANT_REVIEWED",
            DocumentStatus::ProcessingValidator => "PROCESSING_VALIDATOR",
            DocumentStatus::Validated => "VALIDATED",
            DocumentStatus::PendingHumanReview => "PENDING_HUMAN_REVIEW",
            DocumentStatus::ExportReady => "EXPORT_READY",
            DocumentStatus::GeneratingSie4 => "GENERATING_SIE4",
            DocumentStatus::ReviewCompleted => "REVIEW_COMPLETED",
            DocumentStatus::Exported => "EXPORTED",
            DocumentStatus::Archived => "ARCHIVED",
            DocumentStatus::Failed => "FAILED",
        }
    }
}

impl From<&str> for DocumentStatus {
    fn from(s: &str) -> Self {
        match s {
            "RECEIVED" => DocumentStatus::Received,
            "PROCESSING_VISION" => DocumentStatus::ProcessingVision,
            "VISION_COMPLETE" => DocumentStatus::VisionComplete,
            "PROCESSING_ACCOUNTANT" => DocumentStatus::ProcessingAccountant,
            "ACCOUNTANT_REVIEWED" => DocumentStatus::AccountantReviewed,
            "PROCESSING_VALIDATOR" => DocumentStatus::ProcessingValidator,
            "VALIDATED" => DocumentStatus::Validated,
            "PENDING_HUMAN_REVIEW" => DocumentStatus::PendingHumanReview,
            "EXPORT_READY" => DocumentStatus::ExportReady,
            "GENERATING_SIE4" => DocumentStatus::GeneratingSie4,
            "REVIEW_COMPLETED" => DocumentStatus::ReviewCompleted,
            "EXPORTED" => DocumentStatus::Exported,
            "ARCHIVED" => DocumentStatus::Archived,
            "FAILED" => DocumentStatus::Failed,
            _ => DocumentStatus::Received,
        }
    }
}

/// Context passed to agents containing shared resources
#[derive(Clone)]
pub struct AgentContext {
    pub pool: DbPool,
    pub config: crate::config::AppConfig,
    pub events: AppEventBus,
}

impl AgentContext {
    pub fn new(pool: DbPool, config: crate::config::AppConfig, events: AppEventBus) -> Self {
        Self {
            pool,
            config,
            events,
        }
    }

    pub async fn update_document_status(
        &self,
        document_id: i64,
        status: DocumentStatus,
    ) -> crate::error::AppResult<()> {
        sqlx::query(
            r#"
            UPDATE documents
            SET status = $1, updated_at = CURRENT_TIMESTAMP
            WHERE id = $2
            "#,
        )
        .bind(status.as_str())
        .bind(document_id)
        .execute(&self.pool)
        .await?;

        tracing::info!(
            document_id = %document_id,
            status = %status.as_str(),
            "Document status updated"
        );

        self.record_document_event(
            document_id,
            "STATUS_CHANGED",
            json!({ "status": status.as_str() }),
        )
        .await?;

        Ok(())
    }

    pub async fn record_document_event(
        &self,
        document_id: i64,
        event_type: &str,
        payload: serde_json::Value,
    ) -> crate::error::AppResult<i64> {
        let event_id =
            db_helpers::record_document_event(&self.pool, document_id, event_type, payload).await?;
        publish_document_event(&self.pool, &self.events, event_id, document_id, event_type).await?;
        Ok(event_id)
    }
}

pub async fn publish_document_event(
    _pool: &DbPool,
    events: &AppEventBus,
    event_id: i64,
    document_id: i64,
    event_type: &str,
) -> crate::error::AppResult<()> {
    events.publish(AppEvent::new(
        crate::workspace::active_workspace_id(),
        "document.event_recorded",
        json!({
            "event_id": event_id,
            "document_id": document_id,
            "document_event_type": event_type,
        }),
    ));

    Ok(())
}

/// Helper functions for database operations
pub mod db_helpers {
    use super::*;
    use crate::db::DbPool;
    use sqlx::{Sqlite, Transaction};

    #[derive(sqlx::FromRow)]
    struct ExtractedFieldValue {
        field_type: String,
        parsed_value: Option<String>,
        source: String,
    }

    fn resolve_extracted_field_rows(
        rows: Vec<ExtractedFieldValue>,
        mode: FieldLoadMode,
    ) -> HashMap<String, Option<String>> {
        let mut fields = HashMap::new();

        for row in rows {
            if mode == FieldLoadMode::OriginalOnly && row.source == "USER_CORRECTION" {
                continue;
            }

            fields
                .entry(row.field_type)
                .or_insert_with(|| row.parsed_value.clone());
        }

        fields
    }

    pub async fn load_document_fields(
        pool: &DbPool,
        document_id: i64,
        mode: FieldLoadMode,
    ) -> crate::error::AppResult<HashMap<String, Option<String>>> {
        let include_user_corrections = mode == FieldLoadMode::EffectiveWithCorrections;
        let rows: Vec<ExtractedFieldValue> = sqlx::query_as(
            r#"
            SELECT field_type, parsed_value, source
            FROM (
                SELECT
                    field_type,
                    parsed_value,
                    source,
                    ROW_NUMBER() OVER (
                        PARTITION BY field_type
                        ORDER BY
                            CASE
                                WHEN $2 AND source = 'USER_CORRECTION' THEN 0
                                ELSE 1
                            END,
                            updated_at DESC,
                            created_at DESC,
                            source ASC,
                            COALESCE(parsed_value, '') DESC
                    ) AS rn
                FROM extracted_fields
                WHERE document_id = $1
                  AND ($2 OR source <> 'USER_CORRECTION')
            )
            WHERE rn = 1
            "#,
        )
        .bind(document_id)
        .bind(include_user_corrections)
        .fetch_all(pool)
        .await?;

        Ok(resolve_extracted_field_rows(rows, mode))
    }

    pub async fn update_document_status(
        pool: &DbPool,
        document_id: i64,
        status: DocumentStatus,
    ) -> crate::error::AppResult<()> {
        sqlx::query(
            r#"
            UPDATE documents 
            SET status = $1, updated_at = CURRENT_TIMESTAMP
            WHERE id = $2
            "#,
        )
        .bind(status.as_str())
        .bind(document_id)
        .execute(pool)
        .await?;

        tracing::info!(
            document_id = %document_id,
            status = %status.as_str(),
            "Document status updated"
        );

        record_document_event(
            pool,
            document_id,
            "STATUS_CHANGED",
            serde_json::json!({ "status": status.as_str() }),
        )
        .await?;

        Ok(())
    }

    pub async fn record_document_event(
        pool: &DbPool,
        document_id: i64,
        event_type: &str,
        payload: serde_json::Value,
    ) -> crate::error::AppResult<i64> {
        let event_id = sqlx::query_scalar(
            r#"
            INSERT INTO document_events (document_id, event_type, payload)
            VALUES ($1, $2, $3)
            RETURNING id
            "#,
        )
        .bind(document_id)
        .bind(event_type)
        .bind(payload)
        .fetch_one(pool)
        .await?;

        Ok(event_id)
    }

    pub async fn record_document_event_in_tx(
        tx: &mut Transaction<'_, Sqlite>,
        document_id: i64,
        event_type: &str,
        payload: serde_json::Value,
    ) -> crate::error::AppResult<i64> {
        let event_id = sqlx::query_scalar(
            r#"
            INSERT INTO document_events (document_id, event_type, payload)
            VALUES ($1, $2, $3)
            RETURNING id
            "#,
        )
        .bind(document_id)
        .bind(event_type)
        .bind(payload)
        .fetch_one(&mut **tx)
        .await?;

        Ok(event_id)
    }

    pub async fn update_vision_timestamps(
        pool: &DbPool,
        document_id: i64,
        started: bool,
        completed: bool,
    ) -> crate::error::AppResult<()> {
        if completed {
            sqlx::query(
                r#"
                UPDATE documents 
                SET vision_completed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
                WHERE id = $1
                "#,
            )
            .bind(document_id)
            .execute(pool)
            .await?;
        }

        if started {
            sqlx::query(
                r#"
                UPDATE documents 
                SET vision_started_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
                WHERE id = $1
                "#,
            )
            .bind(document_id)
            .execute(pool)
            .await?;
        }

        Ok(())
    }

    pub async fn update_accountant_timestamp(
        pool: &DbPool,
        document_id: i64,
    ) -> crate::error::AppResult<()> {
        sqlx::query(
            r#"
            UPDATE documents 
            SET accountant_reviewed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            "#,
        )
        .bind(document_id)
        .execute(pool)
        .await?;

        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn field(
            field_type: &str,
            parsed_value: Option<&str>,
            source: &str,
        ) -> ExtractedFieldValue {
            ExtractedFieldValue {
                field_type: field_type.to_string(),
                parsed_value: parsed_value.map(str::to_string),
                source: source.to_string(),
            }
        }

        #[test]
        fn original_only_ignores_user_corrections() {
            let fields = resolve_extracted_field_rows(
                vec![
                    field("total_amount", Some("10.00"), "USER_CORRECTION"),
                    field("total_amount", Some("12.00"), "vision"),
                ],
                FieldLoadMode::OriginalOnly,
            );

            assert_eq!(
                fields
                    .get("total_amount")
                    .and_then(|value| value.as_deref()),
                Some("12.00")
            );
        }

        #[test]
        fn effective_with_corrections_prefers_user_corrections() {
            let fields = resolve_extracted_field_rows(
                vec![
                    field("total_amount", Some("10.00"), "USER_CORRECTION"),
                    field("total_amount", Some("12.00"), "vision"),
                    field("vat_amount", Some("0.00"), "vision"),
                ],
                FieldLoadMode::EffectiveWithCorrections,
            );

            assert_eq!(
                fields
                    .get("total_amount")
                    .and_then(|value| value.as_deref()),
                Some("10.00")
            );
            assert_eq!(
                fields.get("vat_amount").and_then(|value| value.as_deref()),
                Some("0.00")
            );
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn document_status_roundtrip() {
        use super::DocumentStatus;

        let variants = vec![
            DocumentStatus::Received,
            DocumentStatus::ProcessingVision,
            DocumentStatus::VisionComplete,
            DocumentStatus::ProcessingAccountant,
            DocumentStatus::AccountantReviewed,
            DocumentStatus::ProcessingValidator,
            DocumentStatus::Validated,
            DocumentStatus::PendingHumanReview,
            DocumentStatus::ExportReady,
            DocumentStatus::GeneratingSie4,
            DocumentStatus::ReviewCompleted,
            DocumentStatus::Exported,
            DocumentStatus::Archived,
            DocumentStatus::Failed,
        ];

        for status in &variants {
            let s = status.as_str();
            let round = DocumentStatus::from(s);
            assert_eq!(
                *status, round,
                "DocumentStatus {:?} -> {} -> {:?} did not roundtrip",
                status, s, round
            );
        }
    }

    #[test]
    fn document_status_from_unknown_defaults_to_received() {
        use super::DocumentStatus;
        let round = DocumentStatus::from("UNKNOWN_STATUS");
        assert_eq!(round, DocumentStatus::Received);
    }
}
