use crate::db::DbPool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::inference::{ToolCall, ToolDefinition as InferenceToolDefinition, ToolFunction};
use crate::query::{
    DocumentSummary, count_documents, count_documents_by_status,
    count_documents_requiring_attention, document_status_counts, document_summary_by_short_ref,
    list_documents_by_status, list_documents_requiring_attention, list_recent_documents,
};

use super::commands::describe_document_why;
use super::intents::normalize_short_ref;

pub const MAX_READ_ONLY_TOOL_CALLS: usize = 3;
const DEFAULT_LIST_LIMIT: i64 = 10;
const MAX_LIST_LIMIT: i64 = 20;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReadOnlyToolCall {
    pub name: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

impl TryFrom<&ToolCall> for ReadOnlyToolCall {
    type Error = anyhow::Error;

    fn try_from(value: &ToolCall) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.function.name.clone(),
            args: normalize_tool_arguments(&value.function.arguments)?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AppToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
}

impl From<AppToolDefinition> for InferenceToolDefinition {
    fn from(definition: AppToolDefinition) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: ToolFunction {
                name: definition.name.to_string(),
                description: definition.description.to_string(),
                parameters: definition.input_schema,
            },
        }
    }
}

pub fn read_only_tool_catalog() -> Vec<AppToolDefinition> {
    vec![
        AppToolDefinition {
            name: "document_status_summary",
            description: "Get exact company document counts by processing status. Use for count questions.",
            input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        },
        AppToolDefinition {
            name: "list_documents",
            description: "List recent company documents with an exact total_count and compact returned items.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIST_LIMIT }
                },
                "additionalProperties": false
            }),
        },
        AppToolDefinition {
            name: "get_document",
            description: "Get compact status/details for one company document by short_ref.",
            input_schema: json!({
                "type": "object",
                "properties": { "short_ref": { "type": "string" } },
                "required": ["short_ref"],
                "additionalProperties": false
            }),
        },
        AppToolDefinition {
            name: "explain_document",
            description: "Explain why one company document is blocked, pending, failed, or ready. Reuses the same logic as /why.",
            input_schema: json!({
                "type": "object",
                "properties": { "short_ref": { "type": "string" } },
                "required": ["short_ref"],
                "additionalProperties": false
            }),
        },
        AppToolDefinition {
            name: "list_pending_reviews",
            description: "List company documents needing attention, with exact total_count and compact returned items.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIST_LIMIT }
                },
                "additionalProperties": false
            }),
        },
        AppToolDefinition {
            name: "list_export_ready",
            description: "List company documents ready for export, with exact total_count and compact returned items.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIST_LIMIT }
                },
                "additionalProperties": false
            }),
        },
    ]
}

pub fn read_only_inference_tools() -> Vec<InferenceToolDefinition> {
    read_only_tool_catalog()
        .into_iter()
        .map(InferenceToolDefinition::from)
        .collect()
}

pub fn mutating_prepare_tool_catalog() -> Vec<AppToolDefinition> {
    vec![
        AppToolDefinition {
            name: "prepare_open_review",
            description: "Prepare a confirmation prompt to open review actions for one document by short_ref. Use only when the user explicitly asks to review or open review for a document.",
            input_schema: json!({
                "type": "object",
                "properties": { "short_ref": { "type": "string" } },
                "required": ["short_ref"],
                "additionalProperties": false
            }),
        },
        AppToolDefinition {
            name: "prepare_retry_document",
            description: "Prepare a confirmation prompt to retry or reprocess one document by short_ref. Use only when the user explicitly asks to retry or reprocess.",
            input_schema: json!({
                "type": "object",
                "properties": { "short_ref": { "type": "string" } },
                "required": ["short_ref"],
                "additionalProperties": false
            }),
        },
        AppToolDefinition {
            name: "prepare_export_documents",
            description: "Prepare a confirmation prompt to export ready documents. Use only when the user explicitly asks to export documents.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "short_ref": { "type": "string" },
                    "date_from": { "type": "string" },
                    "date_to": { "type": "string" },
                    "document_types": { "type": "array", "items": { "type": "string" } },
                    "confidence_min": { "type": "number" }
                },
                "additionalProperties": false
            }),
        },
    ]
}

