use std::collections::HashSet;
use std::sync::Arc;

use crate::db::DbPool;
use crate::kv::EphemeralStore;
use serde_json::json;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::agents::review::{HumanReviewKeyboard, ReviewField};
use crate::agents::{HumanReviewCallbackResult, process_human_review_callback};
use crate::config::AppConfig;
use crate::inference::{OllamaProvider, ToolChatMessage};
use crate::query::{document_ref_by_short_ref, workspace_identity};
use crate::queue::QueueProducer;
use crate::skills::SkillRegistry;
use crate::web::events::AppEventBus;

use super::commands::{
    build_help_message, execute_gateway_intent, export_documents, reopen_review_actions,
    retry_document,
};
use super::confirmations::{
    AgentConfirmation, AgentConfirmationActionKind, NewAgentConfirmation,
    cancel_agent_confirmation, consume_agent_confirmation, create_agent_confirmation,
};
use super::contracts::{
    ActionButton, AgentInboundMessage, DocumentAction, GatewayActionResponse,
    GatewayMessageResponse, MessageSource, ReviewFieldAction,
};
use super::conversation::{
    AgentSession, AgentTurnMetadata, ConversationContext, DEFAULT_RECENT_TURN_LIMIT,
    append_completed_turn, load_or_create_session, load_recent_context,
};
use super::intents::{
    GatewayIntentArgs, GatewayIntentKind, GatewayIntentResolution, normalize_short_ref,
    parse_slash_intent,
};
use super::interactions::{
    DocumentInteractionTextInput, clear_document_interactions,
    process_pending_document_interaction_text, start_document_interaction,
};
use super::prompt::{PromptAssemblyInput, WorkspaceIdentitySnapshot, assemble_chat_messages};
use super::tools::{
    MAX_READ_ONLY_TOOL_CALLS, ReadOnlyToolCall, agent_inference_tools, execute_read_only_tool,
    is_mutating_prepare_tool, read_only_inference_tools,
};

enum AgentTurnDirective {
    Freeform,
    ForcedReadOnlyTool(ReadOnlyToolCall),
}

enum SlashRoute {
    Help,
    ReadOnlyTool(ReadOnlyToolCall),
    DeterministicAction,
}

struct AgentLoopResult {
    response: GatewayMessageResponse,
    metadata: AgentTurnMetadata,
}

#[derive(Clone)]
pub struct AgentGatewayState {
    pub config: Arc<AppConfig>,
    pub pool: DbPool,
    pub queue_producer: QueueProducer,
    pub ephemeral_store: EphemeralStore,
    pub events: AppEventBus,
    pub running_sessions: Arc<Mutex<HashSet<String>>>,
    pub active_document_interaction_sessions: Arc<Mutex<HashSet<String>>>,
    pub skills_registry: Arc<SkillRegistry>,
}

impl AgentGatewayState {
    pub async fn handle_inbound_message(
        &self,
        message: AgentInboundMessage,
    ) -> anyhow::Result<GatewayMessageResponse> {
        match message {
            AgentInboundMessage::Text { source, text } => {
                self.handle_text_message(source, text).await
            }
            AgentInboundMessage::Attachment { .. } => Ok(GatewayMessageResponse::text(
                "Send invoices or receipts as files or images so I can process them.",
            )),
            AgentInboundMessage::Action { .. } => Ok(GatewayMessageResponse::text(
                "That action is not available in chat yet.",
            )),
        }
    }

    async fn handle_text_message(
        &self,
        source: MessageSource,
        text: String,
    ) -> anyhow::Result<GatewayMessageResponse> {
        let session_key = build_session_key(&source);
        let reviewed_by = source
            .profile_identifier
            .as_deref()
            .unwrap_or(source.channel_identifier.as_str());
        let response = process_pending_document_interaction_text(
            &self.pool,
            &self.queue_producer,
            &self.events,
            DocumentInteractionTextInput {
                channel_type: source.channel.as_str(),
                channel_identifier: &source.channel_identifier,
                profile_identifier: source.profile_identifier.as_deref(),
                actor_identifier: reviewed_by,
                text: &text,
            },
        )
        .await?;

        if let Some(message) = response {
            self.clear_active_document_interaction(&session_key).await;
            return Ok(GatewayMessageResponse::text(message));
        }

        let workspace_id = crate::workspace::active_workspace_id();

        if !self.try_acquire_session(&session_key).await {
            return Ok(GatewayMessageResponse::text(
                "I am still working on your previous message. Please wait a moment.",
            ));
        }

        let result = self
            .route_text_message(workspace_id, &session_key, &source, &text)
            .await;
        self.release_session(&session_key).await;
        result
    }

