//! Telegram module for notifications and human review UX
//!
//! Handles:
//! - Document status notifications
//! - Human review inline keyboards
//! - Callback handling for reviews

use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, InputFile, ParseMode};
use tracing::info;
use uuid::Uuid;

use crate::error::AppError;
use base64::Engine;

/// Telegram bot notifier
#[derive(Clone)]
pub struct TelegramNotifier {
    bot: Bot,
}

/// Re-export types
pub use teloxide::types::InlineKeyboardButton;

#[derive(Debug, Clone, Copy)]
pub enum MarkdownStyle {
    Plain,
    Bold,
    Code,
}

#[derive(Debug, Clone)]
pub struct MarkdownSegment<'a> {
    pub text: &'a str,
    pub style: MarkdownStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoredTelegramMediaKind {
    Photo,
    Document,
}

impl StoredTelegramMediaKind {
    fn from_mime_type(mime_type: &str) -> Self {
        match mime_type {
            "image/jpeg" | "image/jpg" | "image/png" | "image/webp" => Self::Photo,
            _ => Self::Document,
        }
    }
}

impl TelegramNotifier {
    /// Create a new Telegram notifier
    pub fn new(bot_token: String) -> Self {
        Self {
            bot: Bot::new(bot_token),
        }
    }

    /// Send a simple notification message
    pub async fn send_notification(&self, chat_id: i64, message: &str) -> Result<(), AppError> {
        self.bot
            .send_message(ChatId(chat_id), message)
            .parse_mode(ParseMode::MarkdownV2)
            .await
            .map_err(AppError::Telegram)?;

        info!(chat_id = chat_id, "Sent Telegram notification");
        Ok(())
    }

    /// Send a review message with inline keyboard
    pub async fn send_review_message(
        &self,
        chat_id: i64,
        message: &str,
        keyboard: InlineKeyboardMarkup,
    ) -> Result<(), AppError> {
        self.bot
            .send_message(ChatId(chat_id), message)
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(keyboard)
            .await
            .map_err(AppError::Telegram)?;

        info!(chat_id = chat_id, "Sent review message");
        Ok(())
    }

    /// Re-send previously uploaded Telegram media by file_id.
    pub async fn send_stored_media(
        &self,
        chat_id: i64,
        file_id: &str,
        mime_type: &str,
        filename: Option<&str>,
    ) -> Result<(), AppError> {
        let input = InputFile::file_id(file_id.to_string());

        match StoredTelegramMediaKind::from_mime_type(mime_type) {
            StoredTelegramMediaKind::Photo => {
                self.bot
                    .send_photo(ChatId(chat_id), input)
                    .await
                    .map_err(AppError::Telegram)?;

                info!(
                    chat_id = chat_id,
                    file_id = file_id,
                    mime_type = mime_type,
                    "Sent stored Telegram photo"
                );
            }
            StoredTelegramMediaKind::Document => {
                let request = self.bot.send_document(ChatId(chat_id), input);
                let request = if let Some(filename) = filename {
                    request.caption(filename.to_string())
                } else {
                    request
                };

                request.await.map_err(AppError::Telegram)?;

                info!(
                    chat_id = chat_id,
                    file_id = file_id,
                    mime_type = mime_type,
                    "Sent stored Telegram document"
                );
            }
        }

        Ok(())
    }

    /// Edit an existing message with new text
    pub async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i32,
        new_text: &str,
    ) -> Result<(), AppError> {
        let msg_id = teloxide::types::MessageId(message_id);
        self.bot
            .edit_message_text(ChatId(chat_id), msg_id, new_text)
            .await
            .map_err(AppError::Telegram)?;

        Ok(())
    }

    /// Answer a callback query (button press)
    pub async fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
    ) -> Result<(), AppError> {
        self.bot
            .answer_callback_query(callback_query_id)
            .text(text.unwrap_or(""))
            .await
            .map_err(AppError::Telegram)?;

        Ok(())
    }

    /// Send document status update
    pub async fn send_status_update(
        &self,
        chat_id: i64,
        document_id: uuid::Uuid,
        status: &str,
    ) -> Result<(), AppError> {
        let status_emoji = status_to_emoji(status);

        let lines = [
            markdown_message(&[
                MarkdownSegment {
                    text: status_emoji,
                    style: MarkdownStyle::Plain,
                },
                MarkdownSegment {
                    text: " ",
                    style: MarkdownStyle::Plain,
                },
                MarkdownSegment {
                    text: "Document Update",
                    style: MarkdownStyle::Bold,
                },
            ]),
            String::new(),
            markdown_label_code_value("Document ID", &document_id.to_string()),
            markdown_label_value("Status", status),
        ];
        let message = lines.join("\n");

        self.send_notification(chat_id, &message).await
    }
}