pub fn agent_inference_tools() -> Vec<InferenceToolDefinition> {
    read_only_tool_catalog()
        .into_iter()
        .chain(mutating_prepare_tool_catalog())
        .map(InferenceToolDefinition::from)
        .collect()
}

pub fn is_mutating_prepare_tool(name: &str) -> bool {
    matches!(
        name,
        "prepare_open_review" | "prepare_retry_document" | "prepare_export_documents"
    )
}

pub async fn execute_read_only_tool(
    pool: &DbPool,
    tool_call: &ReadOnlyToolCall,
) -> serde_json::Value {
    match execute_read_only_tool_inner(pool, tool_call).await {
        Ok(value) => json!({
            "ok": true,
            "tool": tool_call.name,
            "result": value,
        }),
        Err(err) => json!({
            "ok": false,
            "tool": tool_call.name,
            "error": err.to_string(),
        }),
    }
}

async fn execute_read_only_tool_inner(
    pool: &DbPool,
    tool_call: &ReadOnlyToolCall,
) -> anyhow::Result<serde_json::Value> {
    match tool_call.name.as_str() {
        "document_status_summary" => document_status_summary(pool).await,
        "list_documents" => {
            let limit = limit_arg(&tool_call.args);
            let total_count = count_documents(pool).await?;
            let documents = list_recent_documents(pool, limit).await?;
            Ok(document_list_result(total_count, documents))
        }
        "get_document" => {
            let short_ref = required_short_ref(&tool_call.args)?;
            let document = document_summary_by_short_ref(pool, &short_ref).await?;
            Ok(json!({
                "short_ref": short_ref,
                "document": document.as_ref().map(document_summary_json),
                "found": document.is_some(),
            }))
        }
        "explain_document" => {
            let short_ref = required_short_ref(&tool_call.args)?;
            let explanation = describe_document_why(pool, &short_ref).await?;
            Ok(json!({
                "short_ref": short_ref,
                "explanation": explanation,
            }))
        }
        "list_pending_reviews" => {
            let limit = limit_arg(&tool_call.args);
            let total_count = count_documents_requiring_attention(pool).await?;
            let documents = list_documents_requiring_attention(pool, limit).await?;
            Ok(document_list_result(total_count, documents))
        }
        "list_export_ready" => {
            let limit = limit_arg(&tool_call.args);
            let total_count = count_documents_by_status(pool, "EXPORT_READY").await?;
            let documents = list_documents_by_status(pool, "EXPORT_READY", limit).await?;
            Ok(document_list_result(total_count, documents))
        }
        other => Err(anyhow::anyhow!("unknown read-only tool: {other}")),
    }
}

async fn document_status_summary(pool: &DbPool) -> anyhow::Result<serde_json::Value> {
    let counts = document_status_counts(pool).await?;
    Ok(json!({
        "processing": counts.processing_count,
        "pending_review": counts.pending_count,
        "export_ready": counts.ready_count,
        "exported": counts.exported_count,
        "failed": counts.failed_count,
    }))
}

fn document_list_result(total_count: i64, documents: Vec<DocumentSummary>) -> serde_json::Value {
    let returned_count = documents.len();
    json!({
        "total_count": total_count,
        "returned_count": returned_count,
        "items": documents.iter().map(document_summary_json).collect::<Vec<_>>(),
    })
}

fn document_summary_json(document: &DocumentSummary) -> serde_json::Value {
    json!({
        "short_ref": document.short_ref,
        "status": document.status,
        "supplier_name": document.supplier_name,
        "invoice_date": document.invoice_date,
        "total_amount": document.total_amount,
        "review_reason": document.review_reason,
    })
}

fn limit_arg(args: &serde_json::Value) -> i64 {
    args.get("limit")
        .and_then(|value| value.as_i64())
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .clamp(1, MAX_LIST_LIMIT)
}

fn required_short_ref(args: &serde_json::Value) -> anyhow::Result<String> {
    let raw = args
        .get("short_ref")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required short_ref"))?;
    normalize_short_ref(raw).ok_or_else(|| anyhow::anyhow!("invalid short_ref: {raw}"))
}