    async fn route_text_message(
        &self,
        workspace_id: Uuid,
        session_key: &str,
        source: &MessageSource,
        text: &str,
    ) -> anyhow::Result<GatewayMessageResponse> {
        if let Some(resolution) = parse_slash_intent(text) {
            return match slash_route(&resolution) {
                SlashRoute::Help => Ok(GatewayMessageResponse::text(build_help_message())),
                SlashRoute::ReadOnlyTool(tool_call) => {
                    self.run_agent_chat(
                        workspace_id,
                        session_key,
                        source,
                        text,
                        AgentTurnDirective::ForcedReadOnlyTool(tool_call),
                    )
                    .await
                }
                SlashRoute::DeterministicAction => {
                    tracing::info!(
                        intent = resolution.intent.as_str(),
                        "Deterministic action command routed"
                    );
                    execute_gateway_intent(self, source, workspace_id, &resolution).await
                }
            };
        }

        self.run_agent_chat(
            workspace_id,
            session_key,
            source,
            text,
            AgentTurnDirective::Freeform,
        )
        .await
    }

    async fn run_agent_chat(
        &self,
        workspace_id: Uuid,
        session_key: &str,
        source: &MessageSource,
        text: &str,
        directive: AgentTurnDirective,
    ) -> anyhow::Result<GatewayMessageResponse> {
        tracing::info!(
            workspace_id = %workspace_id,
            session_key = session_key,
            "Agent loop started"
        );

        let workspace_identity = match self.build_workspace_identity_snapshot().await {
            Ok(snapshot) => Some(snapshot),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    workspace_id = %workspace_id,
                    "Failed to build accounting context snapshot"
                );
                None
            }
        };
        let (conversation_session, conversation_context) =
            self.load_short_term_memory(session_key, source).await;
        let mut messages = assemble_chat_messages(PromptAssemblyInput {
            config: &self.config,
            workspace_id,
            session_key,
            source,
            text,
            workspace_identity,
            conversation_context,
            skills_registry: Some(self.skills_registry.clone()),
        })
        .await;

        let allow_mutating_tools = matches!(directive, AgentTurnDirective::Freeform);
        let mut metadata = AgentTurnMetadata::from_user_text(text);
        if let AgentTurnDirective::ForcedReadOnlyTool(tool_call) = &directive {
            let tool_result =
                execute_read_only_tool(&self.pool, Some(&self.skills_registry), tool_call).await;
            metadata.note_tool_call(&tool_call.name, &tool_call.args);
            metadata.note_tool_result(&tool_call.name, &tool_result);
            tracing::info!(
                workspace_id = %workspace_id,
                tool = tool_call.name,
                ok = tool_result.get("ok").and_then(|value| value.as_bool()).unwrap_or(false),
                "Forced read-only tool executed"
            );
            messages.push(crate::inference::ChatMessage {
                role: "user".to_string(),
                content: format!(
                    "Forced read-only tool result for `{}`:\n{}\n\nUse this result to answer the user's slash command in concise plain text.",
                    tool_call.name, tool_result
                ),
            });
        }

        let result = self
            .run_agent_tool_loop(
                messages,
                workspace_id,
                source,
                allow_mutating_tools,
                metadata,
            )
            .await?;
        self.persist_completed_turn(
            conversation_session.as_ref(),
            source,
            text,
            &result.response,
            result.metadata,
        )
        .await;
        Ok(result.response)
    }

    async fn run_agent_tool_loop(
        &self,
        messages: Vec<crate::inference::ChatMessage>,
        workspace_id: Uuid,
        source: &MessageSource,
        allow_mutating_tools: bool,
        mut metadata: AgentTurnMetadata,
    ) -> anyhow::Result<AgentLoopResult> {
        let client = OllamaProvider::new(&self.config.ollama);
        let tools = if allow_mutating_tools {
            agent_inference_tools()
        } else {
            read_only_inference_tools()
        };
        let mut messages = messages
            .into_iter()
            .map(ToolChatMessage::from)
            .collect::<Vec<_>>();

        for tool_call_index in 0..=MAX_READ_ONLY_TOOL_CALLS {
            let response = client
                .chat_with_tools(
                    &self.config.ollama.models.assistant,
                    messages.clone(),
                    tools.clone(),
                )
                .await?;
            let content = response.message.content.trim();

            if let Some(tool_calls) = response
                .message
                .tool_calls
                .filter(|calls| !calls.is_empty())
            {
                if tool_call_index >= MAX_READ_ONLY_TOOL_CALLS {
                    return Ok(AgentLoopResult {
                        response: GatewayMessageResponse::text(
                            "I could not finish that with the available read-only lookups. Please narrow the question or include a document reference."
                                .to_string(),
                        ),
                        metadata,
                    });
                }

                messages.push(ToolChatMessage::assistant_with_tool_calls(
                    content.to_string(),
                    tool_calls.clone(),
                ));

                for native_tool_call in tool_calls {
                    let tool_call = ReadOnlyToolCall::try_from(&native_tool_call)?;
                    metadata.note_tool_call(&tool_call.name, &tool_call.args);
                    if is_mutating_prepare_tool(&tool_call.name) {
                        if !allow_mutating_tools {
                            return Ok(AgentLoopResult {
                                response: GatewayMessageResponse::text(
                                    "That action is not available in this command flow.",
                                ),
                                metadata,
                            });
                        }

                        tracing::info!(
                            tool = tool_call.name,
                            iteration = tool_call_index,
                            "Mutating confirmation tool requested"
                        );
                        let response = self
                            .prepare_mutating_confirmation(workspace_id, source, &tool_call)
                            .await?;
                        return Ok(AgentLoopResult { response, metadata });
                    }

                    tracing::info!(
                        tool = tool_call.name,
                        iteration = tool_call_index,
                        "Read-only tool requested"
                    );
                    let tool_result =
                        execute_read_only_tool(&self.pool, Some(&self.skills_registry), &tool_call)
                            .await;
                    metadata.note_tool_result(&tool_call.name, &tool_result);
                    tracing::info!(
                        tool = tool_call.name,
                        iteration = tool_call_index,
                        ok = tool_result
                            .get("ok")
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false),
                        "Read-only tool executed"
                    );
                    messages.push(ToolChatMessage::tool_result(
                        tool_call.name,
                        tool_result.to_string(),
                    ));
                }

                continue;
            }

            if !content.is_empty() {
                tracing::info!("Agent loop final answer");
                return Ok(AgentLoopResult {
                    response: GatewayMessageResponse::text(content.to_string()),
                    metadata,
                });
            }

            tracing::warn!(
                role = response.message.role,
                preview = %content_preview(content),
                "Agent loop returned empty response without tool calls"
            );
            return Ok(AgentLoopResult {
                response: GatewayMessageResponse::text(
                    "I could not process that reliably. Ask about document status, pending reviews, export-ready documents, or include a document reference."
                        .to_string(),
                ),
                metadata,
            });
        }

        Ok(AgentLoopResult {
            response: GatewayMessageResponse::text(
                "I could not finish that with the available read-only lookups. Please narrow the question or include a document reference."
                    .to_string(),
            ),
            metadata,
        })
    }

    async fn prepare_mutating_confirmation(
        &self,
        workspace_id: Uuid,
        source: &MessageSource,
        tool_call: &ReadOnlyToolCall,
    ) -> anyhow::Result<GatewayMessageResponse> {
        match tool_call.name.as_str() {
            "prepare_open_review" => {
                let short_ref = required_tool_short_ref(&tool_call.args)?;
                let Some(document) = document_ref_by_short_ref(&self.pool, &short_ref).await?
                else {
                    return Ok(GatewayMessageResponse::text(format!(
                        "Document {} was not found for this company.",
                        short_ref
                    )));
                };
                let confirmation = self
                    .store_confirmation(
                        workspace_id,
                        source,
                        Some(document.id),
                        AgentConfirmationActionKind::OpenReview,
                        json!({ "short_ref": short_ref }),
                    )
                    .await?;
                Ok(confirmation_response(
                    confirmation.id,
                    format!("Confirm opening review actions for {}?", document.short_ref),
                ))
            }
            "prepare_retry_document" => {
                let short_ref = required_tool_short_ref(&tool_call.args)?;
                let Some(document) = document_ref_by_short_ref(&self.pool, &short_ref).await?
                else {
                    return Ok(GatewayMessageResponse::text(format!(
                        "Document {} was not found for this company.",
                        short_ref
                    )));
                };
                let confirmation = self
                    .store_confirmation(
                        workspace_id,
                        source,
                        Some(document.id),
                        AgentConfirmationActionKind::RetryDocument,
                        json!({ "short_ref": short_ref }),
                    )
                    .await?;
                Ok(confirmation_response(
                    confirmation.id,
                    format!(
                        "Confirm retrying {} from the beginning? This will reprocess the document.",
                        document.short_ref
                    ),
                ))
            }
            "prepare_export_documents" => {
                let payload = export_payload_from_tool_args(&tool_call.args)?;
                let document_id = if let Some(short_ref) =
                    payload.get("short_ref").and_then(|v| v.as_str())
                {
                    let Some(document) = document_ref_by_short_ref(&self.pool, short_ref).await?
                    else {
                        return Ok(GatewayMessageResponse::text(format!(
                            "Document {} was not found for this company.",
                            short_ref
                        )));
                    };
                    Some(document.id)
                } else {
                    None
                };
                let confirmation = self
                    .store_confirmation(
                        workspace_id,
                        source,
                        document_id,
                        AgentConfirmationActionKind::ExportDocuments,
                        payload.clone(),
                    )
                    .await?;
                let message =
                    if let Some(short_ref) = payload.get("short_ref").and_then(|v| v.as_str()) {
                        format!("Confirm exporting {} if it is ready?", short_ref)
                    } else {
                        "Confirm exporting all currently ready documents?".to_string()
                    };
                Ok(confirmation_response(confirmation.id, message))
            }
            other => Ok(GatewayMessageResponse::text(format!(
                "I cannot prepare that action yet: {}.",
                other
            ))),
        }
    }

    async fn store_confirmation(
        &self,
        workspace_id: Uuid,
        source: &MessageSource,
        document_id: Option<i64>,
        action_kind: AgentConfirmationActionKind,
        payload: serde_json::Value,
    ) -> anyhow::Result<AgentConfirmation> {
        create_agent_confirmation(
            &self.ephemeral_store,
            NewAgentConfirmation {
                workspace_id,
                document_id,
                channel_type: source.channel.as_str().to_string(),
                channel_identifier: source.channel_identifier.clone(),
                profile_identifier: source.profile_identifier.clone(),
                action_kind,
                payload,
            },
        )
        .await
        .map_err(anyhow::Error::from)
    }

    async fn build_workspace_identity_snapshot(&self) -> anyhow::Result<WorkspaceIdentitySnapshot> {
        let identity = workspace_identity(&self.pool).await?;
        Ok(WorkspaceIdentitySnapshot {
            workspace_name: identity
                .as_ref()
                .and_then(|identity| identity.workspace_name.clone()),
            jurisdiction: identity.and_then(|identity| identity.jurisdiction),
        })
    }

    async fn load_short_term_memory(
        &self,
        session_key: &str,
        source: &MessageSource,
    ) -> (Option<AgentSession>, Option<ConversationContext>) {
        let workspace_id = crate::workspace::active_workspace_id();
        let session = match load_or_create_session(&self.pool, session_key, source).await {
            Ok(session) => session,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    workspace_id = %workspace_id,
                    session_key = session_key,
                    "Agent short-term memory unavailable"
                );
                return (None, None);
            }
        };

        let context =
            match load_recent_context(&self.pool, session.id, DEFAULT_RECENT_TURN_LIMIT).await {
                Ok(context) if !context.messages.is_empty() => Some(context),
                Ok(_) => None,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        workspace_id = %workspace_id,
                        session_key = session_key,
                        "Failed to load recent Agent session context"
                    );
                    None
                }
            };

        (Some(session), context)
    }

    async fn persist_completed_turn(
        &self,
        session: Option<&AgentSession>,
        source: &MessageSource,
        user_text: &str,
        response: &GatewayMessageResponse,
        metadata: AgentTurnMetadata,
    ) {
        let Some(session) = session else {
            return;
        };

        if let Err(err) = append_completed_turn(
            &self.pool,
            session,
            source,
            user_text,
            &response.message,
            metadata,
        )
        .await
        {
            let workspace_id = crate::workspace::active_workspace_id();
            tracing::warn!(
                error = %err,
                workspace_id = %workspace_id,
                session_id = %session.id,
                "Failed to persist Agent short-term memory turn"
            );
        }
    }

    async fn try_acquire_session(&self, session_key: &str) -> bool {
        let mut running = self.running_sessions.lock().await;
        running.insert(session_key.to_string())
    }

    async fn release_session(&self, session_key: &str) {
        let mut running = self.running_sessions.lock().await;
        running.remove(session_key);
    }

    async fn mark_active_document_interaction(&self, session_key: &str) {
        let mut active = self.active_document_interaction_sessions.lock().await;
        active.insert(session_key.to_string());
    }

    async fn clear_active_document_interaction(&self, session_key: &str) {
        let mut active = self.active_document_interaction_sessions.lock().await;
        active.remove(session_key);
    }

    pub async fn handle_document_action(
        &self,
        source: &MessageSource,
        action: DocumentAction,
    ) -> anyhow::Result<GatewayActionResponse> {
        match &action {
            DocumentAction::ConfirmAgentAction { confirmation_id } => {
                return self.confirm_agent_action(source, confirmation_id).await;
            }
            DocumentAction::CancelAgentAction { confirmation_id } => {
                return self.cancel_agent_action(confirmation_id).await;
            }
            _ => {}
        }

        let reviewed_by = source
            .profile_identifier
            .as_deref()
            .unwrap_or(source.channel_identifier.as_str());
        let callback_data = document_action_callback_data(&action);
        let result = process_human_review_callback(
            &self.pool,
            &self.queue_producer,
            &callback_data,
            reviewed_by,
        )
        .await?;

        if result.callback_text == "Edit recorded"
            && let DocumentAction::EditField { document_id, field } = &action
        {
            start_document_interaction(
                &self.pool,
                *document_id,
                source.channel.as_str(),
                &source.channel_identifier,
                source.profile_identifier.as_deref(),
                (*field).into(),
            )
            .await?;
            self.mark_active_document_interaction(&build_session_key(source))
                .await;
        } else if matches!(
            result.callback_text.as_str(),
            "Document approved" | "Document rejected" | "Accounting re-queued"
        ) {
            clear_document_interactions(&self.pool, action.document_id()).await?;
            self.clear_active_document_interaction(&build_session_key(source))
                .await;
        }

        Ok(to_gateway_response(result))
    }

    async fn confirm_agent_action(
        &self,
        source: &MessageSource,
        confirmation_id: &str,
    ) -> anyhow::Result<GatewayActionResponse> {
        let Some(confirmation) =
            consume_agent_confirmation(&self.ephemeral_store, confirmation_id).await?
        else {
            return Ok(GatewayActionResponse::text(
                "Confirmation expired. Please ask me to start that action again.",
                "Confirmation expired",
            ));
        };

        if !confirmation_matches_source(&confirmation, source) {
            return Ok(GatewayActionResponse::text(
                "That confirmation belongs to a different chat or user.",
                "Not allowed",
            ));
        }

        let response = match confirmation.action_kind {
            AgentConfirmationActionKind::OpenReview => {
                let short_ref = confirmation_short_ref(&confirmation)?;
                reopen_review_actions(&self.pool, &short_ref).await?
            }
            AgentConfirmationActionKind::RetryDocument => {
                let short_ref = confirmation_short_ref(&confirmation)?;
                GatewayMessageResponse::text(retry_document(self, source, &short_ref).await?)
            }
            AgentConfirmationActionKind::ExportDocuments => {
                let args = confirmation_export_args(&confirmation);
                export_documents(self, source, confirmation.workspace_id, &args).await?
            }
        };

        Ok(GatewayActionResponse::from_message_response(
            "Confirmed",
            response,
        ))
    }

    async fn cancel_agent_action(
        &self,
        confirmation_id: &str,
    ) -> anyhow::Result<GatewayActionResponse> {
        let _ = cancel_agent_confirmation(&self.ephemeral_store, confirmation_id).await?;
        Ok(GatewayActionResponse::text("Cancelled.", "Cancelled"))
    }
}

