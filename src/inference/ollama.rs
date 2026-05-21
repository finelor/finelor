use std::time::{Duration, Instant};

use async_trait::async_trait;
use backoff::{ExponentialBackoff, backoff::Backoff};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use super::{
    ChatJsonRequest, ChatMessage, ImageJsonRequest, InferenceProvider, ModelChatResponse,
    ModelTextResponse, ToolCall, ToolChatMessage, ToolChatRequest, ToolDefinition,
};
use crate::config::OllamaConfig;
use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    max_retries: u32,
    timeout: Duration,
    initial_backoff: Duration,
    max_backoff: Duration,
}

#[derive(Debug, Clone, Serialize)]
struct OllamaGenerateRequest {
    model: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    images: Option<Vec<String>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct OllamaGenerateResponse {
    pub model: String,
    pub created_at: String,
    pub response: String,
    pub done: bool,
    #[serde(default)]
    pub context: Option<Vec<i64>>,
    #[serde(default)]
    pub total_duration: Option<i64>,
    #[serde(default)]
    pub load_duration: Option<i64>,
    #[serde(default)]
    pub prompt_eval_count: Option<i64>,
    #[serde(default)]
    pub prompt_eval_duration: Option<i64>,
    #[serde(default)]
    pub eval_count: Option<i64>,
    #[serde(default)]
    pub eval_duration: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<serde_json::Value>,
    stream: bool,
    think: bool,
}

#[derive(Debug, Clone, Serialize)]
struct OllamaToolChatRequest {
    model: String,
    messages: Vec<ToolChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDefinition>>,
    stream: bool,
    think: bool,
}

#[derive(Debug, Deserialize)]
pub struct OllamaChatResponse {
    pub model: String,
    pub created_at: String,
    pub message: OllamaChatResponseMessage,
    pub done: bool,
    #[serde(default)]
    pub total_duration: Option<i64>,
    #[serde(default)]
    pub load_duration: Option<i64>,
    #[serde(default)]
    pub prompt_eval_count: Option<i64>,
    #[serde(default)]
    pub prompt_eval_duration: Option<i64>,
    #[serde(default)]
    pub eval_count: Option<i64>,
    #[serde(default)]
    pub eval_duration: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct OllamaChatResponseMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl OllamaProvider {
    pub fn new(config: &OllamaConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            max_retries: config.max_retries,
            timeout: Duration::from_secs(config.timeout_seconds),
            initial_backoff: Duration::from_millis(config.initial_backoff_ms),
            max_backoff: Duration::from_secs(config.max_backoff_seconds),
        }
    }

    /// Generate text from a prompt (text-only models like accountant)
    pub async fn generate(
        &self,
        model: &str,
        prompt: &str,
        options: Option<serde_json::Value>,
    ) -> AppResult<OllamaGenerateResponse> {
        let request = OllamaGenerateRequest {
            model: model.to_string(),
            prompt: prompt.to_string(),
            images: None,
            stream: false,
            options,
        };

        self.call_with_retry(request).await
    }

    /// Generate text from an image (vision models like qwen)
    pub async fn generate_with_image(
        &self,
        model: &str,
        prompt: &str,
        base64_image: &str,
    ) -> AppResult<OllamaGenerateResponse> {
        let request = OllamaGenerateRequest {
            model: model.to_string(),
            prompt: prompt.to_string(),
            images: Some(vec![base64_image.to_string()]),
            stream: false,
            options: None,
        };

        self.call_with_retry(request).await
    }

    pub async fn chat_json(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        think: bool,
    ) -> AppResult<OllamaChatResponse> {
        let request = OllamaChatRequest {
            model: model.to_string(),
            messages,
            format: Some(serde_json::Value::String("json".to_string())),
            stream: false,
            think,
        };

        self.call_chat_with_retry(request).await
    }

    pub async fn chat(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
    ) -> AppResult<OllamaChatResponse> {
        let request = OllamaChatRequest {
            model: model.to_string(),
            messages,
            format: None,
            stream: false,
            think: false,
        };

        self.call_chat_with_retry(request).await
    }

    pub async fn chat_with_tools(
        &self,
        model: &str,
        messages: Vec<ToolChatMessage>,
        tools: Vec<ToolDefinition>,
    ) -> AppResult<OllamaChatResponse> {
        let request = OllamaToolChatRequest {
            model: model.to_string(),
            messages,
            tools: Some(tools),
            stream: false,
            think: false,
        };

        self.call_tool_chat_with_retry(request).await
    }