/// Helper function to escape markdown special characters
pub fn escape_markdown(text: &str) -> String {
    text.replace('_', "\\_")
        .replace('*', "\\*")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('(', "\\(")
        .replace(')', "\\)")
        .replace('~', "\\~")
        .replace('`', "\\`")
        .replace('>', "\\>")
        .replace('#', "\\#")
        .replace('&', "\\&")
        .replace('+', "\\+")
        .replace('-', "\\-")
        .replace('=', "\\=")
        .replace('|', "\\|")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace('.', "\\.")
        .replace('!', "\\!")
}

pub fn markdown(text: &str) -> String {
    escape_markdown(text)
}

pub fn markdown_bold(text: &str) -> String {
    format!("*{}*", escape_markdown(text))
}

pub fn markdown_code(text: &str) -> String {
    format!("`{}`", escape_markdown(text))
}

pub fn markdown_label_value(label: &str, value: &str) -> String {
    format!("{}: {}", markdown(label), markdown(value))
}

pub fn markdown_label_code_value(label: &str, value: &str) -> String {
    format!("{}: {}", markdown(label), markdown_code(value))
}

pub fn markdown_message(segments: &[MarkdownSegment<'_>]) -> String {
    let mut output = String::new();

    for segment in segments {
        let rendered = match segment.style {
            MarkdownStyle::Plain => markdown(segment.text),
            MarkdownStyle::Bold => markdown_bold(segment.text),
            MarkdownStyle::Code => markdown_code(segment.text),
        };
        output.push_str(&rendered);
    }

    output
}

#[derive(Debug, Clone)]
pub struct TelegramConnectClaims {
    pub workspace_id: Uuid,
    pub user_id: i64,
    pub exp_ts: i64,
}

pub fn create_connect_token(
    secret: &str,
    workspace_id: Uuid,
    user_id: i64,
    ttl_seconds: i64,
) -> String {
    let exp_ts = chrono::Utc::now().timestamp() + ttl_seconds.max(60);
    let nonce = Uuid::new_v4();
    let payload = format!("v1.{workspace_id}.{user_id}.{exp_ts}.{nonce}");
    let sig = sign_connect_payload(secret, &payload);

    format!("{payload}.{sig}")
}

pub fn validate_connect_token(secret: &str, token: &str) -> Option<TelegramConnectClaims> {
    if !token.starts_with("v1.") {
        return validate_compact_connect_token(secret, token);
    }

    let (payload, sig) = token.rsplit_once('.')?;
    let expected = sign_connect_payload(secret, payload);
    if sig != expected {
        return None;
    }

    let parts = payload.split('.').collect::<Vec<_>>();
    if parts.len() != 5 || parts[0] != "v1" {
        return None;
    }

    let workspace_id = Uuid::parse_str(parts[1]).ok()?;
    let user_id = parts[2].parse::<i64>().ok()?;
    let exp_ts = parts[3].parse::<i64>().ok()?;

    if chrono::Utc::now().timestamp() > exp_ts {
        return None;
    }

    Some(TelegramConnectClaims {
        workspace_id,
        user_id,
        exp_ts,
    })
}

fn validate_compact_connect_token(secret: &str, token: &str) -> Option<TelegramConnectClaims> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .ok()?;
    if decoded.len() != 40 {
        return None;
    }

    let payload = &decoded[..28];
    let sig = &decoded[28..];
    let expected = sign_connect_bytes(secret, payload);
    if sig != &expected[..12] {
        return None;
    }

    let workspace_id = Uuid::from_slice(&payload[..16]).ok()?;
    let exp_ts = u32::from_be_bytes(payload[16..20].try_into().ok()?) as i64;
    if chrono::Utc::now().timestamp() > exp_ts {
        return None;
    }

    Some(TelegramConnectClaims {
        workspace_id,
        user_id: 0,
        exp_ts,
    })
}

