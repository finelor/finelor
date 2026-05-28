use uuid::Uuid;

use crate::config::AppConfig;
use crate::inference::ChatMessage;
use crate::query::{DocumentStatusCounts, DocumentSummary, WorkspaceProfile};

use super::contracts::MessageSource;
use super::conversation::ConversationContext;
use super::tools::MAX_READ_ONLY_TOOL_CALLS;

const DEFAULT_AGENT_SOUL: &str =
    include_str!("../../assets/prompts/_shared/assistant_agent_soul.md");

#[derive(Debug, Clone)]
pub struct PromptAssemblyInput<'a> {
    pub config: &'a AppConfig,
    pub workspace_id: Uuid,
    pub session_key: &'a str,
    pub source: &'a MessageSource,
    pub text: &'a str,
    pub accounting_context: Option<AccountingContextSnapshot>,
    pub conversation_context: Option<ConversationContext>,
}

#[derive(Debug, Clone)]
pub struct AccountingContextSnapshot {
    pub workspace: Option<WorkspaceProfile>,
    pub counts: DocumentStatusCounts,
    pub recent_documents: Vec<DocumentSummary>,
    pub attention_documents: Vec<DocumentSummary>,
    pub export_ready_documents: Vec<DocumentSummary>,
}

pub fn assemble_chat_messages(input: PromptAssemblyInput<'_>) -> Vec<ChatMessage> {
    let mut messages = vec![ChatMessage {
        role: "system".to_string(),
        content: assemble_system_prompt(&input),
    }];

    if let Some(context) = input.conversation_context.as_ref() {
        messages.extend(context.messages.iter().map(|message| ChatMessage {
            role: message.role.clone(),
            content: message.content.clone(),
        }));
    }

    messages.push(ChatMessage {
        role: "user".to_string(),
        content: input.text.to_string(),
    });

    messages
}

fn assemble_system_prompt(input: &PromptAssemblyInput<'_>) -> String {
    let soul = load_agent_soul(input.config);
    let platform = input.source.channel.as_str();
    let profile = input
        .source
        .profile_identifier
        .as_deref()
        .unwrap_or("unknown");

    let accounting_context = match input.accounting_context.as_ref() {
        Some(snapshot) => format_accounting_context(snapshot),
        None => "No workspace accounting snapshot is available in this chat turn. Do not claim access to document status, accounting data, exports, or prior messages unless the user supplied that information directly.".to_string(),
    };
    let conversation_context = input
        .conversation_context
        .as_ref()
        .map(format_conversation_context)
        .unwrap_or_else(|| {
            "No recent session context is available. Resolve references only from the current message, snapshot, or tool results.".to_string()
        });
    format!(
        "{soul}\n\n\
         # Runtime Context\n\
         Workspace ID: {workspace_id}\n\
         Session: {session_key}\n\
         Channel: {platform}\n\
         Channel identifier: {channel_identifier}\n\
         Profile identifier: {profile}\n\n\
         # Workspace Accounting Snapshot\n\
         {accounting_context}\n\n\
         # Recent Session Context\n\
         {conversation_context}\n\n\
         # Runtime Tools\n\
         Read-only Finelor tools are provided by the runtime. Use them when the snapshot is insufficient for an operational document question. \
         Mutating tools only prepare a user confirmation prompt; they do not execute changes. Use prepare tools only when the user explicitly asks to review, retry, reprocess, or export. \
         For ambiguous wording such as whether something can be retried or exported, answer or ask a clarifying question instead of preparing a confirmation. \
         You may make at most {max_tool_calls} read-only tool calls in this turn. \
         Use total_count, not returned_count, when answering count questions. \
         Use concise Markdown for readable chat replies. Prefer short bullets over tables. \
         If the user asks for something outside Finelor, invoices, receipts, accounting workflow, document review, uploads, exports, or workspace accounting data, return an answer refusing briefly and describe supported Finelor work. \
         If the user asks for analytics that no available tool supports, answer that you cannot answer it precisely yet and describe supported operational document queries.\n\n\
         # Current Capability Boundary\n\
         Use recent session context only to resolve conversational references such as \"that one\", \"the first one\", \"those\", \"do it\", or \"why\". \
         Treat the snapshot and read-only tool results as workspace-scoped and point-in-time. Do not invent documents, statuses, amounts, exports, or actions outside the snapshot or tool results. \
         Always use tools or deterministic action paths for current accounting state and mutations; conversation history is not accounting truth. \
         Do not claim to have queried live data beyond the provided snapshot and tools. Do not provide final accounting, tax, or legal advice.",
        workspace_id = input.workspace_id,
        session_key = input.session_key,
        channel_identifier = input.source.channel_identifier,
        max_tool_calls = MAX_READ_ONLY_TOOL_CALLS,
    )
}