    async fn call_with_retry(
        &self,
        request: OllamaGenerateRequest,
    ) -> AppResult<OllamaGenerateResponse> {
        let url = format!("{}/generate", self.base_url);
        let client = self.client.clone();
        let request_clone = request.clone();
        let model = request.model.clone();
        let start = Instant::now();
        let mut attempts = 0;
        let max_attempts = self.max_retries.saturating_add(1);
        let mut backoff = self.build_backoff();
        let mut last_error = String::new();

        loop {
            attempts += 1;
            debug!("Calling Ollama API: model={}, attempt={}", model, attempts);

            let mut request_builder = client.post(&url).json(&request_clone);

            if let Some(ref key) = self.api_key {
                request_builder =
                    request_builder.header("Authorization", format!("Bearer {}", key));
            }

            let response = match request_builder.send().await {
                Ok(response) => response,
                Err(error) => {
                    let err = AppError::Ollama(format!(
                        "HTTP error: error sending request for url ({})",
                        error.url().map(|u| u.as_str()).unwrap_or(&url)
                    ));
                    match self.handle_transient_retry(
                        "generate",
                        &url,
                        &model,
                        &mut backoff,
                        start,
                        attempts,
                        max_attempts,
                        &err,
                        &mut last_error,
                    ) {
                        Ok(delay) => {
                            sleep(delay).await;
                        }
                        Err(exhausted) => return Err(exhausted),
                    }
                    continue;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let err = AppError::Ollama(format!("API error {}: {}", status, body));

                if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    match self.handle_transient_retry(
                        "generate",
                        &url,
                        &model,
                        &mut backoff,
                        start,
                        attempts,
                        max_attempts,
                        &err,
                        &mut last_error,
                    ) {
                        Ok(delay) => {
                            sleep(delay).await;
                        }
                        Err(exhausted) => return Err(exhausted),
                    }
                    continue;
                }

                return Err(err);
            }

            let res: OllamaGenerateResponse = response
                .json()
                .await
                .map_err(|e| AppError::Ollama(format!("JSON parse error: {}", e)))?;

            info!(
                "Ollama API call successful: model={}, done={}, eval_count={:?}",
                res.model, res.done, res.eval_count
            );

            return Ok(res);
        }
    }

    async fn call_chat_with_retry(
        &self,
        request: OllamaChatRequest,
    ) -> AppResult<OllamaChatResponse> {
        let url = format!("{}/chat", self.base_url);
        let client = self.client.clone();
        let request_clone = request.clone();
        let model = request.model.clone();
        let start = Instant::now();
        let mut attempts = 0;
        let max_attempts = self.max_retries.saturating_add(1);
        let mut backoff = self.build_backoff();
        let mut last_error = String::new();

        loop {
            attempts += 1;
            debug!(
                "Calling Ollama chat API: model={}, attempt={}",
                model, attempts
            );

            let mut request_builder = client.post(&url).json(&request_clone);

            if let Some(ref key) = self.api_key {
                request_builder =
                    request_builder.header("Authorization", format!("Bearer {}", key));
            }

            let response = match request_builder.send().await {
                Ok(response) => response,
                Err(error) => {
                    let err = AppError::Ollama(format!(
                        "HTTP error: error sending request for url ({})",
                        error.url().map(|u| u.as_str()).unwrap_or(&url)
                    ));
                    match self.handle_transient_retry(
                        "chat",
                        &url,
                        &model,
                        &mut backoff,
                        start,
                        attempts,
                        max_attempts,
                        &err,
                        &mut last_error,
                    ) {
                        Ok(delay) => {
                            sleep(delay).await;
                        }
                        Err(exhausted) => return Err(exhausted),
                    }
                    continue;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let err = AppError::Ollama(format!("API error {}: {}", status, body));

                if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    match self.handle_transient_retry(
                        "chat",
                        &url,
                        &model,
                        &mut backoff,
                        start,
                        attempts,
                        max_attempts,
                        &err,
                        &mut last_error,
                    ) {
                        Ok(delay) => {
                            sleep(delay).await;
                        }
                        Err(exhausted) => return Err(exhausted),
                    }
                    continue;
                }

                return Err(err);
            }

            let res: OllamaChatResponse = response
                .json()
                .await
                .map_err(|e| AppError::Ollama(format!("JSON parse error: {}", e)))?;

            info!(
                "Ollama chat API call successful: model={}, done={}, eval_count={:?}",
                res.model, res.done, res.eval_count
            );

            return Ok(res);
        }
    }

