//! Integration tests for gateway, tool, and review interaction information flow.

mod common;

use std::sync::Arc;

use common::TestDocumentStatePreset as Preset;
use finelor::agents::review::ReviewField;
use finelor::db::ChannelType;
use finelor::kv::EphemeralStore;
use finelor::messaging::confirmations::{
    AgentConfirmationActionKind, NewAgentConfirmation, create_agent_confirmation,
};
use finelor::messaging::contracts::{AgentInboundMessage, MessageSource};
use finelor::messaging::conversation::load_recent_context;
use finelor::messaging::gateway::{AgentGatewayState, build_session_key};
use finelor::messaging::interactions::{
    DocumentInteractionTextInput, process_pending_document_interaction_text,
    start_document_interaction,
};
use finelor::queue::{JobType, QueueProducer};
use finelor::skills::SkillRegistry;
use finelor::web::events::AppEventBus;
use serde_json::json;
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn source(chat_id: &str) -> MessageSource {
    MessageSource {
        channel: ChannelType::Telegram,
        channel_identifier: chat_id.to_string(),
        profile_identifier: Some("profile-1".to_string()),
        message_id: Some("1".to_string()),
        source_timestamp: None,
        metadata: json!({ "chat_type": "private" }),
    }
}

async fn create_document(pool: &SqlitePool, preset: Preset) -> (i64, String) {
    common::create_document_with_state_preset(
        pool,
        preset,
        Some("/tmp/message-flow.pdf"),
        "application/pdf",
    )
    .await
}

async fn insert_field(pool: &SqlitePool, document_id: i64, field_type: &str, value: &str) {
    sqlx::query(
        r#"
        INSERT INTO extracted_fields (document_id, field_type, raw_value, parsed_value, parsed_type, confidence, source)
        VALUES ($1, $2, $3, $3, 'text', 0.9, 'test')
        "#,
    )
    .bind(document_id)
    .bind(field_type)
    .bind(value)
    .execute(pool)
    .await
    .expect("insert extracted field");
}

fn gateway_state(
    pool: SqlitePool,
    config: finelor::config::AppConfig,
    producer: QueueProducer,
) -> AgentGatewayState {
    AgentGatewayState {
        config: Arc::new(config),
        pool,
        queue_producer: producer,
        ephemeral_store: EphemeralStore::new(),
        events: AppEventBus::new(16),
        running_sessions: Arc::new(Mutex::new(Default::default())),
        active_document_interaction_sessions: Arc::new(Mutex::new(Default::default())),
        skills_registry: Arc::new(SkillRegistry::new()),
    }
}

#[tokio::test]
async fn gateway_status_command_uses_db_tool_and_persists_conversation_context() {
    let pool = common::in_memory_pool().await;
    let queue = common::test_queue();
    let producer = QueueProducer::new(queue);
    let (_, short_ref) = create_document(&pool, Preset::PendingReview).await;

    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "test-agent-chat",
            "created_at": "2026-05-20T00:00:00Z",
            "message": {
                "role": "assistant",
                "content": format!("{} is pending human review.", short_ref)
            },
            "done": true,
            "eval_count": 1
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let mut config = common::test_config();
    config.ollama.base_url = mock_server.uri();
    let state = gateway_state(pool.clone(), config, producer);
    let source = source("status-chat");
    let session_key = build_session_key(&source);

    let response = state
        .handle_inbound_message(AgentInboundMessage::Text {
            source: source.clone(),
            text: format!("What is the status of {}?", short_ref),
        })
        .await
        .expect("gateway status");

    assert!(response.message.contains(&short_ref));

    let session_id: i64 =
        sqlx::query_scalar("SELECT id FROM agent_sessions WHERE session_key = $1")
            .bind(&session_key)
            .fetch_one(&pool)
            .await
            .expect("agent session");
    let context = load_recent_context(&pool, session_id, 4)
        .await
        .expect("recent context");
    assert_eq!(context.messages.len(), 2);
    assert_eq!(
        context.referents.latest_document_ref.as_deref(),
        Some(short_ref.as_str())
    );
}

