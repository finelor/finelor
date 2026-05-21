use crate::db::DbPool;
use serde_json::{Value, json};

use crate::agents::review::ReviewField;
use crate::agents::{process_human_review_text_input, publish_document_event};
use crate::queue::{Job, JobType, QueueProducer};
use crate::web::events::AppEventBus;

#[derive(sqlx::FromRow)]
struct PendingDocumentInteraction {
    id: i64,
    document_id: i64,
    interaction_kind: String,
    pending_action: String,
    payload: serde_json::Value,
}

pub struct DocumentInteractionTextInput<'a> {
    pub channel_type: &'a str,
    pub channel_identifier: &'a str,
    pub profile_identifier: Option<&'a str>,
    pub actor_identifier: &'a str,
    pub text: &'a str,
}

pub async fn start_document_interaction(
    pool: &DbPool,
    document_id: i64,
    channel_type: &str,
    channel_identifier: &str,
    profile_identifier: Option<&str>,
    field: ReviewField,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE document_interactions
        SET completed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
        WHERE channel_type = $1
          AND channel_identifier = $2
          AND COALESCE(profile_identifier, '') = COALESCE($3, '')
          AND interaction_kind = 'HUMAN_REVIEW'
          AND completed_at IS NULL
        "#,
    )
    .bind(channel_type)
    .bind(channel_identifier)
    .bind(profile_identifier)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO document_interactions (
            document_id, channel_type, channel_identifier,
            profile_identifier, interaction_kind, pending_action, payload,
            expires_at, created_at, updated_at
        )
        VALUES (
            $1, $2, $3,
            $4, 'HUMAN_REVIEW', 'EDIT_FIELD', $5,
            datetime('now', '+1 hour'), CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        )
        "#,
    )
    .bind(document_id)
    .bind(channel_type)
    .bind(channel_identifier)
    .bind(profile_identifier)
    .bind(json!({ "field_name": field.as_str() }))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

pub async fn start_sender_clarification_interaction(
    pool: &DbPool,
    events: &AppEventBus,
    document_id: i64,
    channel_type: &str,
    channel_identifier: &str,
    profile_identifier: Option<&str>,
    payload: Value,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE document_interactions
        SET completed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
        WHERE channel_type = $1
          AND channel_identifier = $2
          AND COALESCE(profile_identifier, '') = COALESCE($3, '')
          AND interaction_kind = 'SENDER_CLARIFICATION'
          AND completed_at IS NULL
        "#,
    )
    .bind(channel_type)
    .bind(channel_identifier)
    .bind(profile_identifier)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO document_interactions (
            document_id, channel_type, channel_identifier,
            profile_identifier, interaction_kind, pending_action, payload,
            expires_at, created_at, updated_at
        )
        VALUES (
            $1, $2, $3,
            $4, 'SENDER_CLARIFICATION', 'ANSWER_QUESTION', $5,
            datetime('now', '+7 days'), CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        )
        "#,
    )
    .bind(document_id)
    .bind(channel_type)
    .bind(channel_identifier)
    .bind(profile_identifier)
    .bind(&payload)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let event_id = crate::agents::db_helpers::record_document_event(
        pool,
        document_id,
        "SENDER_CLARIFICATION_REQUESTED",
        payload,
    )
    .await?;
    publish_document_event(
        pool,
        events,
        event_id,
        document_id,
        "SENDER_CLARIFICATION_REQUESTED",
    )
    .await?;

    Ok(())
}

pub async fn process_pending_document_interaction_text(
    pool: &DbPool,
    queue_producer: &QueueProducer,
    events: &AppEventBus,
    input: DocumentInteractionTextInput<'_>,
) -> anyhow::Result<Option<String>> {
    let interaction = resolve_pending_document_interaction(
        pool,
        input.channel_type,
        input.channel_identifier,
        input.profile_identifier,
    )
    .await?;

    let Some(interaction) = interaction else {
        return Ok(None);
    };

    match (
        interaction.interaction_kind.as_str(),
        interaction.pending_action.as_str(),
    ) {
        ("HUMAN_REVIEW", "EDIT_FIELD") => {
            let field_name = interaction
                .payload
                .get("field_name")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!("pending document interaction missing field_name")
                })?;
            let field = field_name.parse::<ReviewField>()?;

            let response = process_human_review_text_input(
                pool,
                queue_producer,
                interaction.document_id,
                field,
                input.actor_identifier,
                input.text,
            )
            .await?;

            complete_document_interaction(pool, interaction.id).await?;
            Ok(Some(response))
        }
        ("SENDER_CLARIFICATION", "ANSWER_QUESTION") => {
            store_sender_clarification_answer(
                pool,
                queue_producer,
                events,
                &interaction,
                input.actor_identifier,
                input.text,
            )
            .await
        }
        _ => Ok(None),
    }
}