pub fn build_session_key(source: &MessageSource) -> String {
    let platform = source.channel.as_str().to_ascii_lowercase();
    let chat_type = source
        .metadata
        .get("chat_type")
        .and_then(|value| value.as_str())
        .unwrap_or("chat");

    format!(
        "agent:main:{platform}:{chat_type}:{}",
        source.channel_identifier
    )
}

fn confirmation_response(
    confirmation_id: String,
    message: impl Into<String>,
) -> GatewayMessageResponse {
    GatewayMessageResponse {
        message: message.into(),
        format: crate::messaging::contracts::GatewayMessageFormat::Markdown,
        attachments: Vec::new(),
        buttons: Some(vec![vec![
            ActionButton {
                label: "Confirm".to_string(),
                action: DocumentAction::ConfirmAgentAction {
                    confirmation_id: confirmation_id.clone(),
                },
            },
            ActionButton {
                label: "Cancel".to_string(),
                action: DocumentAction::CancelAgentAction { confirmation_id },
            },
        ]]),
    }
}

fn confirmation_matches_source(confirmation: &AgentConfirmation, source: &MessageSource) -> bool {
    confirmation
        .channel_type
        .eq_ignore_ascii_case(source.channel.as_str())
        && confirmation.channel_identifier == source.channel_identifier
        && match (
            confirmation.profile_identifier.as_deref(),
            source.profile_identifier.as_deref(),
        ) {
            (Some(expected), Some(actual)) => expected == actual,
            (Some(_), None) => false,
            _ => true,
        }
        && confirmation.expires_at >= chrono::Utc::now().timestamp()
}

