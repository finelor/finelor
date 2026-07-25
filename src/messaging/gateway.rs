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
use crate::query::{
    accounting_processing_candidates_by_ids, document_ref_by_short_ref, workspace_identity,
};
use crate::queue::QueueProducer;
use crate::skills::SkillRegistry;
use crate::web::events::AppEventBus;

use super::commands::{
    AccountingProcessingSelection, build_help_message, describe_accounting_processing_execution,
    describe_accounting_processing_preview, execute_accounting_processing_request,
    export_documents, preview_accounting_processing_selection, reopen_review_actions,
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

enum SlashRoute {
    Help,
}

struct AgentLoopResult {
    response: GatewayMessageResponse,
    metadata: AgentTurnMetadata,
}

struct PrepareToolExecution {
    tool_result: serde_json::Value,
    response: GatewayMessageResponse,
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
            tracing::info!(
                intent = resolution.intent.as_str(),
                "Help slash command routed"
            );
            return match slash_route(&resolution) {
                SlashRoute::Help => Ok(GatewayMessageResponse::text(build_help_message())),
            };
        }

        self.run_agent_chat(workspace_id, session_key, source, text)
            .await
    }

    async fn run_agent_chat(
        &self,
        workspace_id: Uuid,
        session_key: &str,
        source: &MessageSource,
        text: &str,
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
        let messages = assemble_chat_messages(PromptAssemblyInput {
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

        let allow_mutating_tools = true;
        let metadata = AgentTurnMetadata::from_user_text(text);

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

                        // Prepare tools are a different class from read-only reasoning tools.
                        // They resolve action scope, persist a confirmation boundary, and return
                        // a deterministic confirmation response immediately instead of routing
                        // their payload back through the model for another reasoning pass.
                        tracing::info!(
                            tool = tool_call.name,
                            iteration = tool_call_index,
                            "Mutating confirmation tool requested"
                        );
                        let execution = self
                            .prepare_mutating_confirmation(workspace_id, source, &tool_call)
                            .await?;
                        metadata.note_tool_result(&tool_call.name, &execution.tool_result);
                        return Ok(AgentLoopResult {
                            response: execution.response,
                            metadata,
                        });
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
    ) -> anyhow::Result<PrepareToolExecution> {
        match tool_call.name.as_str() {
            "prepare_open_review" => {
                let short_ref = required_tool_document_short_ref(&tool_call.args)?;
                let Some(document) = document_ref_by_short_ref(&self.pool, &short_ref).await?
                else {
                    return Ok(PrepareToolExecution {
                        tool_result: json!({
                            "ok": false,
                            "tool": tool_call.name,
                            "error": format!(
                                "Document {} was not found for this company.",
                                short_ref
                            ),
                        }),
                        response: GatewayMessageResponse::text(format!(
                            "Document {} was not found for this company.",
                            short_ref
                        )),
                    });
                };
                let confirmation = self
                    .store_confirmation(
                        workspace_id,
                        source,
                        Some(document.id),
                        AgentConfirmationActionKind::OpenReview,
                        json!({ "document_short_ref": short_ref }),
                    )
                    .await?;
                let confirmation_id = confirmation.id;
                let message = format!("Confirm opening review actions for {}?", document.short_ref);
                Ok(PrepareToolExecution {
                    tool_result: json!({
                        "ok": true,
                        "tool": tool_call.name,
                        "result": {
                            "message": message,
                            "confirmation_id": confirmation_id,
                            "document_short_ref": document.short_ref,
                            "document_id": document.id,
                        },
                        "result_description": "The result contains a prepared review action, not an executed action. ok indicates whether the preparation succeeded. message is the assistant-facing explanation of what was prepared or why it could not be prepared. If confirmation data is present, confirmation_id identifies the pending confirmation and any returned document reference or document identifiers identify the target document for that prepared review action. If you answer from this result, make it explicit that review has been prepared for confirmation and has not yet been executed.",
                    }),
                    response: confirmation_response(confirmation_id, message),
                })
            }
            "prepare_retry_document" => {
                let short_ref = required_tool_document_short_ref(&tool_call.args)?;
                let Some(document) = document_ref_by_short_ref(&self.pool, &short_ref).await?
                else {
                    return Ok(PrepareToolExecution {
                        tool_result: json!({
                            "ok": false,
                            "tool": tool_call.name,
                            "error": format!(
                                "Document {} was not found for this company.",
                                short_ref
                            ),
                        }),
                        response: GatewayMessageResponse::text(format!(
                            "Document {} was not found for this company.",
                            short_ref
                        )),
                    });
                };
                let confirmation = self
                    .store_confirmation(
                        workspace_id,
                        source,
                        Some(document.id),
                        AgentConfirmationActionKind::RetryDocument,
                        json!({ "document_short_ref": short_ref }),
                    )
                    .await?;
                let confirmation_id = confirmation.id;
                let message = format!(
                    "Confirm retrying {} from the beginning? This will reprocess the document.",
                    document.short_ref
                );
                Ok(PrepareToolExecution {
                    tool_result: json!({
                        "ok": true,
                        "tool": tool_call.name,
                        "result": {
                            "message": message,
                            "confirmation_id": confirmation_id,
                            "document_short_ref": document.short_ref,
                            "document_id": document.id,
                        },
                        "result_description": "The result contains a prepared retry or reprocess action, not an executed action. ok indicates whether the preparation succeeded. message is the assistant-facing explanation of what was prepared or why it could not be prepared. If confirmation data is present, confirmation_id identifies the pending confirmation and any returned document reference or document identifiers identify the target document for that prepared retry action. If you answer from this result, make it explicit that retry has been prepared for confirmation and has not yet been executed.",
                    }),
                    response: confirmation_response(confirmation_id, message),
                })
            }
            "prepare_export_documents" => {
                let payload = export_payload_from_tool_args(&tool_call.args)?;
                let document_id = if let Some(short_ref) =
                    payload.get("document_short_ref").and_then(|v| v.as_str())
                {
                    let Some(document) = document_ref_by_short_ref(&self.pool, short_ref).await?
                    else {
                        return Ok(PrepareToolExecution {
                            tool_result: json!({
                                "ok": false,
                                "tool": tool_call.name,
                                "error": format!(
                                    "Document {} was not found for this company.",
                                    short_ref
                                ),
                            }),
                            response: GatewayMessageResponse::text(format!(
                                "Document {} was not found for this company.",
                                short_ref
                            )),
                        });
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
                let message = if let Some(short_ref) =
                    payload.get("document_short_ref").and_then(|v| v.as_str())
                {
                    format!("Confirm exporting {} if it is ready?", short_ref)
                } else {
                    "Confirm exporting all currently ready documents?".to_string()
                };
                let confirmation_id = confirmation.id;
                Ok(PrepareToolExecution {
                    tool_result: json!({
                        "ok": true,
                        "tool": tool_call.name,
                        "result": {
                            "message": message,
                            "confirmation_id": confirmation_id,
                            "document_id": document_id,
                            "document_short_ref": payload.get("document_short_ref").and_then(|v| v.as_str()),
                            "date_from": payload.get("date_from").and_then(|v| v.as_str()),
                            "date_to": payload.get("date_to").and_then(|v| v.as_str()),
                            "document_types": payload.get("document_types").cloned(),
                            "confidence_min": payload.get("confidence_min").cloned(),
                        },
                        "result_description": "The result contains a prepared export action, not an executed export. ok indicates whether the preparation succeeded. message is the assistant-facing explanation of what export scope was prepared or why it could not be prepared. If confirmation data is present, confirmation_id identifies the pending confirmation and any returned document reference, document identifiers, or filter values identify the export scope that was prepared. If you answer from this result, make it explicit that export has been prepared for confirmation and has not yet been executed.",
                    }),
                    response: confirmation_response(confirmation_id, message),
                })
            }
            "prepare_process_accounting_documents" => {
                let selection = accounting_processing_selection_from_tool_args(&tool_call.args)?;
                let preview =
                    preview_accounting_processing_selection(&self.pool, selection).await?;
                let message = describe_accounting_processing_preview(&preview);
                if preview.eligible_documents.is_empty() {
                    return Ok(PrepareToolExecution {
                        tool_result: json!({
                            "ok": false,
                            "tool": tool_call.name,
                            "error": message,
                        }),
                        response: GatewayMessageResponse::text(message),
                    });
                }

                let payload = json!({
                    "document_ids": preview
                        .eligible_documents
                        .iter()
                        .map(|document| document.id)
                        .collect::<Vec<_>>(),
                    "document_short_refs": preview
                        .eligible_documents
                        .iter()
                        .map(|document| document.short_ref.clone())
                        .collect::<Vec<_>>(),
                });
                let confirmation = self
                    .store_confirmation(
                        workspace_id,
                        source,
                        None,
                        AgentConfirmationActionKind::ProcessAccountingDocuments,
                        payload,
                    )
                    .await?;
                let confirmation_id = confirmation.id;
                let eligible_documents = preview
                    .eligible_documents
                    .iter()
                    .map(|document| {
                        json!({
                            "document_id": document.id,
                            "document_short_ref": document.short_ref,
                            "intake_status": document.intake_status,
                            "accounting_status": document.accounting_status,
                            "accounting_requested_at": document.accounting_requested_at,
                        })
                    })
                    .collect::<Vec<_>>();
                let skipped_documents = preview
                    .skipped
                    .iter()
                    .map(|skip| {
                        json!({
                            "document_short_ref": skip.short_ref,
                            "status": skip.status,
                            "reason": skip.reason,
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(PrepareToolExecution {
                    tool_result: json!({
                        "ok": true,
                        "tool": tool_call.name,
                        "result": {
                            "message": message,
                            "confirmation_id": confirmation_id,
                            "eligible_documents": eligible_documents,
                            "skipped_documents": skipped_documents,
                            "document_ids": preview
                                .eligible_documents
                                .iter()
                                .map(|document| document.id)
                                .collect::<Vec<_>>(),
                            "document_short_refs": preview
                                .eligible_documents
                                .iter()
                                .map(|document| document.short_ref.clone())
                                .collect::<Vec<_>>(),
                        },
                        "result_description": "The result contains a prepared accounting-processing action, not started accounting work. ok indicates whether the preparation succeeded. message is the assistant-facing explanation of what was prepared or why it could not be prepared. If the result includes eligible and skipped documents, eligible documents are the ones that can proceed if confirmed and skipped documents are the ones that will not be processed in this prepared action. If confirmation data is present, confirmation_id identifies the pending confirmation and any returned document references or document identifiers identify the prepared accounting-processing scope. If you answer from this result, make it explicit that accounting processing has been prepared for confirmation and has not yet started.",
                    }),
                    response: confirmation_response(confirmation_id, message),
                })
            }
            other => Ok(PrepareToolExecution {
                tool_result: json!({
                    "ok": false,
                    "tool": tool_call.name,
                    "error": format!("I cannot prepare that action yet: {}.", other),
                }),
                response: GatewayMessageResponse::text(format!(
                    "I cannot prepare that action yet: {}.",
                    other
                )),
            }),
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
                let short_ref = confirmation_document_short_ref(&confirmation)?;
                reopen_review_actions(&self.pool, &short_ref).await?
            }
            AgentConfirmationActionKind::RetryDocument => {
                let short_ref = confirmation_document_short_ref(&confirmation)?;
                GatewayMessageResponse::text(retry_document(self, source, &short_ref).await?)
            }
            AgentConfirmationActionKind::ExportDocuments => {
                let args = confirmation_export_args(&confirmation);
                export_documents(self, source, confirmation.workspace_id, &args).await?
            }
            AgentConfirmationActionKind::ProcessAccountingDocuments => {
                let document_ids = confirmation_document_ids(&confirmation)?;
                let candidates =
                    accounting_processing_candidates_by_ids(&self.pool, &document_ids).await?;
                let eligible = candidates
                    .into_iter()
                    .filter(|candidate| {
                        candidate.intake_status == "INGESTED"
                            && candidate.accounting_status == "NOT_REQUESTED"
                    })
                    .collect::<Vec<_>>();
                if eligible.is_empty() {
                    GatewayMessageResponse::text(
                        "Those documents are no longer eligible for accounting processing.",
                    )
                } else {
                    let execution = execute_accounting_processing_request(
                        &self.pool,
                        &self.queue_producer,
                        &eligible,
                    )
                    .await?;
                    GatewayMessageResponse::text(describe_accounting_processing_execution(
                        &execution,
                    ))
                }
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

fn required_tool_document_short_ref(args: &serde_json::Value) -> anyhow::Result<String> {
    let raw = args
        .get("document_short_ref")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required document_short_ref"))?;
    normalize_short_ref(raw).ok_or_else(|| anyhow::anyhow!("invalid document_short_ref: {raw}"))
}

fn export_payload_from_tool_args(args: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let mut payload = serde_json::Map::new();
    if let Some(raw) = args
        .get("document_short_ref")
        .and_then(|value| value.as_str())
    {
        payload.insert(
            "document_short_ref".to_string(),
            json!(
                normalize_short_ref(raw)
                    .ok_or_else(|| anyhow::anyhow!("invalid document_short_ref: {raw}"))?
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

fn accounting_processing_selection_from_tool_args(
    args: &serde_json::Value,
) -> anyhow::Result<AccountingProcessingSelection> {
    let single = args
        .get("document_short_ref")
        .and_then(|value| value.as_str());
    let multiple = args
        .get("document_short_refs")
        .and_then(|value| value.as_array());
    let all_eligible = args
        .get("all_eligible")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    let selector_count =
        usize::from(single.is_some()) + usize::from(multiple.is_some()) + usize::from(all_eligible);
    if selector_count != 1 {
        return Err(anyhow::anyhow!(
            "provide exactly one of document_short_ref, document_short_refs, or all_eligible"
        ));
    }

    if let Some(raw) = single {
        return normalize_short_ref(raw)
            .map(AccountingProcessingSelection::One)
            .ok_or_else(|| anyhow::anyhow!("invalid document_short_ref: {raw}"));
    }

    if let Some(values) = multiple {
        let mut refs = Vec::new();
        for value in values {
            let raw = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("document_short_refs must contain only strings"))?;
            let normalized = normalize_short_ref(raw)
                .ok_or_else(|| anyhow::anyhow!("invalid document_short_ref: {raw}"))?;
            if !refs.contains(&normalized) {
                refs.push(normalized);
            }
        }
        if refs.is_empty() {
            return Err(anyhow::anyhow!("document_short_refs must not be empty"));
        }
        return Ok(AccountingProcessingSelection::Many(refs));
    }

    Ok(AccountingProcessingSelection::AllEligible)
}

fn confirmation_document_short_ref(confirmation: &AgentConfirmation) -> anyhow::Result<String> {
    confirmation
        .payload
        .get("document_short_ref")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("confirmation missing document_short_ref"))
}

fn confirmation_export_args(confirmation: &AgentConfirmation) -> GatewayIntentArgs {
    GatewayIntentArgs {
        document_short_ref: confirmation
            .payload
            .get("document_short_ref")
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

fn confirmation_document_ids(confirmation: &AgentConfirmation) -> anyhow::Result<Vec<i64>> {
    let ids = confirmation
        .payload
        .get("document_ids")
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow::anyhow!("confirmation missing document_ids"))?
        .iter()
        .map(|value| {
            value
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("confirmation document_ids must be integers"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    if ids.is_empty() {
        return Err(anyhow::anyhow!(
            "confirmation document_ids must not be empty"
        ));
    }

    Ok(ids)
}

fn slash_route(resolution: &GatewayIntentResolution) -> SlashRoute {
    match resolution.intent {
        GatewayIntentKind::Help => SlashRoute::Help,
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
    use crate::config::{
        AppConfig, DatabaseConfig, ExportConfig, LoggingConfig, MessagingConfig, MessagingProvider,
        OllamaConfig, OllamaModels, SessionConfig, SlackConfig, TelegramConfig, UploadConfig,
        WorkerConfig,
    };
    use crate::db::ChannelType;
    use crate::kv::EphemeralStore;
    use crate::messaging::contracts::ReviewFieldAction;
    use crate::queue::QueueProducer;
    use crate::skills::SkillRegistry;
    use crate::web::events::AppEventBus;
    use std::sync::Arc;
    use tokio::sync::Mutex;

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
    fn help_is_the_only_slash_route() {
        let help = parse_slash_intent("/help").unwrap();
        assert!(matches!(slash_route(&help), SlashRoute::Help));
    }

    #[test]
    fn removed_slash_commands_fall_back_to_agent_chat() {
        assert!(parse_slash_intent("/status").is_none());
        assert!(parse_slash_intent("/review D57").is_none());
        assert!(parse_slash_intent("/export").is_none());
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

    fn test_config() -> AppConfig {
        AppConfig {
            ollama: OllamaConfig {
                base_url: "http://localhost:11434".to_string(),
                api_key: None,
                models: OllamaModels {
                    vision: "vision-model".to_string(),
                    accountant: "accountant-model".to_string(),
                    assistant: "agent-chat-model".to_string(),
                },
                vision_prompt_path: "assets/prompts/sweden/vision_agent.md".to_string(),
                accountant_prompt_path: "assets/prompts/sweden/accountant_agent.md".to_string(),
                assistant_soul_prompt_path: "assets/prompts/_shared/assistant_agent_soul.md"
                    .to_string(),
                timeout_seconds: 120,
                max_retries: 3,
                initial_backoff_ms: 500,
                max_backoff_seconds: 10,
            },
            database: DatabaseConfig {
                path: ".finelor/db/finelor_test.db".to_string(),
                max_connections: 10,
            },
            session: SessionConfig {
                secret: "secret".to_string(),
                expiry_days: 30,
                cleanup_interval_seconds: 3600,
                secure: false,
            },
            worker: WorkerConfig { max_job_retries: 3 },
            web: crate::config::WebConfig {
                allowed_hosts: vec![
                    "localhost".to_string(),
                    "127.0.0.1".to_string(),
                    "::1".to_string(),
                    "0.0.0.0".to_string(),
                ],
                allowed_origins: vec![
                    "http://localhost:3000".to_string(),
                    "http://127.0.0.1:3000".to_string(),
                ],
            },
            messaging: MessagingConfig {
                provider: MessagingProvider::Telegram,
                telegram: TelegramConfig {
                    bot_token: "dummy".to_string(),
                    webhook_url: None,
                },
                slack: SlackConfig {
                    bot_token: String::new(),
                    app_token: String::new(),
                },
            },
            upload: UploadConfig {
                storage_path: "./.finelor/uploads".to_string(),
            },
            export: ExportConfig {
                sie4_encoding: "PC8".to_string(),
                download_expiry_hours: 168,
                company_org_nr: "5560000000".to_string(),
                company_name: "Your Company AB".to_string(),
                fiscal_year_start: "20240101".to_string(),
                fiscal_year_end: "20241231".to_string(),
                output_path: "./.finelor/exports".to_string(),
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                format: "json".to_string(),
            },
        }
    }

    fn test_gateway_state(pool: sqlx::SqlitePool) -> AgentGatewayState {
        AgentGatewayState {
            config: Arc::new(test_config()),
            pool,
            queue_producer: QueueProducer::new(Arc::new(crate::queue::InMemoryJobQueue::new())),
            ephemeral_store: EphemeralStore::new(),
            events: AppEventBus::new(16),
            running_sessions: Arc::new(Mutex::new(Default::default())),
            active_document_interaction_sessions: Arc::new(Mutex::new(Default::default())),
            skills_registry: Arc::new(SkillRegistry::new()),
        }
    }

    fn test_source() -> MessageSource {
        MessageSource {
            channel: ChannelType::Telegram,
            channel_identifier: "12345".to_string(),
            profile_identifier: Some("67890".to_string()),
            message_id: Some("44".to_string()),
            source_timestamp: None,
            metadata: json!({ "chat_type": "private" }),
        }
    }

    async fn create_document(
        pool: &sqlx::SqlitePool,
        preset: crate::document_state::TestDocumentStatePreset,
    ) -> (i64, String) {
        let document_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO documents (filename, file_hash, original_path, mime_type)
            VALUES ('doc.pdf', $1, '/tmp/doc.pdf', 'application/pdf')
            RETURNING id
            "#,
        )
        .bind(format!("gateway-{}", uuid::Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .expect("insert document");
        crate::document_state::seed_document_state_preset(pool, document_id, preset)
            .await
            .expect("seed state");
        let short_ref: String = sqlx::query_scalar("SELECT short_ref FROM documents WHERE id = $1")
            .bind(document_id)
            .fetch_one(pool)
            .await
            .expect("fetch short_ref");
        (document_id, short_ref)
    }

    #[tokio::test]
    async fn prepare_open_review_returns_result_description_in_tool_result() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .in_memory(true)
                    .foreign_keys(true),
            )
            .await
            .expect("connect");
        crate::db::run_migrations(&pool).await.expect("migrate");
        let (_, short_ref) = create_document(
            &pool,
            crate::document_state::TestDocumentStatePreset::PendingReview,
        )
        .await;

        let state = test_gateway_state(pool);
        let source = test_source();
        let execution = state
            .prepare_mutating_confirmation(
                crate::workspace::active_workspace_id(),
                &source,
                &ReadOnlyToolCall {
                    name: "prepare_open_review".to_string(),
                    args: json!({ "document_short_ref": short_ref }),
                },
            )
            .await
            .expect("prepare review");

        assert_eq!(execution.tool_result["ok"], true);
        assert!(
            execution.tool_result["result_description"]
                .as_str()
                .expect("result_description")
                .contains("prepared review action")
        );
        assert!(
            execution.tool_result["result"]["confirmation_id"]
                .as_str()
                .is_some()
        );
        assert!(execution.response.buttons.is_some());
    }

    #[tokio::test]
    async fn prepare_process_accounting_documents_returns_result_description_and_preview_fields() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .in_memory(true)
                    .foreign_keys(true),
            )
            .await
            .expect("connect");
        crate::db::run_migrations(&pool).await.expect("migrate");
        let (document_id, short_ref) = create_document(
            &pool,
            crate::document_state::TestDocumentStatePreset::IntakeIngested,
        )
        .await;

        let state = test_gateway_state(pool);
        let source = test_source();
        let execution = state
            .prepare_mutating_confirmation(
                crate::workspace::active_workspace_id(),
                &source,
                &ReadOnlyToolCall {
                    name: "prepare_process_accounting_documents".to_string(),
                    args: json!({ "document_short_ref": short_ref }),
                },
            )
            .await
            .expect("prepare accounting processing");

        assert_eq!(execution.tool_result["ok"], true);
        assert!(
            execution.tool_result["result_description"]
                .as_str()
                .expect("result_description")
                .contains("prepared accounting-processing action")
        );
        assert_eq!(
            execution.tool_result["result"]["document_ids"][0],
            json!(document_id)
        );
        assert_eq!(
            execution.tool_result["result"]["eligible_documents"][0]["document_short_ref"],
            json!(short_ref)
        );
        assert!(execution.response.buttons.is_some());
    }

    #[test]
    fn required_tool_document_short_ref_normalizes_or_rejects_refs() {
        assert_eq!(
            required_tool_document_short_ref(&json!({ "document_short_ref": "57" })).unwrap(),
            "D000057"
        );

        let missing = required_tool_document_short_ref(&json!({})).expect_err("missing ref");
        assert!(
            missing
                .to_string()
                .contains("missing required document_short_ref")
        );

        let invalid = required_tool_document_short_ref(&json!({ "document_short_ref": "ABC" }))
            .expect_err("invalid ref");
        assert!(invalid.to_string().contains("invalid document_short_ref"));
    }

    #[test]
    fn export_payload_from_tool_args_normalizes_supported_filters() {
        let payload = export_payload_from_tool_args(&json!({
            "document_short_ref": "57",
            "date_from": "2026-01-01",
            "date_to": "2026-01-31",
            "document_types": ["invoice", 1, "receipt"],
            "confidence_min": 0.8
        }))
        .unwrap();

        assert_eq!(payload["document_short_ref"], "D000057");
        assert_eq!(payload["date_from"], "2026-01-01");
        assert_eq!(payload["date_to"], "2026-01-31");
        assert_eq!(payload["document_types"], json!(["INVOICE", "RECEIPT"]));
        assert_eq!(payload["confidence_min"], 0.8);
    }

    #[test]
    fn accounting_processing_selection_from_tool_args_requires_exactly_one_selector() {
        let err = accounting_processing_selection_from_tool_args(&json!({}))
            .expect_err("missing selector should fail");
        assert!(err.to_string().contains("exactly one"));

        let err = accounting_processing_selection_from_tool_args(&json!({
            "document_short_ref": "D57",
            "all_eligible": true
        }))
        .expect_err("multiple selectors should fail");
        assert!(err.to_string().contains("exactly one"));
    }

    #[test]
    fn accounting_processing_selection_from_tool_args_normalizes_refs() {
        let single = accounting_processing_selection_from_tool_args(&json!({
            "document_short_ref": "57"
        }))
        .expect("single selector");
        assert_eq!(
            single,
            AccountingProcessingSelection::One("D000057".to_string())
        );

        let many = accounting_processing_selection_from_tool_args(&json!({
            "document_short_refs": ["57", "D000058", "57"]
        }))
        .expect("many selector");
        assert_eq!(
            many,
            AccountingProcessingSelection::Many(vec!["D000057".to_string(), "D000058".to_string()])
        );
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
                "document_short_ref": "D000057",
                "date_from": "2026-01-01",
                "date_to": "2026-01-31",
                "document_types": ["INVOICE"],
                "confidence_min": 0.75
            }),
            expires_at: chrono::Utc::now().timestamp() + 60,
        };

        let args = confirmation_export_args(&confirmation);

        assert_eq!(args.document_short_ref.as_deref(), Some("D000057"));
        assert_eq!(args.date_from.as_deref(), Some("2026-01-01"));
        assert_eq!(args.date_to.as_deref(), Some("2026-01-31"));
        assert_eq!(args.document_types, Some(vec!["INVOICE".to_string()]));
        assert_eq!(args.confidence_min, Some(0.75));
    }

    #[test]
    fn confirmation_document_ids_reads_integer_payload() {
        let confirmation = AgentConfirmation {
            id: "confirmation".to_string(),
            workspace_id: Uuid::new_v4(),
            document_id: None,
            channel_type: "telegram".to_string(),
            channel_identifier: "chat".to_string(),
            profile_identifier: None,
            action_kind: AgentConfirmationActionKind::ProcessAccountingDocuments,
            payload: json!({
                "document_ids": [7, 8]
            }),
            expires_at: chrono::Utc::now().timestamp() + 60,
        };

        assert_eq!(
            confirmation_document_ids(&confirmation).unwrap(),
            vec![7, 8]
        );
    }
}