#[tokio::test]
async fn pending_review_text_interaction_persists_correction_and_queues_validator() {
    let pool = common::in_memory_pool().await;
    let queue = common::test_queue();
    let producer = QueueProducer::new(queue.clone());
    let (document_id, _) = create_document(&pool, Preset::PendingReview).await;
    insert_field(&pool, document_id, "supplier_name", "Old Supplier").await;
    sqlx::query(
        r#"
        INSERT INTO review_decisions (document_id, decision_type, human_review_required, review_reason)
        VALUES ($1, 'PENDING_HUMAN_REVIEW', TRUE, 'Supplier needs correction')
        "#,
    )
    .bind(document_id)
    .execute(&pool)
    .await
    .expect("insert review decision");

    start_document_interaction(
        &pool,
        document_id,
        "TELEGRAM",
        "review-chat",
        Some("profile-1"),
        ReviewField::Supplier,
    )
    .await
    .expect("start interaction");

    let message = process_pending_document_interaction_text(
        &pool,
        &producer,
        &AppEventBus::new(16),
        DocumentInteractionTextInput {
            channel_type: "TELEGRAM",
            channel_identifier: "review-chat",
            profile_identifier: Some("profile-1"),
            actor_identifier: "profile-1",
            text: "Correct Supplier AB",
        },
    )
    .await
    .expect("process interaction")
    .expect("interaction response");

    assert!(message.contains("queued for re-validation"));
    let supplier: String = sqlx::query_scalar(
        "SELECT parsed_value FROM extracted_fields WHERE document_id = $1 AND field_type = 'supplier_name'",
    )
    .bind(document_id)
    .fetch_one(&pool)
    .await
    .expect("supplier correction");
    assert_eq!(supplier, "Correct Supplier AB");

    let corrections: serde_json::Value =
        sqlx::query_scalar("SELECT corrections FROM review_decisions WHERE document_id = $1")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .expect("corrections");
    assert_eq!(corrections["supplier"], "Correct Supplier AB");

    let completed_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM document_interactions WHERE document_id = $1 AND completed_at IS NOT NULL",
    )
    .bind(document_id)
    .fetch_one(&pool)
    .await
    .expect("completed interactions");
    assert_eq!(completed_count, 1);

    let jobs = common::drain_jobs(&queue, 10).await;
    assert_eq!(jobs.len(), 1);
    assert!(matches!(jobs[0].1.job_type, JobType::Validator));
}

#[tokio::test]
async fn confirmed_accounting_processing_marks_request_and_queues_accountant() {
    let pool = common::in_memory_pool().await;
    let queue = common::test_queue();
    let producer = QueueProducer::new(queue.clone());
    let (document_id, short_ref) = create_document(&pool, Preset::IntakeIngested).await;

    let state = gateway_state(pool.clone(), common::test_config(), producer);
    let source = source("accounting-chat");
    let confirmation = create_agent_confirmation(
        &state.ephemeral_store,
        NewAgentConfirmation {
            workspace_id: finelor::workspace::active_workspace_id(),
            document_id: None,
            channel_type: source.channel.as_str().to_string(),
            channel_identifier: source.channel_identifier.clone(),
            profile_identifier: source.profile_identifier.clone(),
            action_kind: AgentConfirmationActionKind::ProcessAccountingDocuments,
            payload: json!({
                "document_ids": [document_id],
                "document_short_refs": [short_ref]
            }),
        },
    )
    .await
    .expect("create confirmation");

    let response = state
        .handle_document_action(
            &source,
            finelor::messaging::contracts::DocumentAction::ConfirmAgentAction {
                confirmation_id: confirmation.id,
            },
        )
        .await
        .expect("confirm accounting processing");

    assert!(!response.message.is_empty());

    let requested_at: Option<String> = sqlx::query_scalar(
        "SELECT requested_at FROM document_accounting_state WHERE document_id = $1",
    )
    .bind(document_id)
    .fetch_one(&pool)
    .await
    .expect("accounting requested");
    assert!(requested_at.is_some());

    let jobs = common::drain_jobs(&queue, 10).await;
    assert_eq!(jobs.len(), 1);
    assert!(matches!(jobs[0].1.job_type, JobType::Accountant));
}
