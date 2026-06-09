use std::collections::BTreeSet;

use crate::db::DbPool;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::messaging::contracts::MessageSource;

pub const DEFAULT_RECENT_TURN_LIMIT: i64 = 6;

#[derive(Debug, Clone)]
pub struct AgentSession {
    pub id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    pub metadata: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationReferents {
    pub latest_document_ref: Option<String>,
    pub latest_list_topic: Option<String>,
    pub latest_ordered_refs: Vec<String>,
    pub latest_skill_name: Option<String>,
    pub latest_skill_supporting_paths: Vec<String>,
    pub latest_action_topic: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationContext {
    pub messages: Vec<ConversationMessage>,
    pub referents: ConversationReferents,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTurnMetadata {
    #[serde(default)]
    pub user_refs: Vec<String>,
    #[serde(default)]
    pub assistant_refs: Vec<String>,
    #[serde(default)]
    pub tool_calls: Vec<String>,
    #[serde(default)]
    pub returned_refs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_name: Option<String>,
    #[serde(default)]
    pub skill_supporting_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_topic: Option<String>,
}

impl AgentTurnMetadata {
    pub fn from_user_text(text: &str) -> Self {
        Self {
            user_refs: extract_short_refs(text),
            ..Self::default()
        }
    }

    pub fn note_assistant_text(&mut self, text: &str) {
        self.assistant_refs = extract_short_refs(text);
    }

    pub fn note_tool_call(&mut self, tool_name: &str, args: &Value) {
        push_unique(&mut self.tool_calls, tool_name.to_string());
        if self.topic.is_none() {
            self.topic = list_topic_for_tool(tool_name).map(ToString::to_string);
        }
        if self.action_topic.is_none() {
            self.action_topic = action_topic_for_tool(tool_name).map(ToString::to_string);
        }
        if let Some(short_ref) = args.get("document_short_ref").and_then(Value::as_str) {
            push_unique(&mut self.returned_refs, normalize_short_ref_text(short_ref));
        }
    }

    pub fn note_tool_result(&mut self, tool_name: &str, result: &Value) {
        if self.topic.is_none() {
            self.topic = list_topic_for_tool(tool_name).map(ToString::to_string);
        }
        if self.action_topic.is_none() {
            self.action_topic = action_topic_for_tool(tool_name).map(ToString::to_string);
        }

        if let Some(items) = result
            .get("result")
            .and_then(|value| value.get("items"))
            .and_then(Value::as_array)
        {
            for item in items {
                if let Some(short_ref) = item.get("document_short_ref").and_then(Value::as_str) {
                    push_unique(&mut self.returned_refs, normalize_short_ref_text(short_ref));
                }
            }
        }

        if let Some(short_ref) = result
            .get("result")
            .and_then(|value| value.get("document_short_ref"))
            .and_then(Value::as_str)
        {
            push_unique(&mut self.returned_refs, normalize_short_ref_text(short_ref));
        }

        if let Some(document_short_ref) = result
            .get("result")
            .and_then(|value| value.get("document"))
            .and_then(|value| value.get("document_short_ref"))
            .and_then(Value::as_str)
        {
            push_unique(
                &mut self.returned_refs,
                normalize_short_ref_text(document_short_ref),
            );
        }

        if tool_name == "skill_view"
            && let Some(skill_name) = result
                .get("result")
                .and_then(|value| value.get("name").or_else(|| value.get("skill_name")))
                .and_then(Value::as_str)
        {
            self.skill_name = Some(skill_name.to_string());
        }

        if tool_name == "skill_view"
            && let Some(path) = result
                .get("result")
                .and_then(|value| value.get("requested_path"))
                .and_then(Value::as_str)
        {
            push_unique(&mut self.skill_supporting_paths, path.to_string());
        }
    }

    fn user_metadata(&self) -> Value {
        json!({
            "mentioned_refs": self.user_refs,
        })
    }

    fn assistant_metadata(&self) -> Value {
        json!({
            "mentioned_refs": self.assistant_refs,
            "tool_calls": self.tool_calls,
            "returned_refs": self.returned_refs,
            "topic": self.topic,
            "skill_name": self.skill_name,
            "skill_supporting_paths": self.skill_supporting_paths,
            "action_topic": self.action_topic,
        })
    }
}

pub async fn load_or_create_session(
    pool: &DbPool,
    session_key: &str,
    source: &MessageSource,
) -> anyhow::Result<AgentSession> {
    let row: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO agent_sessions (
            session_key,
            channel_type,
            channel_identifier,
            metadata
        )
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (session_key)
        DO UPDATE SET
            last_active_at = CURRENT_TIMESTAMP,
            channel_type = EXCLUDED.channel_type,
            channel_identifier = EXCLUDED.channel_identifier,
            metadata = EXCLUDED.metadata
        RETURNING id
        "#,
    )
    .bind(session_key)
    .bind(source.channel.as_str())
    .bind(&source.channel_identifier)
    .bind(json!({
        "chat_type": source.metadata.get("chat_type").and_then(Value::as_str),
        "profile_identifier": source.profile_identifier,
    }))
    .fetch_one(pool)
    .await?;

    Ok(AgentSession { id: row.0 })
}

pub async fn load_recent_context(
    pool: &DbPool,
    session_id: i64,
    max_turns: i64,
) -> anyhow::Result<ConversationContext> {
    let messages = sqlx::query_as::<_, (String, String, Value)>(
        r#"
        SELECT role, content, metadata
        FROM agent_messages
        WHERE session_id = $1
          AND turn_index >= COALESCE(
              (
                  SELECT MAX(turn_index) - $2 + 1
                  FROM agent_messages
                  WHERE session_id = $1
              ),
              0
          )
        ORDER BY turn_index ASC, CASE WHEN role = 'user' THEN 0 ELSE 1 END ASC, created_at ASC
        "#,
    )
    .bind(session_id)
    .bind(max_turns.max(1))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(role, content, metadata)| ConversationMessage {
        role,
        content,
        metadata,
    })
    .collect::<Vec<_>>();

    let referents = derive_referents(&messages);
    Ok(ConversationContext {
        messages,
        referents,
    })
}

pub async fn append_completed_turn(
    pool: &DbPool,
    session: &AgentSession,
    source: &MessageSource,
    user_text: &str,
    assistant_text: &str,
    mut metadata: AgentTurnMetadata,
) -> anyhow::Result<()> {
    metadata.note_assistant_text(assistant_text);
    let turn_index: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(MAX(turn_index), 0) + 1
        FROM agent_messages
        WHERE session_id = $1
        "#,
    )
    .bind(session.id)
    .fetch_one(pool)
    .await?;

    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO agent_messages (
            session_id,
            turn_index,
            role,
            content,
            source_message_id,
            profile_identifier,
            metadata
        )
        VALUES ($1, $2, 'user', $3, $4, $5, $6)
        "#,
    )
    .bind(session.id)
    .bind(turn_index)
    .bind(user_text)
    .bind(source.message_id.as_deref())
    .bind(source.profile_identifier.as_deref())
    .bind(metadata.user_metadata())
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO agent_messages (
            session_id,
            turn_index,
            role,
            content,
            source_message_id,
            profile_identifier,
            metadata
        )
        VALUES ($1, $2, 'assistant', $3, NULL, NULL, $4)
        "#,
    )
    .bind(session.id)
    .bind(turn_index)
    .bind(assistant_text)
    .bind(metadata.assistant_metadata())
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE agent_sessions SET last_active_at = CURRENT_TIMESTAMP WHERE id = $1")
        .bind(session.id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

pub fn extract_short_refs(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut refs = BTreeSet::new();
    let mut i = 0;

    while i + 7 <= bytes.len() {
        if bytes[i].eq_ignore_ascii_case(&b'D')
            && bytes[i + 1..i + 7].iter().all(u8::is_ascii_digit)
        {
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            let after_ok = i + 7 == bytes.len() || !bytes[i + 7].is_ascii_alphanumeric();
            if before_ok && after_ok {
                refs.insert(text[i..i + 7].to_ascii_uppercase());
                i += 7;
                continue;
            }
        }
        i += 1;
    }

    refs.into_iter().collect()
}

pub fn derive_referents(messages: &[ConversationMessage]) -> ConversationReferents {
    let mut latest_document_ref = None;
    let mut latest_list_topic = None;
    let mut latest_ordered_refs = Vec::new();
    let mut latest_skill_name = None;
    let mut latest_skill_supporting_paths = Vec::new();
    let mut latest_action_topic = None;

    for message in messages.iter().rev() {
        if latest_document_ref.is_none()
            && let Some(refs) = message
                .metadata
                .get("mentioned_refs")
                .and_then(Value::as_array)
        {
            latest_document_ref = refs
                .iter()
                .filter_map(Value::as_str)
                .next_back()
                .map(ToString::to_string);
        }

        if latest_ordered_refs.is_empty()
            && let Some(refs) = message
                .metadata
                .get("returned_refs")
                .and_then(Value::as_array)
        {
            latest_ordered_refs = refs
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect();
        }

        if latest_list_topic.is_none() {
            latest_list_topic = message
                .metadata
                .get("topic")
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }

        if latest_skill_name.is_none() {
            latest_skill_name = message
                .metadata
                .get("skill_name")
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }

        if latest_skill_supporting_paths.is_empty()
            && let Some(paths) = message
                .metadata
                .get("skill_supporting_paths")
                .and_then(Value::as_array)
        {
            latest_skill_supporting_paths = paths
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect();
        }

        if latest_action_topic.is_none() {
            latest_action_topic = message
                .metadata
                .get("action_topic")
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }

        if latest_document_ref.is_some()
            && latest_list_topic.is_some()
            && !latest_ordered_refs.is_empty()
            && latest_skill_name.is_some()
            && latest_action_topic.is_some()
            && !latest_skill_supporting_paths.is_empty()
        {
            break;
        }
    }

    if latest_document_ref.is_none() {
        latest_document_ref = latest_ordered_refs.first().cloned();
    }

    ConversationReferents {
        latest_document_ref,
        latest_list_topic,
        latest_ordered_refs,
        latest_skill_name,
        latest_skill_supporting_paths,
        latest_action_topic,
    }
}

fn list_topic_for_tool(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "document_status_summary" => Some("company document status summary"),
        "list_documents" => Some("recent documents"),
        "get_document" => Some("specific document status"),
        "explain_document" => Some("document explanation"),
        "list_pending_reviews" => Some("pending review documents"),
        "list_export_ready" => Some("export-ready documents"),
        "list_accounting_eligible_documents" => Some("vision processed documents"),
        _ => None,
    }
}