fn required_tool_short_ref(args: &serde_json::Value) -> anyhow::Result<String> {
    let raw = args
        .get("short_ref")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required short_ref"))?;
    normalize_short_ref(raw).ok_or_else(|| anyhow::anyhow!("invalid short_ref: {raw}"))
}

fn export_payload_from_tool_args(args: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let mut payload = serde_json::Map::new();
    if let Some(raw) = args.get("short_ref").and_then(|value| value.as_str()) {
        payload.insert(
            "short_ref".to_string(),
            json!(
                normalize_short_ref(raw)
                    .ok_or_else(|| anyhow::anyhow!("invalid short_ref: {raw}"))?
            ),
        );
    }
    if let Some(value) = args.get("date_from").and_then(|value| value.as_str()) {
        payload.insert("date_from".to_string(), json!(value));
    }
    if let Some(value) = args.get("date_to").and_then(|value| value.as_str()) {
        payload.insert("date_to".to_string(), json!(value));
    }
    if let Some(value) = args
        .get("document_types")
        .and_then(|value| value.as_array())
    {
        let document_types = value
            .iter()
            .filter_map(|value| value.as_str())
            .map(|value| value.to_ascii_uppercase())
            .collect::<Vec<_>>();
        if !document_types.is_empty() {
            payload.insert("document_types".to_string(), json!(document_types));
        }
    }
    if let Some(value) = args.get("confidence_min").and_then(|value| value.as_f64()) {
        payload.insert("confidence_min".to_string(), json!(value));
    }

    Ok(serde_json::Value::Object(payload))
}

