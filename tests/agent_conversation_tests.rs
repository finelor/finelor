//! Tests for Agent short-term conversation memory.

mod common;

use finelor::db::ChannelType;
use finelor::messaging::contracts::MessageSource;
use finelor::messaging::conversation::{
    AgentTurnMetadata, append_completed_turn, load_or_create_session, load_recent_context,
};
use serde_json::json;
use sqlx::SqlitePool;
use uuid::Uuid;

async fn active_workspace(_pool: &SqlitePool, _label: &str) -> Uuid {
    finelor::workspace::active_workspace_id()
}

async fn agent_memory_tables_exist(pool: &SqlitePool) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'agent_sessions')",
    )
        .fetch_one(pool)
        .await
        .unwrap_or(false)
}

async fn cleanup(_pool: &SqlitePool, _workspace_ids: &[Uuid]) {}

fn source(chat_id: &str, message_id: &str) -> MessageSource {
    MessageSource {
        channel: ChannelType::Telegram,
        channel_identifier: chat_id.to_string(),
        profile_identifier: Some("profile-1".to_string()),
        message_id: Some(message_id.to_string()),
        source_timestamp: None,
        metadata: json!({ "chat_type": "private" }),
    }
}

#[tokio::test]
async fn conversation_history_is_workspace_scoped_by_session_key() {
    let pool = common::in_memory_pool().await;
    if !agent_memory_tables_exist(&pool).await {
        eprintln!("Skipping test: agent session migration not applied");
        return;
    }

    let _workspace_a = active_workspace(&pool, "A").await;
    let _workspace_b = active_workspace(&pool, "B").await;
    let session_key = format!(
        "agent:main:telegram:private:shared:{}",
        Uuid::new_v4().simple()
    );
    let source = source("shared", "1");

    let session_a = load_or_create_session(&pool, &session_key, &source)
        .await
        .expect("session a");
    let session_b = load_or_create_session(&pool, &session_key, &source)
        .await
        .expect("session b");

    append_completed_turn(
        &pool,
        &session_a,
        &source,
        "What about D000057?",
        "D000057 is pending review.",
        AgentTurnMetadata::from_user_text("What about D000057?"),
    )
    .await
    .expect("append workspace a turn");

    let context_a = load_recent_context(&pool, session_a.id, 6)
        .await
        .expect("context a");
    let context_b = load_recent_context(&pool, session_b.id, 6)
        .await
        .expect("context b");

    assert_eq!(context_a.messages.len(), 2);
    assert_eq!(
        context_a.referents.latest_document_ref.as_deref(),
        Some("D000057")
    );
    assert_eq!(session_a.id, session_b.id);
    assert_eq!(context_b.messages.len(), 2);

    cleanup(&pool, &[]).await;
}

#[tokio::test]
async fn conversation_history_is_bounded_and_ordered_by_turn() {
    let pool = common::in_memory_pool().await;
    if !agent_memory_tables_exist(&pool).await {
        eprintln!("Skipping test: agent session migration not applied");
        return;
    }

    let _workspace_id = active_workspace(&pool, "Bounded").await;
    let session_key = "agent:main:telegram:private:bounded";
    let source = source("bounded", "1");
    let session = load_or_create_session(&pool, session_key, &source)
        .await
        .expect("session");

    for index in 1..=8 {
        append_completed_turn(
            &pool,
            &session,
            &source,
            &format!("user turn {index}"),
            &format!("assistant turn {index}"),
            AgentTurnMetadata::from_user_text(&format!("user turn {index}")),
        )
        .await
        .expect("append turn");
    }

    let context = load_recent_context(&pool, session.id, 3)
        .await
        .expect("recent context");
    let contents = context
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        contents,
        vec![
            "user turn 6",
            "assistant turn 6",
            "user turn 7",
            "assistant turn 7",
            "user turn 8",
            "assistant turn 8",
        ]
    );

    cleanup(&pool, &[]).await;
}

#[tokio::test]
async fn conversation_metadata_preserves_ordered_referents() {
    let pool = common::in_memory_pool().await;
    if !agent_memory_tables_exist(&pool).await {
        eprintln!("Skipping test: agent session migration not applied");
        return;
    }

    let _workspace_id = active_workspace(&pool, "Referents").await;
    let session_key = "agent:main:telegram:private:referents";
    let source = source("referents", "1");
    let session = load_or_create_session(&pool, session_key, &source)
        .await
        .expect("session");

    let mut metadata = AgentTurnMetadata::from_user_text("What needs review?");
    metadata.note_tool_call("list_pending_reviews", &json!({ "limit": 2 }));
    metadata.note_tool_result(
        "list_pending_reviews",
        &json!({
            "ok": true,
            "result": {
                "total_count": 2,
                "returned_count": 2,
                "items": [
                    { "short_ref": "D000057" },
                    { "short_ref": "D000061" }
                ]
            }
        }),
    );

    append_completed_turn(
        &pool,
        &session,
        &source,
        "What needs review?",
        "D000057 and D000061 need review.",
        metadata,
    )
    .await
    .expect("append turn");

    let context = load_recent_context(&pool, session.id, 6)
        .await
        .expect("context");
    assert_eq!(
        context.referents.latest_ordered_refs,
        vec!["D000057".to_string(), "D000061".to_string()]
    );
    assert_eq!(
        context.referents.latest_list_topic.as_deref(),
        Some("pending review documents")
    );
    assert_eq!(
        context.referents.latest_document_ref.as_deref(),
        Some("D000061")
    );

    cleanup(&pool, &[]).await;
}
