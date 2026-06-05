use std::sync::Arc;

use uuid::Uuid;

use crate::config::AppConfig;
use crate::inference::ChatMessage;
use crate::skills::SkillRegistry;

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
    pub workspace_identity: Option<WorkspaceIdentitySnapshot>,
    pub conversation_context: Option<ConversationContext>,
    pub skills_registry: Option<Arc<SkillRegistry>>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceIdentitySnapshot {
    pub workspace_name: Option<String>,
    pub jurisdiction: Option<String>,
}

pub async fn assemble_chat_messages(input: PromptAssemblyInput<'_>) -> Vec<ChatMessage> {
    let mut messages = vec![ChatMessage {
        role: "system".to_string(),
        content: assemble_system_prompt(&input).await,
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

async fn assemble_system_prompt(input: &PromptAssemblyInput<'_>) -> String {
    let soul = load_agent_soul(input.config);
    let platform = input.source.channel.as_str();

    let workspace_identity = match input.workspace_identity.as_ref() {
        Some(snapshot) => format_workspace_identity(snapshot),
        None => "No workspace identity snapshot is available in this chat turn. Do not claim access to workspace identity, document status, accounting data, exports, or prior messages unless the user supplied that information directly.".to_string(),
    };
    let conversation_context = input
        .conversation_context
        .as_ref()
        .map(format_conversation_context)
        .unwrap_or_else(|| {
            "No recent session context is available. Resolve references only from the current message, snapshot, or tool results.".to_string()
        });

    // Load and format skills context
    let skills_context = if let Some(registry) = input.skills_registry.as_ref() {
        format_skills_context(registry).await
    } else {
        "No skills registry available.".to_string()
    };

    format!(
        "{soul}\n\n\
         # Runtime Context\n\
         Workspace ID: {workspace_id}\n\
         Session: {session_key}\n\
         Channel: {platform}\n\n\
         # Available Skills\n\
         {skills_context}\n\n\
         # Workspace Identity Snapshot\n\
         {workspace_identity}\n\n\
         # Recent Session Context\n\
         {conversation_context}\n\n\
         # Runtime Tools\n\
         Read-only Finelor tools are provided by the runtime. Use them for operational document questions and whenever the identity snapshot is insufficient. \
         If a request clearly matches an available skill, load that skill with skill_view(name) before answering, then use runtime tools as needed for current workspace data. \
         Use skills for workflow, constraints, and response format; use tools for live operational facts. \
         If a loaded skill exposes a relevant reference or template, load only the specific supporting file you need with skill_view(name, path). \
         Skip skill loading only for trivial turns that are fully answerable without workflow guidance. \
         Mutating tools only prepare a user confirmation prompt; they do not execute changes. \
         You may make at most {max_tool_calls} read-only tool calls in this turn. \
         skill_view also uses this read-only tool budget, so load at most one matching skill by default and only fetch supporting files that materially improve the answer. \
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
            "- Latest document reference: {short_ref} (verify current state with tools before answering or acting)."
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

/// Format the skills context for the system prompt.
/// Includes lazy initialization to ensure skills are available even if the
/// registry was not initialized during startup.
async fn format_skills_context(registry: &SkillRegistry) -> String {
    if registry.is_empty().await {
        tracing::debug!(
            "Skills registry is empty at prompt assembly time, attempting lazy initialization"
        );
        if let Err(e) = registry.initialize().await {
            tracing::error!(error = %e, "Failed to lazily initialize skills registry");
        } else {
            tracing::info!("Skills registry lazily initialized successfully");
        }
    }

    build_skills_context(registry).await
}

async fn build_skills_context(registry: &SkillRegistry) -> String {
    let skills = registry.get_all_skills().await;

    if skills.is_empty() {
        return "No skills are currently loaded.".to_string();
    }

    tracing::info!(
        count = skills.len(),
        skill_names = ?skills
            .iter()
            .map(|skill| skill.name().to_string())
            .collect::<Vec<_>>(),
        "Injected skills into assistant system prompt context"
    );

    let mut lines = vec![];
    lines.push("Skills available:".to_string());
    for skill in skills {
        lines.push(format!("- {}: {}", skill.name(), skill.description()));
    }
    lines.push(String::new());
    lines.push("Match skills by request intent, not only by explicit skill name.".to_string());
    lines.push(
        "When a request falls within an available skill, load that skill with skill_view(name) before answering."
            .to_string(),
    );
    lines.push(
        "Use tools after the skill load for live data, and load only the specific reference/template files needed with skill_view(name, path)."
            .to_string(),
    );

    lines.join("\n")
}

fn format_workspace_identity(snapshot: &WorkspaceIdentitySnapshot) -> String {
    let mut lines = vec![
        "Snapshot scope: this workspace only.".to_string(),
        "Snapshot freshness: point-in-time at prompt assembly.".to_string(),
    ];

    if let Some(workspace_name) = snapshot.workspace_name.as_deref() {
        lines.push(format!("Workspace: {workspace_name}"));
    }

    if let Some(jurisdiction) = snapshot.jurisdiction.as_deref() {
        lines.push(format!("Jurisdiction: {jurisdiction}"));
    }

    lines.join("\n")
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
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    use super::*;
    use crate::config::{
        AppConfig, DatabaseConfig, ExportConfig, LoggingConfig, MessagingConfig, MessagingProvider,
        OllamaConfig, OllamaModels, SessionConfig, SlackConfig, TelegramConfig, UploadConfig,
        WorkerConfig,
    };
    use crate::db::ChannelType;
    use crate::messaging::conversation::{ConversationMessage, ConversationReferents};
    use crate::skills::SkillRegistry;

    fn create_temp_skill_root() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn write_skill_fixture(root: &std::path::Path, skill_dir_name: &str, skill_md: &str) {
        let skill_dir = root.join(skill_dir_name);
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(skill_dir.join("SKILL.md"), skill_md).expect("write SKILL.md");
    }

    #[tokio::test]
    async fn prompt_assembly_includes_soul_and_runtime_context() {
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
            workspace_identity: None,
            conversation_context: None,
            skills_registry: None,
        })
        .await;

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
                .contains("No workspace identity snapshot")
        );
        assert!(
            messages[0]
                .content
                .contains("Do not claim access to workspace identity, document status")
        );
        assert!(messages[0].content.contains("# Available Skills"));
        assert!(messages[0].content.contains("No skills registry available"));
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
                .contains("If a request clearly matches an available skill")
        );
        assert!(
            messages[0]
                .content
                .contains("Use skills for workflow, constraints, and response format")
        );
        assert!(
            messages[0]
                .content
                .contains("skill_view also uses this read-only tool budget")
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

    #[tokio::test]
    async fn prompt_assembly_includes_workspace_identity_snapshot() {
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
        let snapshot = WorkspaceIdentitySnapshot {
            workspace_name: Some("Acme AB".to_string()),
            jurisdiction: Some("SE".to_string()),
        };

        let messages = assemble_chat_messages(PromptAssemblyInput {
            config: &config,
            workspace_id,
            session_key: "agent:main:telegram:private:12345",
            source: &source,
            text: "What needs attention?",
            workspace_identity: Some(snapshot),
            conversation_context: None,
            skills_registry: None,
        })
        .await;

        let system = &messages[0].content;
        assert!(system.contains("Snapshot scope: this workspace only."));
        assert!(system.contains("Snapshot freshness: point-in-time"));
        assert!(system.contains("Workspace: Acme AB"));
        assert!(system.contains("Jurisdiction: SE"));
        assert!(!system.contains("Pending review"));
        assert!(!system.contains("Recent documents"));
        assert!(system.contains("Do not invent documents"));
        assert!(system.contains(
            "Do not claim to have queried live data beyond the provided snapshot and tools"
        ));
        assert!(system.contains("Use total_count, not returned_count"));
    }

    #[tokio::test]
    async fn prompt_assembly_includes_recent_session_context() {
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
                latest_skill_name: None,
                latest_skill_supporting_paths: vec![],
                latest_action_topic: None,
            },
        };

        let messages = assemble_chat_messages(PromptAssemblyInput {
            config: &config,
            workspace_id,
            session_key: "agent:main:telegram:private:12345",
            source: &source,
            text: "Why the first one?",
            workspace_identity: None,
            conversation_context: Some(conversation_context),
            skills_registry: None,
        })
        .await;

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

    #[tokio::test]
    async fn prompt_assembly_reports_empty_skills_registry() {
        let config = test_config();
        let source = test_source();
        let root = create_temp_skill_root();
        let registry = Arc::new(SkillRegistry::with_dir(root.path()));
        registry.initialize().await.expect("initialize registry");

        let messages = assemble_chat_messages(PromptAssemblyInput {
            config: &config,
            workspace_id: Uuid::parse_str("37fd57d2-51d9-42a3-9bda-8d01f9ad03e1").unwrap(),
            session_key: "agent:main:telegram:private:12345",
            source: &source,
            text: "What skills do you have?",
            workspace_identity: None,
            conversation_context: None,
            skills_registry: Some(registry),
        })
        .await;

        assert!(
            messages[0]
                .content
                .contains("No skills are currently loaded.")
        );
    }

    #[tokio::test]
    async fn prompt_assembly_lists_loaded_skills() {
        let config = test_config();
        let source = test_source();
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "invoice-helper",
            r#"---
name: Invoice Helper
description: Helps with structured invoice workflows
category: accounting
---
# Invoice Helper

## Overview

Useful overview.
"#,
        );
        let registry = Arc::new(SkillRegistry::with_dir(root.path()));
        registry.initialize().await.expect("initialize registry");

        let messages = assemble_chat_messages(PromptAssemblyInput {
            config: &config,
            workspace_id: Uuid::parse_str("37fd57d2-51d9-42a3-9bda-8d01f9ad03e1").unwrap(),
            session_key: "agent:main:telegram:private:12345",
            source: &source,
            text: "Show me the available skills",
            workspace_identity: None,
            conversation_context: None,
            skills_registry: Some(registry),
        })
        .await;

        let system = &messages[0].content;
        assert!(system.contains("# Available Skills"));
        assert!(system.contains("Invoice Helper"));
        assert!(system.contains("Helps with structured invoice workflows"));
        assert!(system.contains("Match skills by request intent"));
        assert!(system.contains("load that skill with skill_view(name) before answering"));
        assert!(system.contains("load only the specific reference/template files needed"));
        assert!(!system.contains("To use a skill, ask about it by name"));
    }

    #[tokio::test]
    async fn prompt_assembly_lazily_initializes_skills_registry() {
        let config = test_config();
        let source = test_source();
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "review-helper",
            r#"---
name: Review Helper
description: Helps with review workflows
category: operations
---
# Review Helper

## Overview

Useful overview.
"#,
        );
        let registry = Arc::new(SkillRegistry::with_dir(root.path()));

        let messages = assemble_chat_messages(PromptAssemblyInput {
            config: &config,
            workspace_id: Uuid::parse_str("37fd57d2-51d9-42a3-9bda-8d01f9ad03e1").unwrap(),
            session_key: "agent:main:telegram:private:12345",
            source: &source,
            text: "What can you do?",
            workspace_identity: None,
            conversation_context: None,
            skills_registry: Some(registry),
        })
        .await;

        assert!(messages[0].content.contains("Review Helper"));
        assert!(messages[0].content.contains("Helps with review workflows"));
    }

    #[tokio::test]
    async fn prompt_assembly_omits_missing_optional_workspace_identity_fields() {
        let config = test_config();
        let source = test_source();

        let messages = assemble_chat_messages(PromptAssemblyInput {
            config: &config,
            workspace_id: Uuid::parse_str("37fd57d2-51d9-42a3-9bda-8d01f9ad03e1").unwrap(),
            session_key: "agent:main:telegram:private:12345",
            source: &source,
            text: "Who am I working for?",
            workspace_identity: Some(WorkspaceIdentitySnapshot {
                workspace_name: Some("Acme AB".to_string()),
                jurisdiction: None,
            }),
            conversation_context: None,
            skills_registry: None,
        })
        .await;

        let system = &messages[0].content;
        assert!(system.contains("Workspace: Acme AB"));
        assert!(!system.contains("Jurisdiction:"));
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
