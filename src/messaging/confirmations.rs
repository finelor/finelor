use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::AppResult;
use crate::kv::EphemeralStore;

const CONFIRMATION_TTL_SECONDS: usize = 15 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AgentConfirmationActionKind {
    OpenReview,
    RetryDocument,
    ExportDocuments,
    GenerateInvoice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentConfirmation {
    pub id: String,
    #[serde(alias = "workspace_id")]
    pub workspace_id: Uuid,
    pub document_id: Option<i64>,
    pub channel_type: String,
    pub channel_identifier: String,
    pub profile_identifier: Option<String>,
    pub action_kind: AgentConfirmationActionKind,
    pub payload: Value,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct NewAgentConfirmation {
    pub workspace_id: Uuid,
    pub document_id: Option<i64>,
    pub channel_type: String,
    pub channel_identifier: String,
    pub profile_identifier: Option<String>,
    pub action_kind: AgentConfirmationActionKind,
    pub payload: Value,
}

pub async fn create_agent_confirmation(
    store: &EphemeralStore,
    input: NewAgentConfirmation,
) -> AppResult<AgentConfirmation> {
    let confirmation = AgentConfirmation {
        id: new_confirmation_id(),
        workspace_id: input.workspace_id,
        document_id: input.document_id,
        channel_type: input.channel_type,
        channel_identifier: input.channel_identifier,
        profile_identifier: input.profile_identifier,
        action_kind: input.action_kind,
        payload: input.payload,
        expires_at: Utc::now().timestamp() + CONFIRMATION_TTL_SECONDS as i64,
    };

    let value = serde_json::to_string(&confirmation)?;
    store
        .set(
            &agent_confirmation_key(&confirmation.id),
            &value,
            Some(CONFIRMATION_TTL_SECONDS as u64),
        )
        .await?;

    Ok(confirmation)
}

pub async fn consume_agent_confirmation(
    store: &EphemeralStore,
    confirmation_id: &str,
) -> AppResult<Option<AgentConfirmation>> {
    let value = store
        .consume(&agent_confirmation_key(confirmation_id))
        .await?;

    Ok(value.and_then(|value| serde_json::from_str::<AgentConfirmation>(&value).ok()))
}

pub async fn cancel_agent_confirmation(
    store: &EphemeralStore,
    confirmation_id: &str,
) -> AppResult<bool> {
    store.delete(&agent_confirmation_key(confirmation_id)).await
}

pub fn confirmation_ttl_seconds() -> usize {
    CONFIRMATION_TTL_SECONDS
}

fn agent_confirmation_key(id: &str) -> String {
    format!("agent_confirmation:{id}")
}

fn new_confirmation_id() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn confirmation_payload_serializes_scope_and_action() {
        let confirmation = AgentConfirmation {
            id: "abc".to_string(),
            workspace_id: Uuid::parse_str("37fd57d2-51d9-42a3-9bda-8d01f9ad03e1").unwrap(),
            document_id: Some(7),
            channel_type: "TELEGRAM".to_string(),
            channel_identifier: "123".to_string(),
            profile_identifier: Some("456".to_string()),
            action_kind: AgentConfirmationActionKind::RetryDocument,
            payload: json!({ "short_ref": "D000057" }),
            expires_at: 123,
        };

        let value = serde_json::to_value(&confirmation).unwrap();
        assert_eq!(
            value["workspace_id"],
            "37fd57d2-51d9-42a3-9bda-8d01f9ad03e1"
        );
        assert_eq!(value["action_kind"], "RETRY_DOCUMENT");
        assert_eq!(value["payload"]["short_ref"], "D000057");
    }

    #[test]
    fn ttl_is_short_lived() {
        assert_eq!(confirmation_ttl_seconds(), 900);
    }
}