    async fn call_tool_chat_with_retry(
        &self,
        request: OllamaToolChatRequest,
    ) -> AppResult<OllamaChatResponse> {
        let url = format!("{}/chat", self.base_url);
        let client = self.client.clone();
        let request_clone = request.clone();
        let model = request.model.clone();
        let start = Instant::now();
        let mut attempts = 0;
        let max_attempts = self.max_retries.saturating_add(1);
        let mut backoff = self.build_backoff();
        let mut last_error = String::new();

        loop {
            attempts += 1;
            debug!(
                "Calling Ollama tool chat API: model={}, attempt={}",
                model, attempts
            );

            let mut request_builder = client.post(&url).json(&request_clone);

            if let Some(ref key) = self.api_key {
                request_builder =
                    request_builder.header("Authorization", format!("Bearer {}", key));
            }

            let response = match request_builder.send().await {
                Ok(response) => response,
                Err(error) => {
                    let err = AppError::Ollama(format!(
                        "HTTP error: error sending request for url ({})",
                        error.url().map(|u| u.as_str()).unwrap_or(&url)
                    ));
                    match self.handle_transient_retry(
                        "tool_chat",
                        &url,
                        &model,
                        &mut backoff,
                        start,
                        attempts,
                        max_attempts,
                        &err,
                        &mut last_error,
                    ) {
                        Ok(delay) => {
                            sleep(delay).await;
                        }
                        Err(exhausted) => return Err(exhausted),
                    }
                    continue;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let err = AppError::Ollama(format!("API error {}: {}", status, body));

                if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    match self.handle_transient_retry(
                        "tool_chat",
                        &url,
                        &model,
                        &mut backoff,
                        start,
                        attempts,
                        max_attempts,
                        &err,
                        &mut last_error,
                    ) {
                        Ok(delay) => {
                            sleep(delay).await;
                        }
                        Err(exhausted) => return Err(exhausted),
                    }
                    continue;
                }

                return Err(err);
            }

            let res: OllamaChatResponse = response
                .json()
                .await
                .map_err(|e| AppError::Ollama(format!("JSON parse error: {}", e)))?;

            info!(
                "Ollama tool chat API call successful: model={}, done={}, eval_count={:?}, tool_calls={}",
                res.model,
                res.done,
                res.eval_count,
                res.message
                    .tool_calls
                    .as_ref()
                    .map(|calls| calls.len())
                    .unwrap_or(0)
            );

            return Ok(res);
        }
    }

