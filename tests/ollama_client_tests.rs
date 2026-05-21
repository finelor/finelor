//! HTTP integration tests for the production Ollama inference provider.

use finelor::config::{OllamaConfig, OllamaModels};
use finelor::inference::{
    ChatMessage, OllamaProvider, ToolChatMessage, ToolDefinition, ToolFunction,
};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config(base_url: String) -> OllamaConfig {
    OllamaConfig {
        base_url,
        api_key: None,
        models: OllamaModels {
            vision: "test-vision".to_string(),
            accountant: "test-accountant".to_string(),
            assistant: "test-agent-chat".to_string(),
        },
        vision_prompt_path: "assets/prompts/sweden/vision_agent.md".to_string(),
        accountant_prompt_path: "assets/prompts/sweden/accountant_agent.md".to_string(),
        assistant_soul_prompt_path: "assets/prompts/_shared/assistant_agent_soul.md".to_string(),
        timeout_seconds: 2,
        max_retries: 0,
        initial_backoff_ms: 1,
        max_backoff_seconds: 1,
    }
}

fn generate_response(model: &str, response: &str) -> serde_json::Value {
    json!({
        "model": model,
        "created_at": "2026-05-20T00:00:00Z",
        "response": response,
        "done": true,
        "eval_count": 3
    })
}

fn chat_response(content: &str) -> serde_json::Value {
    json!({
        "model": "test-agent-chat",
        "created_at": "2026-05-20T00:00:00Z",
        "message": {
            "role": "assistant",
            "content": content,
            "thinking": "brief reasoning"
        },
        "done": true,
        "eval_count": 5
    })
}

#[tokio::test]
async fn generate_posts_expected_request_and_parses_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .and(body_json(json!({
            "model": "test-model",
            "prompt": "Extract JSON",
            "stream": false,
            "options": { "temperature": 0 }
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(generate_response("test-model", r#"{"ok":true}"#)),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(&config(server.uri()));
    let response = provider
        .generate(
            "test-model",
            "Extract JSON",
            Some(json!({ "temperature": 0 })),
        )
        .await
        .expect("generate response");

    assert_eq!(response.model, "test-model");
    assert_eq!(response.response, r#"{"ok":true}"#);
    assert_eq!(response.eval_count, Some(3));
}

#[tokio::test]
async fn generate_with_image_sends_images_array() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .and(body_json(json!({
            "model": "vision-model",
            "prompt": "Read image",
            "images": ["base64-image"],
            "stream": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(generate_response(
            "vision-model",
            r#"{"document_type":"receipt"}"#,
        )))
        .expect(1)
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(&config(server.uri()));
    let response = provider
        .generate_with_image("vision-model", "Read image", "base64-image")
        .await
        .expect("image response");

    assert!(response.response.contains("document_type"));
}

#[tokio::test]
async fn api_key_is_sent_as_bearer_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(generate_response("test", "OK")))
        .expect(1)
        .mount(&server)
        .await;

    let mut cfg = config(server.uri());
    cfg.api_key = Some("test-key".to_string());
    let provider = OllamaProvider::new(&cfg);

    let response = provider.generate("test", "prompt", None).await.unwrap();
    assert_eq!(response.response, "OK");
}

#[tokio::test]
async fn non_retryable_client_error_returns_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(ResponseTemplate::new(404).set_body_string("model not found"))
        .expect(1)
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(&config(server.uri()));
    let err = provider
        .generate("missing", "prompt", None)
        .await
        .expect_err("404 should fail");

    assert!(err.to_string().contains("404"));
    assert!(err.to_string().contains("model not found"));
}

#[tokio::test]
async fn transient_server_error_retries_and_can_succeed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(generate_response("test", "OK")))
        .mount(&server)
        .await;

    let mut cfg = config(server.uri());
    cfg.max_retries = 1;
    let provider = OllamaProvider::new(&cfg);

    let response = provider.generate("test", "prompt", None).await.unwrap();
    assert_eq!(response.response, "OK");
}

#[tokio::test]
async fn malformed_response_body_returns_parse_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not valid json"))
        .expect(1)
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(&config(server.uri()));
    let err = provider
        .generate("test", "prompt", None)
        .await
        .expect_err("malformed response should fail");

    assert!(err.to_string().contains("JSON parse error"));
}

#[tokio::test]
async fn chat_json_uses_json_format_and_preserves_thinking() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat"))
        .and(body_json(json!({
            "model": "chat-model",
            "messages": [{ "role": "user", "content": "Return JSON" }],
            "format": "json",
            "stream": false,
            "think": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_response(r#"{"ok":true}"#)))
        .expect(1)
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(&config(server.uri()));
    let response = provider
        .chat_json(
            "chat-model",
            vec![ChatMessage {
                role: "user".to_string(),
                content: "Return JSON".to_string(),
            }],
            true,
        )
        .await
        .expect("chat json");

    assert_eq!(response.message.content, r#"{"ok":true}"#);
    assert_eq!(
        response.message.thinking.as_deref(),
        Some("brief reasoning")
    );
}

#[tokio::test]
async fn chat_with_tools_parses_tool_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "tool-model",
            "created_at": "2026-05-20T00:00:00Z",
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "function": {
                        "name": "get_document",
                        "arguments": { "short_ref": "D000001" }
                    }
                }]
            },
            "done": true,
            "eval_count": 1
        })))
        .expect(1)
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(&config(server.uri()));
    let response = provider
        .chat_with_tools(
            "tool-model",
            vec![ToolChatMessage {
                role: "user".to_string(),
                content: "show D000001".to_string(),
                tool_name: None,
                tool_calls: None,
            }],
            vec![ToolDefinition {
                tool_type: "function".to_string(),
                function: ToolFunction {
                    name: "get_document".to_string(),
                    description: "Get document".to_string(),
                    parameters: json!({ "type": "object" }),
                },
            }],
        )
        .await
        .expect("tool chat");

    let calls = response.message.tool_calls.expect("tool calls");
    assert_eq!(calls[0].function.name, "get_document");
    assert_eq!(calls[0].function.arguments["short_ref"], "D000001");
}
