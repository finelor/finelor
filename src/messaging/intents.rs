use serde::{Deserialize, Serialize};

use crate::inference::{ChatMessage, extract_json_from_response};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayIntentKind {
    Help,
    Status,
    Documents,
    Pending,
    Ready,
    Last,
    Why,
    Review,
    Retry,
    Export,
    UploadInstruction,
    GeneralAccountingChat,
    OutOfScope,
    Unknown,
}

impl GatewayIntentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Status => "status",
            Self::Documents => "documents",
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Last => "last",
            Self::Why => "why",
            Self::Review => "review",
            Self::Retry => "retry",
            Self::Export => "export",
            Self::UploadInstruction => "upload_instruction",
            Self::GeneralAccountingChat => "general_accounting_chat",
            Self::OutOfScope => "out_of_scope",
            Self::Unknown => "unknown",
        }
    }

    pub fn requires_document_ref(self) -> bool {
        matches!(self, Self::Why | Self::Review | Self::Retry)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GatewayIntentArgs {
    pub document_short_ref: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub document_types: Option<Vec<String>>,
    pub confidence_min: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentResolutionSource {
    SlashCommand,
    Model,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GatewayIntentResolution {
    pub intent: GatewayIntentKind,
    pub confidence: f32,
    pub args: GatewayIntentArgs,
    pub missing_args: Vec<String>,
    pub reason: Option<String>,
    pub source: IntentResolutionSource,
}

#[derive(Debug, Deserialize)]
struct ModelIntentResolution {
    intent: GatewayIntentKind,
    confidence: Option<f32>,
    #[serde(default)]
    args: GatewayIntentArgs,
    #[serde(default)]
    missing_args: Vec<String>,
    reason: Option<String>,
}

pub fn parse_slash_intent(text: &str) -> Option<GatewayIntentResolution> {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let mut parts = trimmed.split_whitespace();
    let command_token = parts.next()?;
    let command = command_token
        .trim_start_matches('/')
        .split('@')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();

    let mut args = GatewayIntentArgs::default();
    let mut missing_args = Vec::new();
    let intent = match command.as_str() {
        "help" => GatewayIntentKind::Help,
        "status" => GatewayIntentKind::Status,
        "documents" => GatewayIntentKind::Documents,
        "pending" => GatewayIntentKind::Pending,
        "ready" => GatewayIntentKind::Ready,
        "last" => GatewayIntentKind::Last,
        "why" => {
            args.document_short_ref = parts.next().and_then(normalize_short_ref);
            if args.document_short_ref.is_none() {
                missing_args.push("document_short_ref".to_string());
            }
            GatewayIntentKind::Why
        }
        "review" => {
            args.document_short_ref = parts.next().and_then(normalize_short_ref);
            if args.document_short_ref.is_none() {
                missing_args.push("document_short_ref".to_string());
            }
            GatewayIntentKind::Review
        }
        "retry" => {
            args.document_short_ref = parts.next().and_then(normalize_short_ref);
            if args.document_short_ref.is_none() {
                missing_args.push("document_short_ref".to_string());
            }
            GatewayIntentKind::Retry
        }
        "export" => {
            args = parse_export_args(trimmed);
            GatewayIntentKind::Export
        }
        _ => GatewayIntentKind::Unknown,
    };

    Some(GatewayIntentResolution {
        intent,
        confidence: 1.0,
        args,
        missing_args,
        reason: None,
        source: IntentResolutionSource::SlashCommand,
    })
}

pub fn parse_model_intent_response(content: &str) -> anyhow::Result<GatewayIntentResolution> {
    let json = extract_json_from_response(content)?;
    let parsed: ModelIntentResolution = serde_json::from_value(json)?;
    let mut missing_args = parsed.missing_args;
    if parsed.intent.requires_document_ref() && parsed.args.document_short_ref.is_none() {
        missing_args.push("document_short_ref".to_string());
        missing_args.sort();
        missing_args.dedup();
    }

    Ok(GatewayIntentResolution {
        intent: parsed.intent,
        confidence: parsed.confidence.unwrap_or(0.0).clamp(0.0, 1.0),
        args: GatewayIntentArgs {
            document_short_ref: parsed
                .args
                .document_short_ref
                .and_then(|value| normalize_short_ref(&value)),
            date_from: parsed.args.date_from,
            date_to: parsed.args.date_to,
            document_types: parsed.args.document_types,
            confidence_min: parsed.args.confidence_min,
        },
        missing_args,
        reason: parsed.reason,
        source: IntentResolutionSource::Model,
    })
}

pub fn build_intent_classifier_messages(text: &str) -> Vec<ChatMessage> {
    let intents = intent_registry()
        .iter()
        .map(|intent| format!("- {}: {}", intent.name, intent.description))
        .collect::<Vec<_>>()
        .join("\n");

    vec![
        ChatMessage {
            role: "system".to_string(),
            content: format!(
                "Classify the user's message for Finelor's accounting agent.\n\
                 Return JSON only with this shape: {{\"intent\":\"...\",\"confidence\":0.0,\"args\":{{\"document_short_ref\":null,\"date_from\":null,\"date_to\":null,\"document_types\":null,\"confidence_min\":null}},\"missing_args\":[],\"reason\":\"...\"}}.\n\
                 Use one of these intents:\n{intents}\n\
                 Rules:\n\
                 - Use out_of_scope for requests unrelated to Finelor, invoices, receipts, accounting workflow, exports, document review, uploads, or company accounting data.\n\
                 - Use general_accounting_chat for in-scope conversational questions that are not executable commands.\n\
                 - Extract document references like D123 as D000123.\n\
                 - If the user asks for the status of a specific document reference, use intent=status and include args.document_short_ref.\n\
                 - For why, review, and retry, include missing_args=[\"document_short_ref\"] if no document reference is present.\n\
                 - Do not answer the user."
            ),
        },
        ChatMessage {
            role: "user".to_string(),
            content: text.to_string(),
        },
    ]
}

#[derive(Debug, Clone, Copy)]
pub struct IntentDefinition {
    pub name: &'static str,
    pub description: &'static str,
}

pub fn intent_registry() -> &'static [IntentDefinition] {
    &[
        IntentDefinition {
            name: "help",
            description: "Show what the agent can do and the available commands.",
        },
        IntentDefinition {
            name: "status",
            description: "Summarize document counts by processing status.",
        },
        IntentDefinition {
            name: "documents",
            description: "List recent company documents.",
        },
        IntentDefinition {
            name: "pending",
            description: "List documents that need attention or failed.",
        },
        IntentDefinition {
            name: "ready",
            description: "List documents ready for export.",
        },
        IntentDefinition {
            name: "last",
            description: "Show the latest document.",
        },
        IntentDefinition {
            name: "why",
            description: "Explain why a specific document is blocked, pending, failed, or ready.",
        },
        IntentDefinition {
            name: "review",
            description: "Open the review actions for a specific document.",
        },
        IntentDefinition {
            name: "retry",
            description: "Reprocess a specific document from the beginning.",
        },
        IntentDefinition {
            name: "export",
            description: "Export ready documents, optionally filtered by reference, dates, type, or confidence.",
        },
        IntentDefinition {
            name: "upload_instruction",
            description: "Tell the user how to upload invoices or receipts.",
        },
        IntentDefinition {
            name: "general_accounting_chat",
            description: "In-scope conversational accounting operations question.",
        },
        IntentDefinition {
            name: "out_of_scope",
            description: "Not related to Finelor or accounting operations.",
        },
        IntentDefinition {
            name: "unknown",
            description: "Could not determine the user's intent.",
        },
    ]
}

pub fn slash_command_menu() -> &'static [IntentDefinition] {
    &[
        IntentDefinition {
            name: "help",
            description: "Show what I can do",
        },
        IntentDefinition {
            name: "status",
            description: "Show document counts by status",
        },
        IntentDefinition {
            name: "documents",
            description: "List recent company documents",
        },
        IntentDefinition {
            name: "pending",
            description: "List documents needing attention",
        },
        IntentDefinition {
            name: "ready",
            description: "List documents ready for export",
        },
        IntentDefinition {
            name: "last",
            description: "Show the latest document",
        },
        IntentDefinition {
            name: "why",
            description: "Explain a document, e.g. /why D000123",
        },
        IntentDefinition {
            name: "review",
            description: "Open review actions, e.g. /review D000123",
        },
        IntentDefinition {
            name: "retry",
            description: "Reprocess a document, e.g. /retry D000123",
        },
        IntentDefinition {
            name: "export",
            description: "Export ready documents",
        },
    ]
}

pub fn normalize_short_ref(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let digits = trimmed.trim_start_matches(['d', 'D']).trim();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let number = digits.parse::<u64>().ok()?;
    Some(format!("D{:06}", number))
}

fn parse_export_args(text: &str) -> GatewayIntentArgs {
    let mut args = GatewayIntentArgs::default();
    let mut document_types = Vec::new();

    for token in text.split_whitespace().skip(1) {
        if let Some(value) = token.strip_prefix("from:") {
            args.date_from = Some(value.to_string());
        } else if let Some(value) = token.strip_prefix("to:") {
            args.date_to = Some(value.to_string());
        } else if let Some(value) = token.strip_prefix("type:") {
            document_types.push(value.to_ascii_uppercase());
        } else if let Some(value) = token.strip_prefix("confidence:") {
            args.confidence_min = value.parse::<f64>().ok();
        } else if args.document_short_ref.is_none() {
            args.document_short_ref = normalize_short_ref(token);
        }
    }

    if !document_types.is_empty() {
        args.document_types = Some(document_types);
    }

    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_slash_command_with_bot_suffix() {
        let parsed = parse_slash_intent("/status@fineloraccountant_bot").unwrap();
        assert_eq!(parsed.intent, GatewayIntentKind::Status);
        assert_eq!(parsed.source, IntentResolutionSource::SlashCommand);
    }

    #[test]
    fn start_is_not_a_gateway_help_command() {
        let parsed = parse_slash_intent("/start").unwrap();
        assert_eq!(parsed.intent, GatewayIntentKind::Unknown);

        let parsed_with_token = parse_slash_intent("/start token").unwrap();
        assert_eq!(parsed_with_token.intent, GatewayIntentKind::Unknown);
    }

    #[test]
    fn parses_document_ref_command() {
        let parsed = parse_slash_intent("/why d12").unwrap();
        assert_eq!(parsed.intent, GatewayIntentKind::Why);
        assert_eq!(parsed.args.document_short_ref.as_deref(), Some("D000012"));
        assert!(parsed.missing_args.is_empty());
    }

    #[test]
    fn marks_missing_document_ref() {
        let parsed = parse_slash_intent("/review").unwrap();
        assert_eq!(parsed.intent, GatewayIntentKind::Review);
        assert_eq!(parsed.missing_args, vec!["document_short_ref"]);
    }

    #[test]
    fn parses_export_filters() {
        let parsed = parse_slash_intent(
            "/export D12 from:2026-01-01 to:2026-01-31 type:invoice confidence:0.7",
        )
        .unwrap();

        assert_eq!(parsed.intent, GatewayIntentKind::Export);
        assert_eq!(parsed.args.document_short_ref.as_deref(), Some("D000012"));
        assert_eq!(parsed.args.date_from.as_deref(), Some("2026-01-01"));
        assert_eq!(parsed.args.date_to.as_deref(), Some("2026-01-31"));
        assert_eq!(
            parsed.args.document_types.as_deref(),
            Some(&["INVOICE".to_string()][..])
        );
        assert_eq!(parsed.args.confidence_min, Some(0.7));
    }

    #[test]
    fn parses_model_json() {
        let parsed = parse_model_intent_response(
            r#"{"intent":"status","confidence":0.91,"args":{},"missing_args":[],"reason":"asks status"}"#,
        )
        .unwrap();

        assert_eq!(parsed.intent, GatewayIntentKind::Status);
        assert_eq!(parsed.source, IntentResolutionSource::Model);
        assert!((parsed.confidence - 0.91).abs() < f32::EPSILON);
    }

    #[test]
    fn parses_model_json_with_markdown_prefix() {
        let parsed = parse_model_intent_response(
            r#"Here is the classification:
```json
{"intent":"status","confidence":0.93,"args":{"document_short_ref":"D57"},"missing_args":[],"reason":"asks for a document status"}
```"#,
        )
        .unwrap();

        assert_eq!(parsed.intent, GatewayIntentKind::Status);
        assert_eq!(parsed.args.document_short_ref.as_deref(), Some("D000057"));
    }

    #[test]
    fn slash_command_menu_is_valid_for_telegram() {
        for command in slash_command_menu() {
            assert_ne!(command.name, "start");
            assert!(!command.name.starts_with('/'));
            assert!(command.name.len() <= 32);
            assert!(command.description.len() >= 3);
            assert!(command.description.len() <= 256);
            assert!(
                command
                    .name
                    .chars()
                    .all(|character| character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || character == '_')
            );
        }
    }
}