fn action_topic_for_tool(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "prepare_open_review" => Some("open document review"),
        "prepare_retry_document" => Some("retry document processing"),
        "prepare_export_documents" => Some("export documents"),
        "prepare_process_accounting_documents" => Some("process documents for accounting"),
        _ => None,
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn normalize_short_ref_text(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_short_refs_normalizes_and_deduplicates() {
        assert_eq!(
            extract_short_refs("Check d000057, D000058, and D000057 again."),
            vec!["D000057".to_string(), "D000058".to_string()]
        );
    }

    #[test]
    fn extract_short_refs_ignores_embedded_values() {
        assert!(extract_short_refs("XD000057 D000057A D00005").is_empty());
    }

    #[test]
    fn derive_referents_prefers_latest_ordered_list() {
        let messages = vec![
            ConversationMessage {
                role: "assistant".to_string(),
                content: "Older".to_string(),
                metadata: json!({
                    "returned_refs": ["D000001"],
                    "topic": "recent documents"
                }),
            },
            ConversationMessage {
                role: "assistant".to_string(),
                content: "Latest".to_string(),
                metadata: json!({
                    "returned_refs": ["D000057", "D000061"],
                    "topic": "pending review documents"
                }),
            },
        ];

        let referents = derive_referents(&messages);
        assert_eq!(referents.latest_document_ref.as_deref(), Some("D000057"));
        assert_eq!(
            referents.latest_list_topic.as_deref(),
            Some("pending review documents")
        );
        assert_eq!(
            referents.latest_ordered_refs,
            vec!["D000057".to_string(), "D000061".to_string()]
        );
    }

    #[test]
    fn note_tool_result_tracks_skill_and_supporting_files() {
        let mut metadata = AgentTurnMetadata::default();

        metadata.note_tool_result(
            "skill_view",
            &json!({
                "ok": true,
                "tool": "skill_view",
                "result": {
                    "name": "Document Completeness Checklist"
                }
            }),
        );
        metadata.note_tool_result(
            "skill_view",
            &json!({
                "ok": true,
                "tool": "skill_view",
                "result": {
                    "skill_name": "Document Completeness Checklist",
                    "requested_path": "references/checklist-rules.md"
                }
            }),
        );
        metadata.note_tool_result(
            "skill_view",
            &json!({
                "ok": true,
                "tool": "skill_view",
                "result": {
                    "skill_name": "Document Completeness Checklist",
                    "requested_path": "templates/document-completeness-checklist.md"
                }
            }),
        );

        assert_eq!(
            metadata.skill_name.as_deref(),
            Some("Document Completeness Checklist")
        );
        assert_eq!(
            metadata.skill_supporting_paths,
            vec![
                "references/checklist-rules.md".to_string(),
                "templates/document-completeness-checklist.md".to_string()
            ]
        );
    }

    #[test]
    fn note_tool_call_and_result_keep_action_topics_separate() {
        let mut metadata = AgentTurnMetadata::default();

        metadata.note_tool_call(
            "prepare_retry_document",
            &json!({"document_short_ref": "D000057"}),
        );
        metadata.note_tool_result(
            "prepare_retry_document",
            &json!({
                "ok": true,
                "tool": "prepare_retry_document",
                "result": {}
            }),
        );

        assert!(metadata.topic.is_none());
        assert_eq!(
            metadata.action_topic.as_deref(),
            Some("retry document processing")
        );
        assert_eq!(metadata.returned_refs, vec!["D000057".to_string()]);
    }

    #[test]
    fn derive_referents_tracks_skill_and_action_context() {
        let messages = vec![
            ConversationMessage {
                role: "assistant".to_string(),
                content: "Used a skill".to_string(),
                metadata: json!({
                    "skill_name": "Document Completeness Checklist",
                    "skill_supporting_paths": [
                        "references/checklist-rules.md",
                        "templates/document-completeness-checklist.md"
                    ],
                    "topic": "recent documents",
                    "returned_refs": ["D000057", "D000061"]
                }),
            },
            ConversationMessage {
                role: "assistant".to_string(),
                content: "Prepared retry".to_string(),
                metadata: json!({
                    "action_topic": "retry document processing"
                }),
            },
        ];

        let referents = derive_referents(&messages);
        assert_eq!(
            referents.latest_skill_name.as_deref(),
            Some("Document Completeness Checklist")
        );
        assert_eq!(
            referents.latest_skill_supporting_paths,
            vec![
                "references/checklist-rules.md".to_string(),
                "templates/document-completeness-checklist.md".to_string()
            ]
        );
        assert_eq!(
            referents.latest_action_topic.as_deref(),
            Some("retry document processing")
        );
        assert_eq!(
            referents.latest_list_topic.as_deref(),
            Some("recent documents")
        );
        assert_eq!(
            referents.latest_ordered_refs,
            vec!["D000057".to_string(), "D000061".to_string()]
        );
    }
}