    fn build_backoff(&self) -> ExponentialBackoff {
        ExponentialBackoff {
            initial_interval: self.initial_backoff,
            max_interval: self.max_backoff,
            max_elapsed_time: Some(self.timeout),
            ..Default::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_transient_retry(
        &self,
        operation: &str,
        url: &str,
        model: &str,
        backoff: &mut ExponentialBackoff,
        start: Instant,
        attempts: u32,
        max_attempts: u32,
        error: &AppError,
        last_error: &mut String,
    ) -> Result<Duration, AppError> {
        *last_error = error.to_string();
        let elapsed = start.elapsed();

        if attempts >= max_attempts {
            warn!(
                operation,
                url,
                model,
                attempts,
                max_attempts,
                elapsed_ms = elapsed.as_millis(),
                last_error = %last_error,
                "Ollama request exhausted retry attempts"
            );
            return Err(AppError::Ollama(format!(
                "{} request failed after {} attempts over {:.1}s to {} for model {}. Last error: {}",
                operation,
                attempts,
                elapsed.as_secs_f64(),
                url,
                model,
                last_error
            )));
        }

        match backoff.next_backoff() {
            Some(delay) => {
                warn!(
                    operation,
                    url,
                    model,
                    attempt = attempts,
                    max_attempts,
                    elapsed_ms = elapsed.as_millis(),
                    retry_delay_ms = delay.as_millis(),
                    last_error = %last_error,
                    "Ollama request failed; retrying"
                );
                Ok(delay)
            }
            None => {
                warn!(
                    operation,
                    url,
                    model,
                    attempts,
                    max_attempts,
                    elapsed_ms = elapsed.as_millis(),
                    timeout_ms = self.timeout.as_millis(),
                    last_error = %last_error,
                    "Ollama request exhausted retry time budget"
                );
                Err(AppError::Ollama(format!(
                    "{} request failed after {} attempts over {:.1}s to {} for model {}. Retry time budget exhausted (timeout {}s). Last error: {}",
                    operation,
                    attempts,
                    elapsed.as_secs_f64(),
                    url,
                    model,
                    self.timeout.as_secs(),
                    last_error
                )))
            }
        }
    }
}

#[async_trait]
impl InferenceProvider for OllamaProvider {
    async fn generate_image_json(&self, request: ImageJsonRequest) -> AppResult<ModelTextResponse> {
        let response = self
            .generate_with_image(&request.model, &request.prompt, &request.base64_image)
            .await?;

        Ok(ModelTextResponse {
            response: response.response,
            eval_count: response.eval_count,
        })
    }

    async fn chat_json(&self, request: ChatJsonRequest) -> AppResult<ModelChatResponse> {
        let response = self
            .chat_json(&request.model, request.messages, request.think)
            .await?;

        Ok(ModelChatResponse {
            content: response.message.content,
            thinking: response.message.thinking,
            tool_calls: response.message.tool_calls,
            eval_count: response.eval_count,
        })
    }

    async fn chat_tools(&self, request: ToolChatRequest) -> AppResult<ModelChatResponse> {
        let response = self
            .chat_with_tools(&request.model, request.messages, request.tools)
            .await?;

        Ok(ModelChatResponse {
            content: response.message.content,
            thinking: response.message.thinking,
            tool_calls: response.message.tool_calls,
            eval_count: response.eval_count,
        })
    }
}

/// Clean up JSON response from Ollama that may contain markdown or extra text
pub fn extract_json_from_response(response: &str) -> AppResult<serde_json::Value> {
    let original = response;

    // First try to parse directly
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(response) {
        return Ok(json);
    }

    // Clean the response of common issues
    let mut cleaned = response.to_string();

    // Remove markdown code blocks
    cleaned = cleaned.replace("```json", "").replace("```", "");

    // Remove Chinese garbage characters (and other non-ASCII control chars) that appear in values
    cleaned = cleaned
        .chars()
        .filter(|c| {
            // Keep ASCII printable, newlines, tabs, and valid JSON syntax
            let c = *c;
            c.is_ascii_graphic() || c.is_ascii_whitespace() || c == '"' || c == '\\' || c == '/'
        })
        .collect();

    // Try to find JSON boundaries - look for outermost { ... }
    // This handles cases where LLM adds explanatory text before/after
    let mut brace_depth = 0;
    let mut json_start: Option<usize> = None;
    let mut json_end: Option<usize> = None;

    for (i, c) in cleaned.char_indices() {
        match c {
            '{' => {
                if brace_depth == 0 {
                    json_start = Some(i);
                }
                brace_depth += 1;
            }
            '}' => {
                brace_depth -= 1;
                if brace_depth == 0 && json_start.is_some() {
                    json_end = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }

    // Try to parse the extracted JSON
    if let (Some(start), Some(end)) = (json_start, json_end) {
        let extracted = &cleaned[start..end];
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(extracted) {
            return Ok(json);
        }

        // Try additional cleaning on the extracted portion
        let mut further_cleaned = extracted.to_string();

        // Fix common numeric issues like "deduction":夯实 0 -> "deduction": 0
        // Remove extra text within value positions
        further_cleaned = regex_replace_all(&further_cleaned);

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&further_cleaned) {
            return Ok(json);
        }
    }

    // Try the cleaned version as-is
    cleaned = cleaned.trim().to_string();
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&cleaned) {
        return Ok(json);
    }

    // Last resort: try to fix specific known patterns
    let fixed = attempt_json_repair(&cleaned);
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&fixed) {
        return Ok(json);
    }

    // Log the failure with the original response
    error!(
        response = %original.chars().take(2000).collect::<String>(),
        cleaned = %cleaned.chars().take(2000).collect::<String>(),
        "Failed to parse JSON from response"
    );

    Err(AppError::Serialization(format!(
        "Could not extract valid JSON. Original response preview: {}",
        original.chars().take(500).collect::<String>()
    )))
}

/// Apply regex replacements to fix common JSON issues
fn regex_replace_all(input: &str) -> String {
    // Fix pattern: key followed by garbage then number -> key: number
    // Replace any non-ASCII characters before numbers
    let mut output = String::with_capacity(input.len());
    let mut prev_was_colon = false;

    for c in input.chars() {
        if c == ':' {
            prev_was_colon = true;
            output.push(c);
        } else if prev_was_colon && c.is_ascii_digit() {
            prev_was_colon = false;
            output.push(c);
        } else if prev_was_colon && !c.is_ascii_whitespace() && c != '-' {
            // Still garbage after colon, skip until we hit a digit or valid value start
            if c == '"' || c == '{' || c == '[' || c == 't' || c == 'f' || c == 'n' {
                prev_was_colon = false;
                output.push(c);
            }
            // Skip this character (garbage)
        } else {
            if !c.is_ascii_whitespace() && !c.is_ascii_control() {
                output.push(c);
            }
            prev_was_colon = false;
        }
    }

    output
}

/// Attempt to repair common JSON structure issues
fn attempt_json_repair(input: &str) -> String {
    let mut result = input.to_string();

    // Remove trailing characters after the last }
    if let Some(last_brace) = input.rfind('}')
        && last_brace < input.len() - 1
    {
        result = input[..=last_brace].to_string();
    }

    // Ensure the result starts with {
    if !result.trim().starts_with('{')
        && let Some(first_brace) = result.find('{')
    {
        result = result[first_brace..].to_string();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::ToolFunction;

    #[test]
    fn plain_chat_request_omits_json_format() {
        let request = OllamaChatRequest {
            model: "kimi-k2.6:cloud".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            }],
            format: None,
            stream: false,
            think: false,
        };

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["model"], "kimi-k2.6:cloud");
        assert!(value.get("format").is_none());
        assert!(value.get("tools").is_none());
    }