fn confirmation_short_ref(confirmation: &AgentConfirmation) -> anyhow::Result<String> {
    confirmation
        .payload
        .get("short_ref")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("confirmation missing short_ref"))
}

fn confirmation_export_args(confirmation: &AgentConfirmation) -> GatewayIntentArgs {
    GatewayIntentArgs {
        short_ref: confirmation
            .payload
            .get("short_ref")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
        date_from: confirmation
            .payload
            .get("date_from")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
        date_to: confirmation
            .payload
            .get("date_to")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
        document_types: confirmation
            .payload
            .get("document_types")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str())
                    .map(ToString::to_string)
                    .collect()
            }),
        confidence_min: confirmation
            .payload
            .get("confidence_min")
            .and_then(|value| value.as_f64()),
    }
}

fn slash_route(resolution: &GatewayIntentResolution) -> SlashRoute {
    match resolution.intent {
        GatewayIntentKind::Help => SlashRoute::Help,
        GatewayIntentKind::Status => {
            if let Some(short_ref) = resolution.args.short_ref.as_deref() {
                SlashRoute::ReadOnlyTool(ReadOnlyToolCall {
                    name: "get_document".to_string(),
                    args: json!({ "short_ref": short_ref }),
                })
            } else {
                SlashRoute::ReadOnlyTool(ReadOnlyToolCall {
                    name: "document_status_summary".to_string(),
                    args: json!({}),
                })
            }
        }
        GatewayIntentKind::Documents => SlashRoute::ReadOnlyTool(ReadOnlyToolCall {
            name: "list_documents".to_string(),
            args: json!({ "limit": 10 }),
        }),
        GatewayIntentKind::Pending => SlashRoute::ReadOnlyTool(ReadOnlyToolCall {
            name: "list_pending_reviews".to_string(),
            args: json!({ "limit": 10 }),
        }),
        GatewayIntentKind::Ready => SlashRoute::ReadOnlyTool(ReadOnlyToolCall {
            name: "list_export_ready".to_string(),
            args: json!({ "limit": 10 }),
        }),
        GatewayIntentKind::Last => SlashRoute::ReadOnlyTool(ReadOnlyToolCall {
            name: "list_documents".to_string(),
            args: json!({ "limit": 1 }),
        }),
        GatewayIntentKind::Why if resolution.args.short_ref.is_some() => {
            let short_ref = resolution.args.short_ref.as_deref().unwrap_or_default();
            SlashRoute::ReadOnlyTool(ReadOnlyToolCall {
                name: "explain_document".to_string(),
                args: json!({ "short_ref": short_ref }),
            })
        }
        GatewayIntentKind::Unknown | GatewayIntentKind::GeneralAccountingChat => SlashRoute::Help,
        GatewayIntentKind::Why
        | GatewayIntentKind::Review
        | GatewayIntentKind::Retry
        | GatewayIntentKind::Export
        | GatewayIntentKind::UploadInstruction
        | GatewayIntentKind::OutOfScope => SlashRoute::DeterministicAction,
    }
}