fn format_conversation_context(context: &ConversationContext) -> String {
    let mut lines = vec![
        "Recent turns are short-term memory for this channel/session only.".to_string(),
        "Use this context to resolve references, not as current accounting truth.".to_string(),
    ];

    if let Some(short_ref) = context.referents.latest_document_ref.as_deref() {
        lines.push(format!(
            "- Latest document-like referent: {short_ref} (verify current state with tools before answering or acting)."
        ));
    }

    if let Some(topic) = context.referents.latest_list_topic.as_deref() {
        lines.push(format!("- Latest operational topic/list: {topic}."));
    }

    if !context.referents.latest_ordered_refs.is_empty() {
        lines.push(format!(
            "- Latest ordered document refs: {}.",
            context.referents.latest_ordered_refs.join(", ")
        ));
        if let Some(first) = context.referents.latest_ordered_refs.first() {
            lines.push(format!("- \"the first one\" may refer to {first}."));
        }
        lines.push("- \"those\" may refer to the latest ordered refs above.".to_string());
    }

    lines.join("\n")
}

fn format_accounting_context(snapshot: &AccountingContextSnapshot) -> String {
    let workspace_line = snapshot
        .workspace
        .as_ref()
        .map(|workspace| {
            format!(
                "Workspace: {} ({})",
                workspace.display_name,
                workspace
                    .jurisdiction
                    .as_deref()
                    .unwrap_or("unknown jurisdiction")
            )
        })
        .unwrap_or_else(|| "Workspace: unknown display name".to_string());

    let mut lines = vec![
        "Snapshot scope: this workspace only.".to_string(),
        "Snapshot freshness: point-in-time at prompt assembly.".to_string(),
        workspace_line,
        "Status counts:".to_string(),
        format!("- Processing: {}", snapshot.counts.processing_count),
        format!("- Pending review: {}", snapshot.counts.pending_count),
        format!("- Export ready: {}", snapshot.counts.ready_count),
        format!("- Exported: {}", snapshot.counts.exported_count),
        format!("- Failed: {}", snapshot.counts.failed_count),
        String::new(),
        "Recent documents:".to_string(),
    ];
    append_document_lines(
        &mut lines,
        &snapshot.recent_documents,
        "No recent documents.",
    );

    lines.push(String::new());
    lines.push("Needs attention:".to_string());
    append_document_lines(
        &mut lines,
        &snapshot.attention_documents,
        "No documents currently need attention.",
    );

    lines.push(String::new());
    lines.push("Export ready:".to_string());
    append_document_lines(
        &mut lines,
        &snapshot.export_ready_documents,
        "No documents currently ready for export.",
    );

    lines.join("\n")
}

fn append_document_lines(lines: &mut Vec<String>, documents: &[DocumentSummary], empty: &str) {
    if documents.is_empty() {
        lines.push(format!("- {empty}"));
        return;
    }

    lines.extend(documents.iter().map(format_document_snapshot_line));
}

fn format_document_snapshot_line(document: &DocumentSummary) -> String {
    let supplier = document
        .supplier_name
        .as_deref()
        .unwrap_or("Unknown supplier");
    let amount = document.total_amount.as_deref().unwrap_or("unknown amount");
    let date = document.invoice_date.as_deref().unwrap_or("unknown date");
    let mut line = format!(
        "- {} | {} | {} | {} SEK | {}",
        document.short_ref, document.status, supplier, amount, date
    );

    if let Some(reason) = document.review_reason.as_deref() {
        line.push_str(&format!(" | Reason: {reason}"));
    }

    line
}