    #[test]
    fn json_chat_request_keeps_json_format() {
        let request = OllamaChatRequest {
            model: "deepseek-v3.2:cloud".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "{}".to_string(),
            }],
            format: Some(serde_json::Value::String("json".to_string())),
            stream: false,
            think: true,
        };

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["format"], "json");
        assert_eq!(value["think"], true);
    }

    #[test]
    fn tool_chat_request_includes_tools_and_omits_json_format() {
        let request = OllamaToolChatRequest {
            model: "kimi-k2.6:cloud".to_string(),
            messages: vec![ToolChatMessage {
                role: "user".to_string(),
                content: "How many documents need review?".to_string(),
                tool_name: None,
                tool_calls: None,
            }],
            tools: Some(vec![ToolDefinition {
                tool_type: "function".to_string(),
                function: ToolFunction {
                    name: "list_pending_reviews".to_string(),
                    description: "List documents needing attention.".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "additionalProperties": false
                    }),
                },
            }]),
            stream: false,
            think: false,
        };

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["model"], "kimi-k2.6:cloud");
        assert!(value.get("format").is_none());
        assert_eq!(value["tools"][0]["type"], "function");
        assert_eq!(
            value["tools"][0]["function"]["name"],
            "list_pending_reviews"
        );
    }

    #[test]
    fn tool_chat_response_deserializes_tool_calls() {
        let raw = serde_json::json!({
            "model": "kimi-k2.6:cloud",
            "created_at": "2026-05-13T00:00:00Z",
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "function": {
                        "name": "list_pending_reviews",
                        "arguments": { "limit": 10 }
                    }
                }]
            },
            "done": true
        });

        let response: OllamaChatResponse = serde_json::from_value(raw).unwrap();
        let tool_calls = response.message.tool_calls.unwrap();
        assert_eq!(tool_calls[0].function.name, "list_pending_reviews");
        assert_eq!(tool_calls[0].function.arguments["limit"], 10);
    }

    #[test]
    fn extracts_plain_json_response() {
        let value = extract_json_from_response(r#"{"supplier":"ACME","total":123.45}"#).unwrap();

        assert_eq!(value["supplier"], "ACME");
        assert_eq!(value["total"], 123.45);
    }

    #[test]
    fn extracts_json_from_markdown_fence() {
        let response = "```json\n{\"document_type\":\"receipt\",\"confidence\":0.82}\n```";
        let value = extract_json_from_response(response).unwrap();

        assert_eq!(value["document_type"], "receipt");
        assert_eq!(value["confidence"], 0.82);
    }

    #[test]
    fn extracts_json_with_surrounding_text() {
        let response = "Here is the JSON:\n{\"status\":\"ok\",\"items\":[1,2]}\nDone.";
        let value = extract_json_from_response(response).unwrap();

        assert_eq!(value["status"], "ok");
        assert_eq!(value["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn rejects_response_without_json_object() {
        let err = extract_json_from_response("there is no JSON here").expect_err("should fail");

        assert!(err.to_string().contains("Could not extract valid JSON"));
    }

    #[test]
    fn rejects_truncated_json_response() {
        let err = extract_json_from_response(r#"{"status":"ok""#).expect_err("should fail");

        assert!(err.to_string().contains("Could not extract valid JSON"));
    }
}