fn normalize_tool_arguments(args: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    match args {
        serde_json::Value::Null => Ok(json!({})),
        serde_json::Value::Object(_) => Ok(args.clone()),
        serde_json::Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Ok(json!({}))
            } else {
                serde_json::from_str(trimmed)
                    .map_err(|err| anyhow::anyhow!("invalid tool argument JSON: {err}"))
            }
        }
        other => Err(anyhow::anyhow!("tool arguments must be an object: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_native_ollama_tool_call() {
        let native = ToolCall {
            function: crate::inference::ToolCallFunction {
                name: "get_document".to_string(),
                arguments: json!({ "short_ref": "D57" }),
            },
        };
        let tool_call = ReadOnlyToolCall::try_from(&native).expect("tool call");

        assert_eq!(
            tool_call,
            ReadOnlyToolCall {
                name: "get_document".to_string(),
                args: json!({ "short_ref": "D57" }),
            }
        );
    }

    #[test]
    fn native_tool_catalog_uses_ollama_function_shape() {
        let tools = agent_inference_tools();
        let status = tools
            .iter()
            .find(|tool| tool.function.name == "document_status_summary")
            .expect("status tool");

        assert_eq!(status.tool_type, "function");
        assert_eq!(status.function.parameters["type"], "object");
    }

    #[test]
    fn agent_tool_catalog_includes_prepare_tools() {
        let tools = agent_inference_tools();
        assert!(
            tools
                .iter()
                .any(|tool| tool.function.name == "prepare_retry_document")
        );
        assert!(is_mutating_prepare_tool("prepare_export_documents"));
        assert!(!is_mutating_prepare_tool("list_documents"));
    }

    #[test]
    fn validates_required_short_ref() {
        let err = required_short_ref(&json!({})).expect_err("missing ref");
        assert!(err.to_string().contains("missing required short_ref"));

        let short_ref = required_short_ref(&json!({ "short_ref": "57" })).expect("short ref");
        assert_eq!(short_ref, "D000057");
    }

    #[test]
    fn clamps_list_limits() {
        assert_eq!(limit_arg(&json!({ "limit": 100 })), MAX_LIST_LIMIT);
        assert_eq!(limit_arg(&json!({ "limit": 0 })), 1);
        assert_eq!(limit_arg(&json!({})), DEFAULT_LIST_LIMIT);
    }

    #[test]
    fn document_list_result_preserves_exact_total_count() {
        let value = document_list_result(
            100,
            vec![DocumentSummary {
                id: 1,
                short_ref: "D000001".to_string(),
                status: "PENDING_HUMAN_REVIEW".to_string(),
                supplier_name: Some("Supplier AB".to_string()),
                invoice_date: None,
                total_amount: None,
                confidence_score: None,
                review_reason: None,
            }],
        );

        assert_eq!(value["total_count"], 100);
        assert_eq!(value["returned_count"], 1);
    }

    #[test]
    fn normalizes_tool_arguments_from_null_object_and_json_string() {
        assert_eq!(
            normalize_tool_arguments(&serde_json::Value::Null).unwrap(),
            json!({})
        );
        assert_eq!(
            normalize_tool_arguments(&json!({ "limit": 5 })).unwrap(),
            json!({ "limit": 5 })
        );
        assert_eq!(
            normalize_tool_arguments(&json!(r#"{ "short_ref": "D57" }"#)).unwrap(),
            json!({ "short_ref": "D57" })
        );
        assert_eq!(normalize_tool_arguments(&json!("   ")).unwrap(), json!({}));
    }

    #[test]
    fn rejects_invalid_tool_arguments() {
        let invalid_json =
            normalize_tool_arguments(&json!("{ not json }")).expect_err("invalid JSON string");
        assert!(
            invalid_json
                .to_string()
                .contains("invalid tool argument JSON")
        );

        let array = normalize_tool_arguments(&json!([])).expect_err("array should fail");
        assert!(
            array
                .to_string()
                .contains("tool arguments must be an object")
        );
    }

    #[test]
    fn read_only_tool_call_accepts_json_string_arguments() {
        let native = ToolCall {
            function: crate::inference::ToolCallFunction {
                name: "get_document".to_string(),
                arguments: json!(r#"{ "short_ref": "D57" }"#),
            },
        };

        let tool_call = ReadOnlyToolCall::try_from(&native).expect("tool call");

        assert_eq!(tool_call.args, json!({ "short_ref": "D57" }));
    }
}