impl DocumentAction {
    pub fn document_id(&self) -> i64 {
        match self {
            DocumentAction::Approve { document_id }
            | DocumentAction::Reject { document_id }
            | DocumentAction::EditField { document_id, .. }
            | DocumentAction::RerunAccounting { document_id }
            | DocumentAction::Retry { document_id }
            | DocumentAction::ReopenReview { document_id } => *document_id,
            DocumentAction::Export {
                document_id: Some(document_id),
            } => *document_id,
            DocumentAction::Export { document_id: None }
            | DocumentAction::ConfirmAgentAction { .. }
            | DocumentAction::CancelAgentAction { .. } => 0,
        }
    }
}

impl From<ReviewFieldAction> for ReviewField {
    fn from(value: ReviewFieldAction) -> Self {
        match value {
            ReviewFieldAction::Supplier => ReviewField::Supplier,
            ReviewFieldAction::Amount => ReviewField::Amount,
            ReviewFieldAction::Date => ReviewField::Date,
            ReviewFieldAction::Kontonummer => ReviewField::Kontonummer,
            ReviewFieldAction::Vat => ReviewField::Vat,
        }
    }
}

impl From<ReviewField> for ReviewFieldAction {
    fn from(value: ReviewField) -> Self {
        match value {
            ReviewField::Supplier => ReviewFieldAction::Supplier,
            ReviewField::Amount => ReviewFieldAction::Amount,
            ReviewField::Date => ReviewFieldAction::Date,
            ReviewField::Kontonummer => ReviewFieldAction::Kontonummer,
            ReviewField::Vat => ReviewFieldAction::Vat,
        }
    }
}

fn document_action_callback_data(action: &DocumentAction) -> String {
    match action {
        DocumentAction::Approve { document_id } => format!("approve:{document_id}"),
        DocumentAction::Reject { document_id } => format!("reject:{document_id}"),
        DocumentAction::EditField { document_id, field } => {
            let field: ReviewField = (*field).into();
            format!("edit:{}:{document_id}", field.as_str())
        }
        DocumentAction::RerunAccounting { document_id } => {
            format!("rerun_accounting:{document_id}")
        }
        DocumentAction::Retry { document_id } => format!("retry:{document_id}"),
        DocumentAction::ReopenReview { document_id } => format!("review:{document_id}"),
        DocumentAction::Export { document_id } => document_id
            .as_ref()
            .map(|document_id| format!("export:{document_id}"))
            .unwrap_or_else(|| "export".to_string()),
        DocumentAction::ConfirmAgentAction { confirmation_id } => {
            format!("confirm_agent:{confirmation_id}")
        }
        DocumentAction::CancelAgentAction { confirmation_id } => {
            format!("cancel_agent:{confirmation_id}")
        }
    }
}