fn load_agent_soul(config: &AppConfig) -> String {
    let prompt_path = &config.ollama.assistant_soul_prompt_path;
    std::fs::read_to_string(prompt_path)
        .map(|content| content.trim().to_string())
        .unwrap_or_else(|err| {
            tracing::warn!(
                prompt_path = %prompt_path,
                error = %err,
                "Could not read configured communication soul prompt, using bundled fallback"
            );
            DEFAULT_AGENT_SOUL.trim().to_string()
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::config::{
        AppConfig, DatabaseConfig, ExportConfig, LoggingConfig, MessagingConfig, MessagingProvider,
        OllamaConfig, OllamaModels, SessionConfig, SlackConfig, TelegramConfig, UploadConfig,
        WorkerConfig,
    };
    use crate::db::ChannelType;
    use crate::messaging::conversation::{ConversationMessage, ConversationReferents};
    use crate::query::{DocumentStatusCounts, WorkspaceProfile};

    #[test]
    fn prompt_assembly_includes_soul_and_runtime_context() {
        let config = test_config();
        let workspace_id = Uuid::parse_str("37fd57d2-51d9-42a3-9bda-8d01f9ad03e1").unwrap();
        let source = MessageSource {
            channel: ChannelType::Telegram,
            channel_identifier: "12345".to_string(),
            profile_identifier: Some("67890".to_string()),
            message_id: Some("44".to_string()),
            source_timestamp: None,
            metadata: json!({ "chat_type": "private" }),
        };

        let messages = assemble_chat_messages(PromptAssemblyInput {
            config: &config,
            workspace_id,
            session_key: "agent:main:telegram:private:12345",
            source: &source,
            text: "Hello",
            accounting_context: None,
            conversation_context: None,
        });

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("# Runtime Context"));
        assert!(messages[0].content.contains(&workspace_id.to_string()));
        assert!(
            messages[0]
                .content
                .contains("agent:main:telegram:private:12345")
        );
        assert!(
            messages[0]
                .content
                .contains("No workspace accounting snapshot")
        );
        assert!(
            messages[0]
                .content
                .contains("Do not claim access to document status")
        );
        assert!(messages[0].content.contains("Runtime Tools"));
        assert!(messages[0].content.contains("Recent Session Context"));
        assert!(messages[0].content.contains("No recent session context"));
        assert!(
            messages[0]
                .content
                .contains("tools are provided by the runtime")
        );
        assert!(
            messages[0]
                .content
                .contains("only prepare a user confirmation prompt")
        );
        assert!(
            messages[0]
                .content
                .contains("If the user asks for something outside Finelor")
        );
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[1].content, "Hello");
    }

    #[test]
    fn prompt_assembly_includes_company_accounting_snapshot() {
        let config = test_config();
        let workspace_id = Uuid::parse_str("37fd57d2-51d9-42a3-9bda-8d01f9ad03e1").unwrap();
        let source = MessageSource {
            channel: ChannelType::Telegram,
            channel_identifier: "12345".to_string(),
            profile_identifier: Some("67890".to_string()),
            message_id: Some("44".to_string()),
            source_timestamp: None,
            metadata: json!({ "chat_type": "private" }),
        };
        let snapshot = AccountingContextSnapshot {
            workspace: Some(WorkspaceProfile {
                id: 1,
                display_name: "Acme AB".to_string(),
                jurisdiction: Some("SE".to_string()),
            }),
            counts: DocumentStatusCounts {
                processing_count: 2,
                pending_count: 1,
                ready_count: 3,
                exported_count: 4,
                failed_count: 0,
            },
            recent_documents: vec![document_summary("D000057", "EXPORT_READY")],
            attention_documents: vec![document_summary("D000058", "PENDING_HUMAN_REVIEW")],
            export_ready_documents: vec![document_summary("D000057", "EXPORT_READY")],
        };

        let messages = assemble_chat_messages(PromptAssemblyInput {
            config: &config,
            workspace_id,
            session_key: "agent:main:telegram:private:12345",
            source: &source,
            text: "What needs attention?",
            accounting_context: Some(snapshot),
            conversation_context: None,
        });

        let system = &messages[0].content;
        assert!(system.contains("Snapshot scope: this workspace only."));
        assert!(system.contains("Snapshot freshness: point-in-time"));
        assert!(system.contains("Workspace: Acme AB (SE)"));
        assert!(system.contains("- Pending review: 1"));
        assert!(system.contains("D000058 | PENDING_HUMAN_REVIEW"));
        assert!(system.contains("Do not invent documents"));
        assert!(system.contains(
            "Do not claim to have queried live data beyond the provided snapshot and tools"
        ));
        assert!(system.contains("Use total_count, not returned_count"));
    }

    #[test]
    fn prompt_assembly_includes_recent_session_context() {
        let config = test_config();
        let workspace_id = Uuid::parse_str("37fd57d2-51d9-42a3-9bda-8d01f9ad03e1").unwrap();
        let source = MessageSource {
            channel: ChannelType::Telegram,
            channel_identifier: "12345".to_string(),
            profile_identifier: Some("67890".to_string()),
            message_id: Some("44".to_string()),
            source_timestamp: None,
            metadata: json!({ "chat_type": "private" }),
        };
        let conversation_context = ConversationContext {
            messages: vec![
                ConversationMessage {
                    role: "user".to_string(),
                    content: "What needs review?".to_string(),
                    metadata: json!({}),
                },
                ConversationMessage {
                    role: "assistant".to_string(),
                    content: "D000057 and D000061 need review.".to_string(),
                    metadata: json!({}),
                },
            ],
            referents: ConversationReferents {
                latest_document_ref: Some("D000057".to_string()),
                latest_list_topic: Some("pending review documents".to_string()),
                latest_ordered_refs: vec!["D000057".to_string(), "D000061".to_string()],
            },
        };

        let messages = assemble_chat_messages(PromptAssemblyInput {
            config: &config,
            workspace_id,
            session_key: "agent:main:telegram:private:12345",
            source: &source,
            text: "Why the first one?",
            accounting_context: None,
            conversation_context: Some(conversation_context),
        });

        assert_eq!(messages.len(), 4);
        assert!(messages[0].content.contains("Latest ordered document refs"));
        assert!(
            messages[0]
                .content
                .contains("\"the first one\" may refer to D000057")
        );
        assert!(
            messages[0]
                .content
                .contains("conversation history is not accounting truth")
        );
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[2].role, "assistant");
        assert_eq!(messages[3].content, "Why the first one?");
    }

    fn document_summary(short_ref: &str, status: &str) -> DocumentSummary {
        DocumentSummary {
            id: 1,
            short_ref: short_ref.to_string(),
            status: status.to_string(),
            supplier_name: Some("Supplier AB".to_string()),
            invoice_date: Some("2026-05-13".to_string()),
            total_amount: Some("1250.00".to_string()),
            confidence_score: Some(0.92),
            review_reason: Some("Needs VAT check".to_string()),
        }
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

    #[test]
    fn prompt_assembler_has_no_channel_adapter_dependency() {
        let source = include_str!("prompt.rs");
        let forbidden_a = ["tel", "oxide"].concat();
        let forbidden_b = ["Callback", "Query"].concat();
        let forbidden_c = ["Inline", "Keyboard", "Markup"].concat();

        assert!(!source.contains(&forbidden_a));
        assert!(!source.contains(&forbidden_b));
        assert!(!source.contains(&forbidden_c));
    }

    #[test]
    fn soul_loader_uses_configured_path_when_file_exists() {
        let mut config = test_config();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("finelor_soul_{stamp}.md"));
        fs::write(&path, "Custom Soul Prompt").expect("write custom soul");
        config.ollama.assistant_soul_prompt_path = path.to_string_lossy().to_string();

        let soul = load_agent_soul(&config);
        assert_eq!(soul, "Custom Soul Prompt");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn soul_loader_falls_back_to_default_when_configured_file_missing() {
        let mut config = test_config();
        config.ollama.assistant_soul_prompt_path =
            "/tmp/finelor_missing_soul_prompt.md".to_string();

        let soul = load_agent_soul(&config);
        assert_eq!(soul, DEFAULT_AGENT_SOUL.trim());
    }
}
