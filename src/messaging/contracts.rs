use async_trait::async_trait;
use std::path::PathBuf;

use crate::db;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentInboundMessage {
    Text {
        source: MessageSource,
        text: String,
    },
    Attachment {
        source: MessageSource,
        attachment: AgentAttachment,
    },
    Action {
        source: MessageSource,
        action: DocumentAction,
        response_token: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageSource {
    pub channel: db::ChannelType,
    pub channel_identifier: String,
    pub profile_identifier: Option<String>,
    pub message_id: Option<String>,
    pub source_timestamp: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAttachment {
    pub external_file_id: String,
    pub filename: Option<String>,
    pub mime_type: String,
    pub kind: AgentAttachmentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentAttachmentKind {
    Photo,
    Document,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentAction {
    Approve {
        document_id: i64,
    },
    Reject {
        document_id: i64,
    },
    EditField {
        document_id: i64,
        field: ReviewFieldAction,
    },
    RerunAccounting {
        document_id: i64,
    },
    Retry {
        document_id: i64,
    },
    ReopenReview {
        document_id: i64,
    },
    Export {
        document_id: Option<i64>,
    },
    ConfirmAgentAction {
        confirmation_id: String,
    },
    CancelAgentAction {
        confirmation_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewFieldAction {
    Supplier,
    Amount,
    Date,
    Kontonummer,
    Vat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionButton {
    pub label: String,
    pub action: DocumentAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayActionResponse {
    pub acknowledgement: String,
    pub message: String,
    pub format: GatewayMessageFormat,
    pub attachments: Vec<GatewayAttachment>,
    pub buttons: Option<Vec<Vec<ActionButton>>>,
}

impl GatewayActionResponse {
    pub fn text(message: impl Into<String>, acknowledgement: impl Into<String>) -> Self {
        Self {
            acknowledgement: acknowledgement.into(),
            message: message.into(),
            format: GatewayMessageFormat::Markdown,
            attachments: Vec::new(),
            buttons: None,
        }
    }

    pub fn from_message_response(
        acknowledgement: impl Into<String>,
        response: GatewayMessageResponse,
    ) -> Self {
        Self {
            acknowledgement: acknowledgement.into(),
            message: response.message,
            format: response.format,
            attachments: response.attachments,
            buttons: response.buttons,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayMessageFormat {
    Markdown,
    PlainText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayMessageResponse {
    pub message: String,
    pub format: GatewayMessageFormat,
    pub attachments: Vec<GatewayAttachment>,
    pub buttons: Option<Vec<Vec<ActionButton>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayAttachment {
    LocalFile {
        path: PathBuf,
    },
    StoredMedia {
        channel_type: String,
        external_file_id: String,
        mime_type: String,
        filename: Option<String>,
    },
}

impl GatewayMessageResponse {
    pub fn text(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            format: GatewayMessageFormat::Markdown,
            attachments: Vec::new(),
            buttons: None,
        }
    }

    pub fn plain_text(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            format: GatewayMessageFormat::PlainText,
            attachments: Vec::new(),
            buttons: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundMessage {
    Text(String),
    DocumentFile {
        path: PathBuf,
    },
    ReviewPrompt {
        text: String,
        buttons: Vec<Vec<ActionButton>>,
    },
}

#[async_trait]
pub trait AgentChannel {
    async fn send_text(&self, channel_identifier: &str, text: String) -> anyhow::Result<()>;
    async fn send_document(&self, channel_identifier: &str, path: PathBuf) -> anyhow::Result<()>;
    async fn answer_action(&self, response_token: &str, text: Option<&str>) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_response_constructors_set_expected_format_and_empty_payloads() {
        let markdown = GatewayMessageResponse::text("hello");
        assert_eq!(markdown.message, "hello");
        assert_eq!(markdown.format, GatewayMessageFormat::Markdown);
        assert!(markdown.attachments.is_empty());
        assert!(markdown.buttons.is_none());

        let plain = GatewayMessageResponse::plain_text("hello");
        assert_eq!(plain.message, "hello");
        assert_eq!(plain.format, GatewayMessageFormat::PlainText);
        assert!(plain.attachments.is_empty());
        assert!(plain.buttons.is_none());
    }

    #[test]
    fn action_response_constructors_preserve_message_response_payload() {
        let response = GatewayMessageResponse {
            message: "Export complete".to_string(),
            format: GatewayMessageFormat::PlainText,
            attachments: vec![GatewayAttachment::LocalFile {
                path: PathBuf::from("/tmp/export.zip"),
            }],
            buttons: Some(vec![vec![ActionButton {
                label: "Export".to_string(),
                action: DocumentAction::Export {
                    document_id: Some(7),
                },
            }]]),
        };

        let action = GatewayActionResponse::from_message_response("Confirmed", response.clone());

        assert_eq!(action.acknowledgement, "Confirmed");
        assert_eq!(action.message, response.message);
        assert_eq!(action.format, response.format);
        assert_eq!(action.attachments, response.attachments);
        assert_eq!(action.buttons, response.buttons);
    }

    #[test]
    fn action_text_response_defaults_to_markdown() {
        let response = GatewayActionResponse::text("Done", "Acknowledged");

        assert_eq!(response.acknowledgement, "Acknowledged");
        assert_eq!(response.message, "Done");
        assert_eq!(response.format, GatewayMessageFormat::Markdown);
        assert!(response.attachments.is_empty());
        assert!(response.buttons.is_none());
    }
}