async fn resolve_pending_document_interaction(
    pool: &DbPool,
    channel_type: &str,
    channel_identifier: &str,
    profile_identifier: Option<&str>,
) -> anyhow::Result<Option<PendingDocumentInteraction>> {
    let row = sqlx::query_as(
        r#"
        SELECT id, document_id, interaction_kind, pending_action, payload
        FROM document_interactions
        WHERE channel_type = $1
          AND channel_identifier = $2
          AND interaction_kind IN ('HUMAN_REVIEW', 'SENDER_CLARIFICATION')
          AND pending_action IN ('EDIT_FIELD', 'ANSWER_QUESTION')
          AND completed_at IS NULL
          AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)
          AND (
              profile_identifier = $3
              OR profile_identifier IS NULL
              OR $3 IS NULL
          )
        ORDER BY (profile_identifier = $3) DESC, updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(channel_type)
    .bind(channel_identifier)
    .bind(profile_identifier)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

async fn store_sender_clarification_answer(
    pool: &DbPool,
    queue_producer: &QueueProducer,
    events: &AppEventBus,
    interaction: &PendingDocumentInteraction,
    answered_by: &str,
    text: &str,
) -> anyhow::Result<Option<String>> {
    let answer_payload = json!({
        "answer": text,
        "answered_by": answered_by,
        "question": interaction.payload.get("question").cloned(),
        "context_key": interaction.payload.get("context_key").cloned(),
        "requested_by_stage": interaction.payload.get("requested_by_stage").cloned(),
    });

    let event_id = crate::agents::db_helpers::record_document_event(
        pool,
        interaction.document_id,
        "SENDER_CLARIFICATION_ANSWERED",
        answer_payload,
    )
    .await?;
    complete_document_interaction(pool, interaction.id).await?;
    publish_document_event(
        pool,
        events,
        event_id,
        interaction.document_id,
        "SENDER_CLARIFICATION_ANSWERED",
    )
    .await?;

    let resume_job_type = interaction
        .payload
        .get("resume_job_type")
        .and_then(|value| value.as_str())
        .and_then(parse_job_type)
        .unwrap_or(JobType::Accountant);
    queue_producer
        .enqueue(&Job::new(resume_job_type, interaction.document_id, 0))
        .await?;

    Ok(Some(
        "Thanks. I have saved that answer and will continue processing the document.".to_string(),
    ))
}

fn parse_job_type(value: &str) -> Option<JobType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "vision" => Some(JobType::Vision),
        "accountant" => Some(JobType::Accountant),
        "validator" => Some(JobType::Validator),
        "review" => Some(JobType::Review),
        _ => None,
    }
}

async fn complete_document_interaction(pool: &DbPool, interaction_id: i64) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE document_interactions
        SET completed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        "#,
    )
    .bind(interaction_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn clear_document_interactions(pool: &DbPool, document_id: i64) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE document_interactions
        SET completed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
          AND completed_at IS NULL
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

    #[test]
    fn parse_job_type_accepts_supported_jobs_case_insensitively() {
        assert!(matches!(parse_job_type(" vision "), Some(JobType::Vision)));
        assert!(matches!(
            parse_job_type("ACCOUNTANT"),
            Some(JobType::Accountant)
        ));
        assert!(matches!(
            parse_job_type("Validator"),
            Some(JobType::Validator)
        ));
        assert!(matches!(parse_job_type("review"), Some(JobType::Review)));
        assert!(parse_job_type("export").is_none());
    }

    #[test]
    fn parse_job_type_rejects_unknown_jobs() {
        assert!(parse_job_type("intake").is_none());
        assert!(parse_job_type("").is_none());
    }
}