pub fn connect_token_fingerprint(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn sign_connect_payload(secret: &str, payload: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update(b":");
    hasher.update(payload.as_bytes());
    hex::encode(hasher.finalize())
}

fn sign_connect_bytes(secret: &str, payload: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update(b":");
    hasher.update(payload);
    hasher.finalize().to_vec()
}

/// Type-safe markdown message formatter with escaped arguments
///
/// This helper macro formats a message template while automatically
/// escaping all dynamic arguments to prevent Telegram MarkdownV2 errors.
///
/// # Example
/// ```
/// use finelor::markdown_msg;
///
/// let doc_id = "invoice.pdf";
/// let message = markdown_msg!("Document: {}", doc_id);
/// ```
#[macro_export]
macro_rules! markdown_msg {
    // No arguments - just return the template
    ($template:expr) => {
        String::from($template)
    };
    // Single argument
    ($template:expr, $arg1:expr) => {
        $template.replacen("{}", &$crate::telegram::escape_markdown($arg1), 1)
    };
    // Two arguments
    ($template:expr, $arg1:expr, $arg2:expr) => {{
        let mut result = $template.to_string();
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg1), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg2), 1);
        result
    }};
    // Three arguments
    ($template:expr, $arg1:expr, $arg2:expr, $arg3:expr) => {{
        let mut result = $template.to_string();
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg1), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg2), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg3), 1);
        result
    }};
    // Four arguments
    ($template:expr, $arg1:expr, $arg2:expr, $arg3:expr, $arg4:expr) => {{
        let mut result = $template.to_string();
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg1), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg2), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg3), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg4), 1);
        result
    }};
    // Five arguments
    ($template:expr, $arg1:expr, $arg2:expr, $arg3:expr, $arg4:expr, $arg5:expr) => {{
        let mut result = $template.to_string();
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg1), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg2), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg3), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg4), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg5), 1);
        result
    }};
    // Six arguments
    ($template:expr, $arg1:expr, $arg2:expr, $arg3:expr, $arg4:expr, $arg5:expr, $arg6:expr) => {{
        let mut result = $template.to_string();
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg1), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg2), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg3), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg4), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg5), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg6), 1);
        result
    }};
    // Seven arguments
    ($template:expr, $arg1:expr, $arg2:expr, $arg3:expr, $arg4:expr, $arg5:expr, $arg6:expr, $arg7:expr) => {{
        let mut result = $template.to_string();
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg1), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg2), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg3), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg4), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg5), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg6), 1);
        result = result.replacen("{}", &$crate::telegram::escape_markdown($arg7), 1);
        result
    }};
}

/// Public helper to get emoji for a document status string.
pub fn status_to_emoji(status: &str) -> &'static str {
    match status {
        "RECEIVED" => "📥",
        "PROCESSING_VISION" | "PROCESSING_ACCOUNTANT" | "PROCESSING_VALIDATOR" => "⏳",
        "VISION_COMPLETE" | "ACCOUNTANT_REVIEWED" | "VALIDATED" => "✨",
        "PENDING_HUMAN_REVIEW" => "⚠️",
        "EXPORT_READY" => "✅",
        "GENERATING_SIE4" => "📦",
        "FAILED" => "❌",
        _ => "📄",
    }
}