fn to_gateway_response(result: HumanReviewCallbackResult) -> GatewayActionResponse {
    GatewayActionResponse {
        acknowledgement: result.callback_text,
        message: result.chat_message,
        format: crate::messaging::contracts::GatewayMessageFormat::Markdown,
        attachments: Vec::new(),
        buttons: result.keyboard.map(to_action_buttons),
    }
}

fn to_action_buttons(keyboard: HumanReviewKeyboard) -> Vec<Vec<ActionButton>> {
    keyboard
        .rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .filter_map(|button| {
                    parse_document_action(&button.callback_data)
                        .ok()
                        .map(|action| ActionButton {
                            label: button.label,
                            action,
                        })
                })
                .collect()
        })
        .collect()
}

pub fn parse_document_action(callback_data: &str) -> anyhow::Result<DocumentAction> {
    let parts: Vec<&str> = callback_data.split(':').collect();
    if parts.len() < 2 {
        anyhow::bail!("invalid callback data");
    }

    match parts[0] {
        "approve" => Ok(DocumentAction::Approve {
            document_id: parts[1].parse::<i64>()?,
        }),
        "reject" => Ok(DocumentAction::Reject {
            document_id: parts[1].parse::<i64>()?,
        }),
        "rerun_accounting" => Ok(DocumentAction::RerunAccounting {
            document_id: parts[1].parse::<i64>()?,
        }),
        "edit" if parts.len() >= 3 => Ok(DocumentAction::EditField {
            document_id: parts[2].parse::<i64>()?,
            field: parts[1].parse::<ReviewField>()?.into(),
        }),
        "confirm_agent" => Ok(DocumentAction::ConfirmAgentAction {
            confirmation_id: parts[1].to_string(),
        }),
        "cancel_agent" => Ok(DocumentAction::CancelAgentAction {
            confirmation_id: parts[1].to_string(),
        }),
        _ => anyhow::bail!("unknown document action"),
    }
}

pub fn encode_document_action(action: DocumentAction) -> String {
    document_action_callback_data(&action)
}

