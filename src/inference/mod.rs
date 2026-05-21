use async_trait::async_trait;

use crate::error::AppResult;

pub mod ollama;

pub use ollama::{OllamaProvider, extract_json_from_response};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl From<ChatMessage> for ToolChatMessage {
    fn from(message: ChatMessage) -> Self {
        Self {
            role: message.role,
            content: message.content,
            tool_name: None,
            tool_calls: None,
        }
    }
}

impl ToolChatMessage {
    pub fn assistant_with_tool_calls(content: String, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            tool_name: None,
            tool_calls: Some(tool_calls),
        }
    }

    pub fn tool_result(tool_name: String, content: String) -> Self {
        Self {
            role: "tool".to_string(),
            content,
            tool_name: Some(tool_name),
            tool_calls: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ToolCallFunction {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ImageJsonRequest {
    pub model: String,
    pub prompt: String,
    pub base64_image: String,
}

#[derive(Debug, Clone)]
pub struct ChatJsonRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub think: bool,
}

#[derive(Debug, Clone)]
pub struct ToolChatRequest {
    pub model: String,
    pub messages: Vec<ToolChatMessage>,
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone)]
pub struct ModelTextResponse {
    pub response: String,
    pub eval_count: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ModelChatResponse {
    pub content: String,
    pub thinking: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub eval_count: Option<i64>,
}

#[async_trait]
pub trait InferenceProvider: Send + Sync {
    async fn generate_image_json(&self, request: ImageJsonRequest) -> AppResult<ModelTextResponse>;
    async fn chat_json(&self, request: ChatJsonRequest) -> AppResult<ModelChatResponse>;
    async fn chat_tools(&self, request: ToolChatRequest) -> AppResult<ModelChatResponse>;
}
