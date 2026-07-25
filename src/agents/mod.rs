use std::collections::HashMap;
use std::fmt::Display;

use crate::db::DbPool;
use crate::web::events::{AppEvent, AppEventBus};
use serde::{Deserialize, Serialize};
use serde_json::json;

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

    #[cfg(test)]
    mod tests {
        use super::{ExtractedFieldValue, FieldLoadMode, resolve_extracted_field_rows};

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