/// Build a concise pipeline-progress Telegram message (MarkdownV2-safe).
pub fn build_pipeline_progress_message(
    short_ref: &str,
    status: &str,
    reason: Option<&str>,
) -> String {
    let emoji = status_to_emoji(status);

    let status_label = match status {
        "VISION_COMPLETE" => "Got it! I'm reading your document.",
        "ACCOUNTANT_REVIEWED" => "Accounting analysis complete.",
        "VALIDATED" => "Validation complete.",
        "EXPORT_READY" => "Done! Ready for export.",
        "PENDING_HUMAN_REVIEW" => "I need your help with this one.",
        "FAILED" => "Document could not be processed.",
        _ => "Status updated.",
    };

    let mut lines = vec![markdown_message(&[
        MarkdownSegment {
            text: emoji,
            style: MarkdownStyle::Plain,
        },
        MarkdownSegment {
            text: " ",
            style: MarkdownStyle::Plain,
        },
        MarkdownSegment {
            text: status_label,
            style: MarkdownStyle::Bold,
        },
    ])];

    lines.push(String::new());
    lines.push(markdown_label_code_value("Reference", short_ref));

    if let Some(reason) = reason {
        lines.push(String::new());
        lines.push(markdown_label_value("Reason", reason));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_mime_types_use_photo_path() {
        assert_eq!(
            StoredTelegramMediaKind::from_mime_type("image/jpeg"),
            StoredTelegramMediaKind::Photo
        );
        assert_eq!(
            StoredTelegramMediaKind::from_mime_type("image/png"),
            StoredTelegramMediaKind::Photo
        );
    }

    #[test]
    fn non_image_mime_types_use_document_path() {
        assert_eq!(
            StoredTelegramMediaKind::from_mime_type("application/pdf"),
            StoredTelegramMediaKind::Document
        );
        assert_eq!(
            StoredTelegramMediaKind::from_mime_type("application/octet-stream"),
            StoredTelegramMediaKind::Document
        );
    }

    #[test]
    fn test_escape_markdown_special_chars() {
        let input = "_.*[]()~`>#+-=|{}!";
        let expected = r"\_\.\*\[\]\(\)\~\`\>\#\+\-\=\|\{\}\!";
        assert_eq!(escape_markdown(input), expected);
    }

    #[test]
    fn test_escape_markdown_single_chars() {
        assert_eq!(escape_markdown("test_file.pdf"), "test\\_file\\.pdf");
        assert_eq!(escape_markdown("item_123"), "item\\_123");
        assert_eq!(escape_markdown("doc*draft"), "doc\\*draft");
        assert_eq!(escape_markdown("sum[total]"), "sum\\[total\\]");
        assert_eq!(escape_markdown("value(1)"), "value\\(1\\)");
        assert_eq!(escape_markdown("note~temp"), "note\\~temp");
        assert_eq!(escape_markdown("code`block"), "code\\`block");
        assert_eq!(escape_markdown("line>here"), "line\\>here");
        assert_eq!(escape_markdown("heading#1"), "heading\\#1");
        assert_eq!(escape_markdown("a+b=c"), "a\\+b\\=c");
        assert_eq!(escape_markdown("a-b"), "a\\-b");
        assert_eq!(escape_markdown("a&b"), "a\\&b");
        assert_eq!(escape_markdown("a|b"), "a\\|b");
        assert_eq!(escape_markdown("a{b"), "a\\{b");
        assert_eq!(escape_markdown("a}b"), "a\\}b");
        assert_eq!(escape_markdown("Hello!"), "Hello\\!");
    }

    #[test]
    fn test_escape_markdown_no_special_chars() {
        let input = "Hello World 123 ABC";
        assert_eq!(escape_markdown(input), input);
    }

    #[test]
    fn test_escape_markdown_empty() {
        assert_eq!(escape_markdown(""), "");
    }

    #[test]
    fn test_escape_markdown_filenames() {
        // Test realistic filenames with dots
        assert_eq!(
            escape_markdown("invoice_2024_03.pdf"),
            "invoice\\_2024\\_03\\.pdf"
        );
        assert_eq!(
            escape_markdown("report_v1.2.3.docx"),
            "report\\_v1\\.2\\.3\\.docx"
        );
        assert_eq!(escape_markdown("image.test.png"), "image\\.test\\.png");
    }

    #[test]
    fn test_escape_markdown_document_ids() {
        // Test document IDs with hyphens
        assert_eq!(
            escape_markdown("550e8400-e29b-41d4-a716-446655440000"),
            "550e8400\\-e29b\\-41d4\\-a716\\-446655440000"
        );
        assert_eq!(escape_markdown("doc-123-abc"), "doc\\-123\\-abc");
    }

    #[test]
    fn test_escape_markdown_error_messages() {
        // Test error messages with special characters
        assert_eq!(
            escape_markdown("Error: File not found!"),
            "Error: File not found\\!"
        );
        assert_eq!(
            escape_markdown("Invalid input (expected: number)"),
            "Invalid input \\(expected: number\\)"
        );
        assert_eq!(
            escape_markdown("Operation failed: item_123"),
            "Operation failed: item\\_123"
        );
    }

    #[test]
    fn test_markdown_msg_single_arg() {
        assert_eq!(
            crate::markdown_msg!("Document: {}", "inv.pdf"),
            r"Document: inv\.pdf"
        );
    }

    #[test]
    fn test_markdown_msg_multiple_args() {
        assert_eq! {
            crate::markdown_msg!("Document: {} Status: {}", "inv_2024.pdf", "OK!"),
            r"Document: inv\_2024\.pdf Status: OK\!"
        };
    }

    #[test]
    fn test_markdown_msg_with_special_chars() {
        let result = crate::markdown_msg!(
            "ID: {} Amount: {}",
            "550e8400-e29b-41d4-a716-446655440000",
            "1.000,50 SEK"
        );
        // backslash is used for escaping in format strings, so we check parts
        assert!(result.contains("550e8400\\-e29b\\-41d4\\-a716\\-446655440000"));
        assert!(result.contains("1\\.000,50 SEK")); // dot escaped, comma not escaped
    }

    #[test]
    fn test_markdown_msg_no_args() {
        let msg = crate::markdown_msg!("Static message");
        assert_eq!(msg, "Static message");
    }

    #[test]
    fn test_markdown_mixing_static_and_dynamic() {
        // The Invoice # is static and should NOT be escaped
        // The doc_id is dynamic and should be escaped
        let result = crate::markdown_msg!(
            "📄 *Invoice #*\nID: `{}`, Amount: {}",
            "doc_123",
            "1.234,56"
        );
        assert!(result.contains("doc\\_123"));
        assert!(result.contains("1\\.234,56"));
    }

    #[test]
    fn test_markdown_message_escapes_static_and_dynamic_text() {
        let message = markdown_message(&[
            MarkdownSegment {
                text: "Document Approved",
                style: MarkdownStyle::Bold,
            },
            MarkdownSegment {
                text: "\n\n",
                style: MarkdownStyle::Plain,
            },
            MarkdownSegment {
                text: "The document has been automatically approved and added to the export pool.",
                style: MarkdownStyle::Plain,
            },
        ]);

        assert_eq!(
            message,
            "*Document Approved*\n\nThe document has been automatically approved and added to the export pool\\."
        );
    }

    #[test]
    fn test_markdown_label_helpers_escape_reserved_characters() {
        assert_eq!(
            markdown_label_value("Supplier", "A&B Co. (SE)"),
            "Supplier: A\\&B Co\\. \\(SE\\)"
        );
        assert_eq!(
            markdown_label_code_value("Document ID", "550e8400-e29b-41d4-a716-446655440000"),
            "Document ID: `550e8400\\-e29b\\-41d4\\-a716\\-446655440000`"
        );
    }

    #[test]
    fn test_build_pipeline_progress_message_escapes_short_ref() {
        let msg = build_pipeline_progress_message("D000123", "EXPORT_READY", None);
        assert!(msg.contains("*Done\\! Ready for export\\.*"));
        assert!(msg.contains("`D000123`"));
    }

    #[test]
    fn test_build_pipeline_progress_message_includes_reason() {
        let msg = build_pipeline_progress_message(
            "D000123",
            "PENDING_HUMAN_REVIEW",
            Some("Validation found issues that need review."),
        );
        assert!(msg.contains("*I need your help with this one\\.*"));
        assert!(msg.contains(r"Reason: Validation found issues that need review\."));
    }

    #[test]
    fn test_build_pipeline_progress_message_vision_complete() {
        let msg = build_pipeline_progress_message("D000001", "VISION_COMPLETE", None);
        assert!(msg.contains("*Got it\\! I'm reading your document\\.*"));
        assert!(msg.contains("`D000001`"));
    }

    #[test]
    fn test_build_pipeline_progress_message_unknown_status() {
        let msg = build_pipeline_progress_message("R0001", "CUSTOM_STATUS", None);
        assert!(msg.contains("*Status updated\\.*"));
    }

    #[test]
    fn connect_token_roundtrip_preserves_internal_user_id() {
        let secret = "test-token-secret";
        let workspace_id = Uuid::new_v4();
        let user_id = 7_i64;

        let token = create_connect_token(secret, workspace_id, user_id, 900);
        let claims = validate_connect_token(secret, &token).expect("token should validate");

        assert_eq!(claims.workspace_id, workspace_id);
        assert_eq!(claims.user_id, user_id);
        assert!(claims.exp_ts > chrono::Utc::now().timestamp());
    }
}