fn content_preview(content: &str) -> String {
    content.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::db::ChannelType;
    use crate::messaging::contracts::ReviewFieldAction;

    #[test]
    fn document_action_encoding_round_trips_review_actions() {
        let document_id = 7_i64;
        let actions = vec![
            DocumentAction::Approve { document_id },
            DocumentAction::Reject { document_id },
            DocumentAction::RerunAccounting { document_id },
            DocumentAction::EditField {
                document_id,
                field: ReviewFieldAction::Amount,
            },
            DocumentAction::ConfirmAgentAction {
                confirmation_id: "abc123".to_string(),
            },
            DocumentAction::CancelAgentAction {
                confirmation_id: "abc123".to_string(),
            },
        ];

        for action in actions {
            let encoded = encode_document_action(action.clone());
            let decoded = parse_document_action(&encoded).unwrap();
            assert_eq!(decoded, action);
        }
    }

    #[test]
    fn confirmation_source_matching_requires_channel_and_profile() {
        let workspace_id = Uuid::new_v4();
        let source = MessageSource {
            channel: ChannelType::Telegram,
            channel_identifier: "12345".to_string(),
            profile_identifier: Some("67890".to_string()),
            message_id: Some("44".to_string()),
            source_timestamp: None,
            metadata: json!({ "chat_type": "private" }),
        };
        let confirmation = AgentConfirmation {
            id: "abc".to_string(),
            workspace_id,
            document_id: None,
            channel_type: "TELEGRAM".to_string(),
            channel_identifier: "12345".to_string(),
            profile_identifier: Some("67890".to_string()),
            action_kind: AgentConfirmationActionKind::ExportDocuments,
            payload: json!({}),
            expires_at: chrono::Utc::now().timestamp() + 60,
        };

        assert!(confirmation_matches_source(&confirmation, &source));

        let mut wrong_profile = confirmation.clone();
        wrong_profile.profile_identifier = Some("other".to_string());
        assert!(!confirmation_matches_source(&wrong_profile, &source));
    }

    #[test]
    fn gateway_core_has_no_channel_adapter_dependency() {
        let source = include_str!("gateway.rs");
        let forbidden_a = ["tel", "oxide"].concat();
        let forbidden_b = ["Callback", "Query"].concat();
        let forbidden_c = ["Inline", "Keyboard", "Markup"].concat();
        let forbidden_d = ["Input", "File"].concat();

        assert!(!source.contains(&forbidden_a));
        assert!(!source.contains(&forbidden_b));
        assert!(!source.contains(&forbidden_c));
        assert!(!source.contains(&forbidden_d));
    }

    #[test]
    fn session_key_uses_channel_context() {
        let source = MessageSource {
            channel: ChannelType::Telegram,
            channel_identifier: "12345".to_string(),
            profile_identifier: Some("67890".to_string()),
            message_id: Some("44".to_string()),
            source_timestamp: None,
            metadata: json!({ "chat_type": "private" }),
        };

        assert_eq!(
            build_session_key(&source),
            "agent:main:telegram:private:12345"
        );
    }

    #[test]
    fn read_only_slash_commands_route_to_forced_tools() {
        let status = parse_slash_intent("/status").unwrap();
        match slash_route(&status) {
            SlashRoute::ReadOnlyTool(tool_call) => {
                assert_eq!(tool_call.name, "document_status_summary");
                assert_eq!(tool_call.args, json!({}));
            }
            _ => panic!("status should route to read-only tool"),
        }

        let pending = parse_slash_intent("/pending").unwrap();
        match slash_route(&pending) {
            SlashRoute::ReadOnlyTool(tool_call) => {
                assert_eq!(tool_call.name, "list_pending_reviews");
                assert_eq!(tool_call.args["limit"], 10);
            }
            _ => panic!("pending should route to read-only tool"),
        }

        let why = parse_slash_intent("/why D57").unwrap();
        match slash_route(&why) {
            SlashRoute::ReadOnlyTool(tool_call) => {
                assert_eq!(tool_call.name, "explain_document");
                assert_eq!(tool_call.args["short_ref"], "D000057");
            }
            _ => panic!("why with short_ref should route to read-only tool"),
        }
    }

    #[test]
    fn action_slash_commands_remain_deterministic() {
        let review = parse_slash_intent("/review D57").unwrap();
        assert!(matches!(
            slash_route(&review),
            SlashRoute::DeterministicAction
        ));

        let retry = parse_slash_intent("/retry D57").unwrap();
        assert!(matches!(
            slash_route(&retry),
            SlashRoute::DeterministicAction
        ));

        let export = parse_slash_intent("/export").unwrap();
        assert!(matches!(
            slash_route(&export),
            SlashRoute::DeterministicAction
        ));
    }

    #[test]
    fn gateway_no_longer_runs_natural_language_intent_classifier() {
        let source = include_str!("gateway.rs");
        let classifier_fn = ["classify", "_text", "_intent"].concat();
        let classifier_prompt = ["build", "_intent", "_classifier", "_messages"].concat();
        let parser_fn = ["parse", "_model", "_intent", "_response"].concat();

        assert!(!source.contains(&classifier_fn));
        assert!(!source.contains(&classifier_prompt));
        assert!(!source.contains(&parser_fn));
    }

    #[test]
    fn required_tool_short_ref_normalizes_or_rejects_refs() {
        assert_eq!(
            required_tool_short_ref(&json!({ "short_ref": "57" })).unwrap(),
            "D000057"
        );

        let missing = required_tool_short_ref(&json!({})).expect_err("missing ref");
        assert!(missing.to_string().contains("missing required short_ref"));

        let invalid =
            required_tool_short_ref(&json!({ "short_ref": "ABC" })).expect_err("invalid ref");
        assert!(invalid.to_string().contains("invalid short_ref"));
    }

    #[test]
    fn export_payload_from_tool_args_normalizes_supported_filters() {
        let payload = export_payload_from_tool_args(&json!({
            "short_ref": "57",
            "date_from": "2026-01-01",
            "date_to": "2026-01-31",
            "document_types": ["invoice", 1, "receipt"],
            "confidence_min": 0.8
        }))
        .unwrap();

        assert_eq!(payload["short_ref"], "D000057");
        assert_eq!(payload["date_from"], "2026-01-01");
        assert_eq!(payload["date_to"], "2026-01-31");
        assert_eq!(payload["document_types"], json!(["INVOICE", "RECEIPT"]));
        assert_eq!(payload["confidence_min"], 0.8);
    }

    #[test]
    fn confirmation_export_args_reads_payload_without_normalizing_again() {
        let confirmation = AgentConfirmation {
            id: "confirmation".to_string(),
            workspace_id: Uuid::new_v4(),
            document_id: Some(57),
            channel_type: "telegram".to_string(),
            channel_identifier: "chat".to_string(),
            profile_identifier: None,
            action_kind: AgentConfirmationActionKind::ExportDocuments,
            payload: json!({
                "short_ref": "D000057",
                "date_from": "2026-01-01",
                "date_to": "2026-01-31",
                "document_types": ["INVOICE"],
                "confidence_min": 0.75
            }),
            expires_at: chrono::Utc::now().timestamp() + 60,
        };

        let args = confirmation_export_args(&confirmation);

        assert_eq!(args.short_ref.as_deref(), Some("D000057"));
        assert_eq!(args.date_from.as_deref(), Some("2026-01-01"));
        assert_eq!(args.date_to.as_deref(), Some("2026-01-31"));
        assert_eq!(args.document_types, Some(vec!["INVOICE".to_string()]));
        assert_eq!(args.confidence_min, Some(0.75));
    }
}
