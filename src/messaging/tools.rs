use std::path::{Component, Path, PathBuf};

use crate::db::DbPool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::document_state::fetch_document_state_by_short_ref;
use crate::inference::{ToolCall, ToolDefinition as InferenceToolDefinition, ToolFunction};
use crate::query::{
    DocumentSummary, count_accounting_eligible_documents, count_documents,
    count_documents_by_accounting_status, count_documents_requiring_attention,
    document_summary_by_short_ref, grouped_document_status_counts,
    list_accounting_eligible_documents, list_documents_by_accounting_status,
    list_documents_requiring_attention, list_recent_documents,
};
use crate::skills::SkillRegistry;

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

#[derive(Debug, Clone)]
pub(crate) struct ToolExecutionOutput {
    // This is the post-execution payload that gets fed back into the model for
    // read-only reasoning turns. `result_description` stays out of the lean
    // pre-call tool catalog and only appears after the tool has actually run.
    pub result: serde_json::Value,
    pub result_description: Option<String>,
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
            description: "Get the current live company document totals and grouped status counts. `documents_total` is all documents in the system. `intake.ingested` is all successfully ingested documents, even if they have already moved into accounting. Accounting counts show the current downstream accounting states. Use this for status, live status, current status, or count questions, and do not answer those from memory when this tool is available.",
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
            description: "Get compact status/details for one company document by document_short_ref.",
            input_schema: json!({
                "type": "object",
                "properties": { "document_short_ref": { "type": "string" } },
                "required": ["document_short_ref"],
                "additionalProperties": false
            }),
        },
        AppToolDefinition {
            name: "explain_document",
            description: "Explain why one company document is blocked, pending, failed, or ready.",
            input_schema: json!({
                "type": "object",
                "properties": { "document_short_ref": { "type": "string" } },
                "required": ["document_short_ref"],
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
        AppToolDefinition {
            name: "list_accounting_eligible_documents",
            description: "List company documents that are ingested and can now be sent to accounting, with exact total_count and compact returned items.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIST_LIMIT }
                },
                "additionalProperties": false
            }),
        },
        AppToolDefinition {
            name: "skill_view",
            description: "View full content of a skill by name, or load a specific supporting file such as a reference or template by path.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "path": { "type": "string" }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
        },
        AppToolDefinition {
            name: "skill_list",
            description: "List all available skills with their descriptions.",
            input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
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
            description: "Prepare a confirmation prompt to open review actions for one document by document_short_ref. Use only when the user explicitly asks to review or open review for a document. This is the only valid way to create a review confirmation boundary; do not write your own confirmation prompt in prose.",
            input_schema: json!({
                "type": "object",
                "properties": { "document_short_ref": { "type": "string" } },
                "required": ["document_short_ref"],
                "additionalProperties": false
            }),
        },
        AppToolDefinition {
            name: "prepare_retry_document",
            description: "Prepare a confirmation prompt to retry or reprocess one document by document_short_ref. Use only when the user explicitly asks to retry or reprocess. This is the only valid way to create a retry or reprocess confirmation boundary; do not write your own confirmation prompt in prose.",
            input_schema: json!({
                "type": "object",
                "properties": { "document_short_ref": { "type": "string" } },
                "required": ["document_short_ref"],
                "additionalProperties": false
            }),
        },
        AppToolDefinition {
            name: "prepare_export_documents",
            description: "Prepare a confirmation prompt to export ready documents. Use only when the user explicitly asks to export documents. This is the only valid way to create an export confirmation boundary; do not write your own confirmation prompt in prose.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "document_short_ref": { "type": "string" },
                    "date_from": { "type": "string" },
                    "date_to": { "type": "string" },
                    "document_types": { "type": "array", "items": { "type": "string" } },
                    "confidence_min": { "type": "number" }
                },
                "additionalProperties": false
            }),
        },
        AppToolDefinition {
            name: "prepare_process_accounting_documents",
            description: "Prepare a confirmation prompt to start accounting processing for one, several, or all ingested documents. Use only when the user explicitly asks to process documents for accounting. This is the only valid way to create an accounting-processing confirmation boundary; do not write your own confirmation prompt in prose.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "document_short_ref": { "type": "string" },
                    "document_short_refs": { "type": "array", "items": { "type": "string" } },
                    "all_eligible": { "type": "boolean" }
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
        "prepare_open_review"
            | "prepare_retry_document"
            | "prepare_export_documents"
            | "prepare_process_accounting_documents"
    )
}

pub async fn execute_read_only_tool(
    pool: &DbPool,
    skills_registry: Option<&SkillRegistry>,
    tool_call: &ReadOnlyToolCall,
) -> serde_json::Value {
    match execute_read_only_tool_inner(pool, skills_registry, tool_call).await {
        Ok(output) => {
            let mut envelope = json!({
                "ok": true,
                "tool": tool_call.name,
                "result": output.result,
            });
            if let Some(result_description) = output.result_description {
                envelope["result_description"] = json!(result_description);
            }
            envelope
        }
        Err(err) => json!({
            "ok": false,
            "tool": tool_call.name,
            "error": err.to_string(),
        }),
    }
}

async fn execute_read_only_tool_inner(
    pool: &DbPool,
    skills_registry: Option<&SkillRegistry>,
    tool_call: &ReadOnlyToolCall,
) -> anyhow::Result<ToolExecutionOutput> {
    match tool_call.name.as_str() {
        "document_status_summary" => document_status_summary(pool).await,
        "list_documents" => {
            let limit = limit_arg(&tool_call.args);
            let total_count = count_documents(pool).await?;
            let documents = list_recent_documents(pool, limit).await?;
            Ok(ToolExecutionOutput {
                result: document_list_result(total_count, documents),
                result_description: Some("The result contains a recent-document list response. total_count is the exact number of documents in this tool's current query scope. returned_count is the number of document rows included in this response. items contains only the returned document rows. For each item, document_short_ref is the stable user-facing document reference. status.intake.status is the current intake state for that document. status.accounting.status is the current accounting state for that document. status.accounting.review_reason is the current review reason when present. supplier_name, invoice_date, and total_amount are the visible document business fields currently available. If you answer from this result, state the total_count clearly and preserve document_short_ref values exactly when summarizing or listing rows.".to_string()),
            })
        }
        "get_document" => {
            let short_ref = required_document_short_ref(&tool_call.args)?;
            let document = document_summary_by_short_ref(pool, &short_ref).await?;
            let document_state = fetch_document_state_by_short_ref(pool, &short_ref).await?;
            Ok(ToolExecutionOutput {
                result: json!({
                    "document_short_ref": short_ref,
                    "document": document.as_ref().map(document_summary_json),
                    "status": document_state.as_ref().map(document_state_json),
                    "found": document.is_some(),
                }),
                result_description: Some("The result contains one document's current visible state and details. document_short_ref identifies the exact document. status.intake.status is the current intake state. status.accounting.status is the current accounting state. status.accounting.review_reason is the current review reason when present. supplier_name, invoice_date, and total_amount are the visible document business fields currently available. If you answer from this result, stay focused on this single document and mirror the returned status and document_short_ref closely.".to_string()),
            })
        }
        "explain_document" => {
            let short_ref = required_document_short_ref(&tool_call.args)?;
            let explanation = describe_document_why(pool, &short_ref).await?;
            Ok(ToolExecutionOutput {
                result: json!({
                    "document_short_ref": short_ref,
                    "explanation": explanation,
                }),
                result_description: Some("The result contains a document-specific explanation. document_short_ref identifies the document being explained. explanation is the current blocker, pending reason, failure reason, or readiness reason for that document. If you answer from this result, use the explanation directly and keep the answer focused on that one document.".to_string()),
            })
        }
        "list_pending_reviews" => {
            let limit = limit_arg(&tool_call.args);
            let total_count = count_documents_requiring_attention(pool).await?;
            let documents = list_documents_requiring_attention(pool, limit).await?;
            Ok(ToolExecutionOutput {
                result: document_list_result(total_count, documents),
                result_description: Some("The result contains the pending-review document subset. total_count is the exact number of documents currently in the pending-review subset for this query scope. returned_count is the number of returned rows in this response. items contains only the returned pending-review documents. For each item, document_short_ref is the stable user-facing document reference. status.intake.status is the current intake state. status.accounting.status is the current accounting state. status.accounting.review_reason is the current review reason when present. supplier_name, invoice_date, and total_amount are the visible document business fields currently available. If you answer from this result, state the total_count clearly and preserve document_short_ref values exactly when listing or summarizing the returned review candidates.".to_string()),
            })
        }
        "list_export_ready" => {
            let limit = limit_arg(&tool_call.args);
            let total_count =
                count_documents_by_accounting_status(pool, "READY_FOR_EXPORT").await?;
            let documents =
                list_documents_by_accounting_status(pool, "READY_FOR_EXPORT", limit).await?;
            Ok(ToolExecutionOutput {
                result: document_list_result(total_count, documents),
                result_description: Some("The result contains the ready-for-export document subset. total_count is the exact number of documents currently ready for export in this query scope. returned_count is the number of returned rows in this response. items contains only the returned ready-for-export documents. For each item, document_short_ref is the stable user-facing document reference. status.intake.status is the current intake state. status.accounting.status is the current accounting state. status.accounting.review_reason is the current review reason when present. supplier_name, invoice_date, and total_amount are the visible document business fields currently available. If you answer from this result, state the total_count clearly and preserve document_short_ref values exactly when listing or summarizing the returned export-ready documents.".to_string()),
            })
        }
        "list_accounting_eligible_documents" => {
            let limit = limit_arg(&tool_call.args);
            let total_count = count_accounting_eligible_documents(pool).await?;
            let documents = list_accounting_eligible_documents(pool, limit).await?;
            Ok(ToolExecutionOutput {
                result: document_list_result(total_count, documents),
                result_description: Some("The result contains the accounting-eligible document subset. total_count is the exact number of documents currently eligible to be sent to accounting in this query scope. returned_count is the number of returned rows in this response. items contains only the returned eligible documents. For each item, document_short_ref is the stable user-facing document reference. status.intake.status is the current intake state. status.accounting.status is the current accounting state. status.accounting.review_reason is the current review reason when present. supplier_name, invoice_date, and total_amount are the visible document business fields currently available. If you answer from this result, make it explicit that these are eligible-for-accounting documents, state the total_count clearly, and preserve document_short_ref values exactly when listing or summarizing the returned items.".to_string()),
            })
        }
        "skill_view" => {
            let name = required_skill_name(&tool_call.args)?;
            let path = optional_skill_path(&tool_call.args)?;
            let registry = required_skills_registry(skills_registry)?;
            read_skill_view_with_registry(registry, &name, path.as_deref()).await
        }
        "skill_list" => {
            let registry = required_skills_registry(skills_registry)?;
            read_skill_list_with_registry(registry).await
        }
        other => Err(anyhow::anyhow!("unknown read-only tool: {other}")),
    }
}

async fn document_status_summary(pool: &DbPool) -> anyhow::Result<ToolExecutionOutput> {
    let counts = grouped_document_status_counts(pool).await?;
    Ok(ToolExecutionOutput {
        result: json!({
            "documents_total": counts.documents_total,
            "intake": {
                "processing": counts.intake_processing_count,
                "ingested": counts.intake_ingested_count,
                "failed": counts.intake_failed_count,
            },
            "accounting": {
                "processing": counts.accounting_processing_count,
                "pending_review": counts.accounting_pending_review_count,
                "ready_for_export": counts.accounting_ready_for_export_count,
                "exported": counts.accounting_exported_count,
                "failed": counts.accounting_failed_count,
            },
        }),
        result_description: Some("The result contains grouped live workspace document counts. documents_total is all documents currently in the system. intake.processing is documents still in intake processing. intake.ingested is all successfully ingested documents, including documents that may already have progressed into accounting. intake.failed is documents that failed during intake. accounting.processing is documents currently active in accounting work. accounting.pending_review is documents currently waiting for human review. accounting.ready_for_export is documents completed in accounting and ready to export. accounting.exported is documents already exported. accounting.failed is documents failed in the accounting domain. If you answer from this result, report every returned count explicitly and do not collapse intake.ingested into prose.".to_string()),
    })
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
        "document_short_ref": document.short_ref,
        "status": {
            "intake": {
                "status": document.intake_status,
            },
            "accounting": {
                "status": document.accounting_status,
                "review_reason": document.review_reason,
            },
        },
        "supplier_name": document.supplier_name,
        "invoice_date": document.invoice_date,
        "total_amount": document.total_amount,
    })
}

fn document_state_json(
    snapshot: &crate::document_state::DocumentStateSnapshot,
) -> serde_json::Value {
    json!({
        "intake": {
            "status": snapshot.intake_status,
            "started_at": snapshot.intake_started_at,
            "completed_at": snapshot.intake_completed_at,
            "failure_reason": snapshot.intake_failure_reason,
        },
        "accounting": {
            "status": snapshot.accounting_status,
            "requested_at": snapshot.accounting_requested_at,
            "started_at": snapshot.accounting_started_at,
            "completed_at": snapshot.accounting_completed_at,
            "failure_reason": snapshot.accounting_failure_reason,
            "review_reason": snapshot.accounting_review_reason,
            "export_batch_id": snapshot.accounting_export_batch_id,
            "latest_run_kind": snapshot.latest_accounting_run_kind,
            "latest_run_status": snapshot.latest_accounting_run_status,
        },
    })
}

fn limit_arg(args: &serde_json::Value) -> i64 {
    args.get("limit")
        .and_then(|value| value.as_i64())
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .clamp(1, MAX_LIST_LIMIT)
}

fn required_document_short_ref(args: &serde_json::Value) -> anyhow::Result<String> {
    let raw = args
        .get("document_short_ref")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required document_short_ref"))?;
    normalize_short_ref(raw).ok_or_else(|| anyhow::anyhow!("invalid document_short_ref: {raw}"))
}

fn required_skill_name(args: &serde_json::Value) -> anyhow::Result<String> {
    args.get("name")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required skill name"))
        .map(|s| s.to_string())
}

fn optional_skill_path(args: &serde_json::Value) -> anyhow::Result<Option<String>> {
    match args.get("path") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(path)) => {
            let trimmed = path.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Some(other) => Err(anyhow::anyhow!("skill path must be a string: {other}")),
    }
}

fn required_skills_registry(
    skills_registry: Option<&SkillRegistry>,
) -> anyhow::Result<&SkillRegistry> {
    if skills_registry.is_some() {
        tracing::debug!("Using skills registry for skill tool request");
    } else {
        tracing::debug!("Skill tool request attempted without a skills registry");
    }
    skills_registry.ok_or_else(|| anyhow::anyhow!("skills registry unavailable"))
}

async fn read_skill_view_with_registry(
    registry: &SkillRegistry,
    name: &str,
    path: Option<&str>,
) -> anyhow::Result<ToolExecutionOutput> {
    tracing::debug!(
        requested_skill_name = name,
        requested_supporting_path = path,
        "Loading skill material from registry for skill_view"
    );
    let skill = registry.get_skill_by_name(name).await?;

    if let Some(requested_path) = path {
        return read_skill_supporting_file(&skill, requested_path);
    }

    tracing::info!(
        skill_id = %skill.id,
        skill_name = skill.name(),
        reference_count = skill.references.len(),
        template_count = skill.templates.len(),
        reference_filenames = ?skill
            .references
            .iter()
            .map(|reference| reference.name.clone())
            .collect::<Vec<_>>(),
        template_names = ?skill
            .templates
            .iter()
            .map(|template| template.name.clone())
            .collect::<Vec<_>>(),
        "Loaded skill instructions from registry through skill_view"
    );
    tracing::debug!(
        skill_id = %skill.id,
        skill_name = skill.name(),
        reference_paths = ?skill
            .references
            .iter()
            .filter_map(|reference| reference_relative_path(&skill, reference.path.as_path()))
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>(),
        template_paths = ?skill
            .templates
            .iter()
            .filter_map(|template| template_relative_path(&skill, template.path.as_path()))
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>(),
        "Loaded skill manifest from registry"
    );
    Ok(ToolExecutionOutput {
        result: skill_view_result(&skill),
        result_description: Some("The result contains one skill's main instructions and supporting-file manifest. id is the stable skill package identifier. name is the skill display name. description is the skill summary. category identifies the skill grouping when present. content is the main SKILL.md body. references lists available supporting reference files and templates lists available supporting template files; these arrays are manifests, not the full bodies of those files. If you answer from this result, treat content as the main skill instructions and do not imply that listed references or templates have already been loaded in full.".to_string()),
    })
}

fn read_skill_supporting_file(
    skill: &crate::skills::types::Skill,
    requested_path: &str,
) -> anyhow::Result<ToolExecutionOutput> {
    let normalized = normalize_skill_supporting_path(requested_path)?;
    let kind = normalized
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("invalid skill supporting file path: {requested_path}"))?;

    if kind == "references"
        && let Some(reference) = skill.references.iter().find(|reference| {
            reference_relative_path(skill, reference.path.as_path())
                .is_some_and(|relative| relative == normalized)
        })
    {
        tracing::info!(
            skill_id = %skill.id,
            skill_name = skill.name(),
            requested_path,
            file_kind = "reference",
            content_length = reference.content.len(),
            "Loaded skill supporting file from registry through skill_view"
        );
        tracing::debug!(
            skill_id = %skill.id,
            skill_name = skill.name(),
            requested_path,
            resolved_path = %normalized.to_string_lossy(),
            file_kind = "reference",
            file_name = reference.name.as_str(),
            content_length = reference.content.len(),
            "Resolved reference file request from skill registry"
        );
        return Ok(ToolExecutionOutput {
            result: json!({
                "id": skill.id.0,
                "skill_name": skill.metadata.name,
                "requested_path": requested_path,
                "resolved_path": normalized.to_string_lossy(),
                "file_kind": "reference",
                "name": reference.name,
                "content": reference.content,
            }),
            result_description: Some("The result contains one loaded supporting file for a skill. id identifies the parent skill package. skill_name is the skill display name. requested_path is the path requested by the assistant. resolved_path is the normalized supporting-file path that was actually loaded. file_kind identifies whether the file is a reference or a template. name is the supporting file name. content is the full body of that supporting file. If you answer from this result, treat content as the loaded file body and do not imply that other supporting files were also loaded.".to_string()),
        });
    }

    if kind == "templates"
        && let Some(template) = skill.templates.iter().find(|template| {
            template_relative_path(skill, template.path.as_path())
                .is_some_and(|relative| relative == normalized)
        })
    {
        tracing::info!(
            skill_id = %skill.id,
            skill_name = skill.name(),
            requested_path,
            file_kind = "template",
            content_length = template.content.len(),
            "Loaded skill supporting file from registry through skill_view"
        );
        tracing::debug!(
            skill_id = %skill.id,
            skill_name = skill.name(),
            requested_path,
            resolved_path = %normalized.to_string_lossy(),
            file_kind = "template",
            file_name = template.name.as_str(),
            content_length = template.content.len(),
            "Resolved template file request from skill registry"
        );
        return Ok(ToolExecutionOutput {
            result: json!({
                "id": skill.id.0,
                "skill_name": skill.metadata.name,
                "requested_path": requested_path,
                "resolved_path": normalized.to_string_lossy(),
                "file_kind": "template",
                "name": template.name,
                "content": template.content,
            }),
            result_description: Some("The result contains one loaded supporting file for a skill. id identifies the parent skill package. skill_name is the skill display name. requested_path is the path requested by the assistant. resolved_path is the normalized supporting-file path that was actually loaded. file_kind identifies whether the file is a reference or a template. name is the supporting file name. content is the full body of that supporting file. If you answer from this result, treat content as the loaded file body and do not imply that other supporting files were also loaded.".to_string()),
        });
    }

    Err(anyhow::anyhow!(
        "skill supporting file not found or not allowed: {requested_path}"
    ))
}

fn normalize_skill_supporting_path(requested_path: &str) -> anyhow::Result<PathBuf> {
    let path = Path::new(requested_path);

    if path.is_absolute() {
        return Err(anyhow::anyhow!(
            "absolute skill supporting file paths are not allowed: {requested_path}"
        ));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => normalized.push(segment),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(anyhow::anyhow!(
                    "path traversal is not allowed in skill supporting file paths: {requested_path}"
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow::anyhow!(
                    "invalid skill supporting file path: {requested_path}"
                ));
            }
        }
    }

    let mut components = normalized.components();
    let root = components
        .next()
        .and_then(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("invalid skill supporting file path: {requested_path}"))?;

    if root != "references" && root != "templates" {
        return Err(anyhow::anyhow!(
            "skill supporting file path must be under references/ or templates/: {requested_path}"
        ));
    }

    if components.next().is_none() {
        return Err(anyhow::anyhow!(
            "skill supporting file path must include a file name: {requested_path}"
        ));
    }

    Ok(normalized)
}

fn reference_relative_path(
    skill: &crate::skills::types::Skill,
    reference_path: &Path,
) -> Option<PathBuf> {
    reference_path
        .strip_prefix(&skill.path)
        .ok()
        .map(Path::to_path_buf)
}

fn template_relative_path(
    skill: &crate::skills::types::Skill,
    template_path: &Path,
) -> Option<PathBuf> {
    template_path
        .strip_prefix(&skill.path)
        .ok()
        .map(Path::to_path_buf)
}

async fn read_skill_list_with_registry(
    registry: &SkillRegistry,
) -> anyhow::Result<ToolExecutionOutput> {
    let skills = registry.get_all_skills().await;
    tracing::info!(
        skill_count = skills.len(),
        skill_names = ?skills.iter().map(|skill| skill.name().to_string()).collect::<Vec<_>>(),
        "Loaded skill list from registry through skill_list"
    );
    Ok(ToolExecutionOutput {
        result: skill_list_result_from_arcs(&skills),
        result_description: Some("The result contains the available skill catalog. items contains the loaded skills available to the assistant. For each item, name is the skill's display name, description explains the skill's intended domain or workflow, and category identifies the skill grouping when present. If you answer from this result, treat it as a catalog summary and do not describe it as the full instructions of a skill.".to_string()),
    })
}

fn skill_view_result(skill: &crate::skills::types::Skill) -> serde_json::Value {
    json!({
        "id": skill.id.0,
        "name": skill.metadata.name,
        "description": skill.metadata.description,
        "category": skill.metadata.category,
        "version": skill.metadata.version,
        "author": skill.metadata.author,
        "keywords": skill.metadata.keywords,
        "depends_on": skill.metadata.depends_on,
        "references": skill.references.iter().map(|r| json!({
            "name": r.name,
            "path": reference_relative_path(skill, r.path.as_path())
                .map(|path| path.to_string_lossy().to_string()),
        })).collect::<Vec<_>>(),
        "templates": skill.templates.iter().map(|t| json!({
            "name": t.name,
            "path": template_relative_path(skill, t.path.as_path())
                .map(|path| path.to_string_lossy().to_string()),
        })).collect::<Vec<_>>(),
        "content": skill.content,
        "loaded_at": skill.loaded_at,
    })
}

fn skill_list_result_from_arcs(
    skills: &[std::sync::Arc<crate::skills::types::Skill>],
) -> serde_json::Value {
    let total_count = skills.len();
    json!({
        "total_count": total_count,
        "items": skills.iter().map(|s| json!({
            "id": s.id.0,
            "name": s.metadata.name,
            "description": s.metadata.description,
            "category": s.metadata.category,
            "version": s.metadata.version,
        })).collect::<Vec<_>>(),
    })
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
    use crate::skills::SkillRegistry;
    use std::fs;
    use tempfile::TempDir;

    fn create_temp_skill_root() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn write_skill_fixture(
        root: &std::path::Path,
        skill_dir_name: &str,
        skill_md: &str,
        references: &[(&str, &str)],
        templates: &[(&str, &str)],
    ) {
        let skill_dir = root.join(skill_dir_name);
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(skill_dir.join("SKILL.md"), skill_md).expect("write SKILL.md");

        if !references.is_empty() {
            let references_dir = skill_dir.join("references");
            fs::create_dir_all(&references_dir).expect("create references dir");
            for (name, content) in references {
                fs::write(references_dir.join(name), content).expect("write reference");
            }
        }

        if !templates.is_empty() {
            let templates_dir = skill_dir.join("templates");
            fs::create_dir_all(&templates_dir).expect("create templates dir");
            for (name, content) in templates {
                fs::write(templates_dir.join(name), content).expect("write template");
            }
        }
    }

    #[test]
    fn converts_native_ollama_tool_call() {
        let native = ToolCall {
            function: crate::inference::ToolCallFunction {
                name: "get_document".to_string(),
                arguments: json!({ "document_short_ref": "D57" }),
            },
        };
        let tool_call = ReadOnlyToolCall::try_from(&native).expect("tool call");

        assert_eq!(
            tool_call,
            ReadOnlyToolCall {
                name: "get_document".to_string(),
                args: json!({ "document_short_ref": "D57" }),
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
        assert!(
            tools
                .iter()
                .any(|tool| tool.function.name == "prepare_process_accounting_documents")
        );
        assert!(is_mutating_prepare_tool("prepare_export_documents"));
        assert!(is_mutating_prepare_tool(
            "prepare_process_accounting_documents"
        ));
        assert!(!is_mutating_prepare_tool("list_documents"));
    }

    #[test]
    fn validates_required_document_short_ref() {
        let err = required_document_short_ref(&json!({})).expect_err("missing ref");
        assert!(
            err.to_string()
                .contains("missing required document_short_ref")
        );

        let short_ref =
            required_document_short_ref(&json!({ "document_short_ref": "57" })).expect("short ref");
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
                intake_status: Some("INGESTED".to_string()),
                accounting_status: Some("PENDING_REVIEW".to_string()),
                supplier_name: Some("Supplier AB".to_string()),
                invoice_date: None,
                total_amount: None,
                confidence_score: None,
                review_reason: Some("Awaiting human review".to_string()),
            }],
        );

        assert_eq!(value["total_count"], 100);
        assert_eq!(value["returned_count"], 1);
    }

    #[tokio::test]
    async fn list_accounting_eligible_documents_returns_only_vision_complete_unrequested_items() {
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

        for preset in [
            crate::document_state::TestDocumentStatePreset::IntakeIngested,
            crate::document_state::TestDocumentStatePreset::AccountingRequested,
            crate::document_state::TestDocumentStatePreset::AccountingRunning,
        ] {
            let document_id: i64 = sqlx::query_scalar(
                r#"
                INSERT INTO documents (filename, file_hash, original_path, mime_type)
                VALUES ('doc.pdf', $1, '/tmp/doc.pdf', 'application/pdf')
                RETURNING id
                "#,
            )
            .bind(format!("eligible-{}", uuid::Uuid::new_v4()))
            .fetch_one(&pool)
            .await
            .expect("insert document");
            crate::document_state::seed_document_state_preset(&pool, document_id, preset)
                .await
                .expect("seed document status");
        }

        let value = execute_read_only_tool(
            &pool,
            None,
            &ReadOnlyToolCall {
                name: "list_accounting_eligible_documents".to_string(),
                args: json!({ "limit": 10 }),
            },
        )
        .await;

        assert_eq!(value["ok"], true);
        assert!(
            value["result_description"]
                .as_str()
                .expect("result_description")
                .contains("document_short_ref")
        );
        assert_eq!(value["result"]["total_count"], 1);
        assert_eq!(value["result"]["returned_count"], 1);
        assert_eq!(
            value["result"]["items"][0]["status"],
            json!({
                "intake": {
                    "status": "INGESTED"
                },
                "accounting": {
                    "status": "NOT_REQUESTED",
                    "review_reason": null
                }
            })
        );
    }

    #[tokio::test]
    async fn document_status_summary_returns_grouped_domain_counts() {
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

        for preset in [
            crate::document_state::TestDocumentStatePreset::IntakeReceived,
            crate::document_state::TestDocumentStatePreset::PendingReview,
            crate::document_state::TestDocumentStatePreset::ReadyForExport,
            crate::document_state::TestDocumentStatePreset::Exported,
            crate::document_state::TestDocumentStatePreset::IntakeFailed,
        ] {
            let document_id: i64 = sqlx::query_scalar(
                r#"
                INSERT INTO documents (filename, file_hash, original_path, mime_type)
                VALUES ('doc.pdf', $1, '/tmp/doc.pdf', 'application/pdf')
                RETURNING id
                "#,
            )
            .bind(format!("status-{}", uuid::Uuid::new_v4()))
            .fetch_one(&pool)
            .await
            .expect("insert document");
            crate::document_state::seed_document_state_preset(&pool, document_id, preset)
                .await
                .expect("seed document status");
        }

        let value = execute_read_only_tool(
            &pool,
            None,
            &ReadOnlyToolCall {
                name: "document_status_summary".to_string(),
                args: json!({}),
            },
        )
        .await;

        assert_eq!(value["ok"], true);
        let description = value["result_description"]
            .as_str()
            .expect("result_description");
        assert!(description.contains("documents_total"));
        assert!(description.contains("intake.ingested"));
        assert!(description.contains("report every returned count explicitly"));
        assert_eq!(value["result"]["documents_total"], 5);
        assert_eq!(value["result"]["intake"]["processing"], 0);
        assert_eq!(value["result"]["intake"]["ingested"], 3);
        assert_eq!(value["result"]["intake"]["failed"], 1);
        assert_eq!(value["result"]["accounting"]["processing"], 0);
        assert_eq!(value["result"]["accounting"]["pending_review"], 1);
        assert_eq!(value["result"]["accounting"]["ready_for_export"], 1);
        assert_eq!(value["result"]["accounting"]["exported"], 1);
        assert_eq!(value["result"]["accounting"]["failed"], 0);
    }

    #[tokio::test]
    async fn document_status_summary_reports_intake_and_accounting_processing_separately() {
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

        for preset in [
            crate::document_state::TestDocumentStatePreset::IntakeReceived,
            crate::document_state::TestDocumentStatePreset::AccountingRunning,
        ] {
            let document_id: i64 = sqlx::query_scalar(
                r#"
                INSERT INTO documents (filename, file_hash, original_path, mime_type)
                VALUES ('doc.pdf', $1, '/tmp/doc.pdf', 'application/pdf')
                RETURNING id
                "#,
            )
            .bind(format!("fallback-{}", uuid::Uuid::new_v4()))
            .fetch_one(&pool)
            .await
            .expect("insert document");
            crate::document_state::seed_document_state_preset(&pool, document_id, preset)
                .await
                .expect("seed document status");
        }

        let value = execute_read_only_tool(
            &pool,
            None,
            &ReadOnlyToolCall {
                name: "document_status_summary".to_string(),
                args: json!({}),
            },
        )
        .await;

        assert_eq!(value["ok"], true);
        assert!(
            value["result_description"]
                .as_str()
                .expect("result_description")
                .contains("intake.ingested")
        );
        assert_eq!(value["result"]["documents_total"], 2);
        assert_eq!(value["result"]["intake"]["processing"], 0);
        assert_eq!(value["result"]["intake"]["ingested"], 1);
        assert_eq!(value["result"]["accounting"]["processing"], 1);
    }

    #[tokio::test]
    async fn document_status_summary_counts_all_ingested_documents_even_after_accounting_progress()
    {
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

        for preset in [
            crate::document_state::TestDocumentStatePreset::IntakeIngested,
            crate::document_state::TestDocumentStatePreset::IntakeIngested,
            crate::document_state::TestDocumentStatePreset::Exported,
        ] {
            let document_id: i64 = sqlx::query_scalar(
                r#"
                INSERT INTO documents (filename, file_hash, original_path, mime_type)
                VALUES ('doc.pdf', $1, '/tmp/doc.pdf', 'application/pdf')
                RETURNING id
                "#,
            )
            .bind(format!("ingested-{}", uuid::Uuid::new_v4()))
            .fetch_one(&pool)
            .await
            .expect("insert document");
            crate::document_state::seed_document_state_preset(&pool, document_id, preset)
                .await
                .expect("seed document status");
        }

        let value = execute_read_only_tool(
            &pool,
            None,
            &ReadOnlyToolCall {
                name: "document_status_summary".to_string(),
                args: json!({}),
            },
        )
        .await;

        assert_eq!(value["ok"], true);
        assert!(
            value["result_description"]
                .as_str()
                .expect("result_description")
                .contains("documents currently in the system")
        );
        assert_eq!(value["result"]["documents_total"], 3);
        assert_eq!(value["result"]["intake"]["processing"], 0);
        assert_eq!(value["result"]["intake"]["ingested"], 3);
        assert_eq!(value["result"]["intake"]["failed"], 0);
        assert_eq!(value["result"]["accounting"]["processing"], 0);
        assert_eq!(value["result"]["accounting"]["pending_review"], 0);
        assert_eq!(value["result"]["accounting"]["ready_for_export"], 0);
        assert_eq!(value["result"]["accounting"]["exported"], 1);
        assert_eq!(value["result"]["accounting"]["failed"], 0);
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
            normalize_tool_arguments(&json!(r#"{ "document_short_ref": "D57" }"#)).unwrap(),
            json!({ "document_short_ref": "D57" })
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

    #[tokio::test]
    async fn read_only_tool_failure_omits_result_description() {
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

        let value = execute_read_only_tool(
            &pool,
            None,
            &ReadOnlyToolCall {
                name: "unknown_tool".to_string(),
                args: json!({}),
            },
        )
        .await;

        assert_eq!(value["ok"], false);
        assert!(value.get("result_description").is_none());
        assert!(
            value["error"]
                .as_str()
                .unwrap()
                .contains("unknown read-only tool")
        );
    }

    #[test]
    fn read_only_tool_call_accepts_json_string_arguments() {
        let native = ToolCall {
            function: crate::inference::ToolCallFunction {
                name: "get_document".to_string(),
                arguments: json!(r#"{ "document_short_ref": "D57" }"#),
            },
        };

        let tool_call = ReadOnlyToolCall::try_from(&native).expect("tool call");

        assert_eq!(tool_call.args, json!({ "document_short_ref": "D57" }));
    }

    #[tokio::test]
    async fn skill_list_returns_loaded_skill_metadata() {
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "zebra-helper",
            r#"---
name: Zebra Helper
description: Helps with zebra invoices
category: accounting
version: "1.0.0"
---
# Zebra Helper

## Overview

Useful overview.
"#,
            &[],
            &[],
        );
        write_skill_fixture(
            root.path(),
            "alpha-helper",
            r#"---
name: Alpha Helper
description: Helps with alpha invoices
category: accounting
version: "2.0.0"
---
# Alpha Helper

## Overview

Useful overview.
"#,
            &[],
            &[],
        );
        let registry = SkillRegistry::with_dir(root.path());
        registry
            .initialize()
            .await
            .expect("registry should initialize");

        let output = read_skill_list_with_registry(&registry)
            .await
            .expect("skill list should load");
        assert!(
            output
                .result_description
                .as_deref()
                .expect("result description")
                .contains("available skill catalog")
        );
        let value = output.result;

        assert_eq!(value["total_count"], 2);
        assert_eq!(value["items"][0]["id"], "alpha-helper");
        assert_eq!(value["items"][0]["name"], "Alpha Helper");
        assert_eq!(
            value["items"][0]["description"],
            "Helps with alpha invoices"
        );
        assert_eq!(value["items"][0]["category"], "accounting");
        assert_eq!(value["items"][0]["version"], "2.0.0");
        assert_eq!(value["items"][1]["id"], "zebra-helper");
        assert_eq!(value["items"][1]["name"], "Zebra Helper");
    }

    #[tokio::test]
    async fn skill_view_returns_expected_content() {
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "invoice-helper",
            r#"---
name: Invoice Helper
description: Helps with invoices
category: accounting
keywords:
  - invoices
---
# Invoice Helper

## Overview

Useful overview.

## Workflow

1. Ask questions
2. Confirm
"#,
            &[("usage.md", "# Usage\n\nFollow the process.")],
            &[("invoice.html", "<html></html>")],
        );
        let registry = SkillRegistry::with_dir(root.path());
        registry
            .initialize()
            .await
            .expect("registry should initialize");

        let output = read_skill_view_with_registry(&registry, "Invoice Helper", None)
            .await
            .expect("skill view should load");
        assert!(
            output
                .result_description
                .as_deref()
                .expect("result description")
                .contains("main instructions and supporting-file manifest")
        );
        let value = output.result;

        assert_eq!(value["id"], "invoice-helper");
        assert_eq!(value["name"], "Invoice Helper");
        assert_eq!(value["description"], "Helps with invoices");
        assert_eq!(value["category"], "accounting");
        assert_eq!(value["keywords"][0], "invoices");
        assert_eq!(value["references"][0]["name"], "usage.md");
        assert_eq!(value["references"][0]["path"], "references/usage.md");
        assert_eq!(value["templates"][0]["name"], "invoice.html");
        assert_eq!(value["templates"][0]["path"], "templates/invoice.html");
        assert!(
            value["content"]
                .as_str()
                .expect("content")
                .contains("Ask questions")
        );
        assert!(
            value["content"]
                .as_str()
                .expect("content")
                .contains("Useful overview")
        );
    }

    #[test]
    fn skill_view_errors_for_missing_name_argument() {
        let err = required_skill_name(&json!({})).expect_err("missing name should fail");
        assert!(err.to_string().contains("missing required skill name"));
    }

    #[test]
    fn skill_view_accepts_optional_path_argument() {
        assert_eq!(
            optional_skill_path(&json!({ "path": "references/usage.md" })).unwrap(),
            Some("references/usage.md".to_string())
        );
        assert_eq!(optional_skill_path(&json!({})).unwrap(), None);
        assert_eq!(
            optional_skill_path(&json!({ "path": "   " })).unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn skill_view_errors_for_unknown_skill_name() {
        let root = create_temp_skill_root();
        let registry = SkillRegistry::with_dir(root.path());
        registry
            .initialize()
            .await
            .expect("registry should initialize");

        let err = read_skill_view_with_registry(&registry, "missing-skill", None)
            .await
            .expect_err("unknown skill should fail");

        assert!(err.to_string().contains("missing-skill"));
    }

    #[tokio::test]
    async fn skill_view_returns_reference_file_content() {
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "invoice-helper",
            r#"---
name: Invoice Helper
description: Helps with invoices
category: accounting
---
# Invoice Helper

## Overview

Useful overview.
"#,
            &[("usage.md", "# Usage\n\nFollow the process.")],
            &[],
        );
        let registry = SkillRegistry::with_dir(root.path());
        registry
            .initialize()
            .await
            .expect("registry should initialize");

        let output =
            read_skill_view_with_registry(&registry, "Invoice Helper", Some("references/usage.md"))
                .await
                .expect("reference view should load");
        assert!(
            output
                .result_description
                .as_deref()
                .expect("result description")
                .contains("loaded supporting file")
        );
        let value = output.result;

        assert_eq!(value["file_kind"], "reference");
        assert_eq!(value["name"], "usage.md");
        assert!(
            value["content"]
                .as_str()
                .unwrap()
                .contains("Follow the process")
        );
    }

    #[tokio::test]
    async fn skill_view_returns_template_file_content() {
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "invoice-helper",
            r#"---
name: Invoice Helper
description: Helps with invoices
category: accounting
---
# Invoice Helper

## Overview

Useful overview.
"#,
            &[],
            &[("invoice.html", "<html></html>")],
        );
        let registry = SkillRegistry::with_dir(root.path());
        registry
            .initialize()
            .await
            .expect("registry should initialize");

        let output = read_skill_view_with_registry(
            &registry,
            "Invoice Helper",
            Some("templates/invoice.html"),
        )
        .await
        .expect("template view should load");
        assert!(
            output
                .result_description
                .as_deref()
                .expect("result description")
                .contains("loaded supporting file")
        );
        let value = output.result;

        assert_eq!(value["file_kind"], "template");
        assert_eq!(value["name"], "invoice.html");
        assert!(value["content"].as_str().unwrap().contains("<html>"));
    }

    #[tokio::test]
    async fn skill_view_rejects_unknown_supporting_file_path() {
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "invoice-helper",
            r#"---
name: Invoice Helper
description: Helps with invoices
category: accounting
---
# Invoice Helper
"#,
            &[],
            &[],
        );
        let registry = SkillRegistry::with_dir(root.path());
        registry
            .initialize()
            .await
            .expect("registry should initialize");

        let err = read_skill_view_with_registry(
            &registry,
            "Invoice Helper",
            Some("references/missing.md"),
        )
        .await
        .expect_err("missing file should fail");

        assert!(err.to_string().contains("not found or not allowed"));
    }

    #[tokio::test]
    async fn skill_view_rejects_path_traversal() {
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "invoice-helper",
            r#"---
name: Invoice Helper
description: Helps with invoices
category: accounting
---
# Invoice Helper
"#,
            &[],
            &[],
        );
        let registry = SkillRegistry::with_dir(root.path());
        registry
            .initialize()
            .await
            .expect("registry should initialize");

        let err = read_skill_view_with_registry(&registry, "Invoice Helper", Some("../SKILL.md"))
            .await
            .expect_err("path traversal should fail");

        assert!(err.to_string().contains("path traversal"));
    }

    #[tokio::test]
    async fn skill_view_rejects_non_reference_or_template_path() {
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "invoice-helper",
            r#"---
name: Invoice Helper
description: Helps with invoices
category: accounting
---
# Invoice Helper
"#,
            &[],
            &[],
        );
        let registry = SkillRegistry::with_dir(root.path());
        registry
            .initialize()
            .await
            .expect("registry should initialize");

        let err = read_skill_view_with_registry(&registry, "Invoice Helper", Some("SKILL.md"))
            .await
            .expect_err("non-supporting path should fail");

        assert!(err.to_string().contains("references/ or templates/"));
    }

    #[test]
    fn skill_tools_require_registry() {
        let err = required_skills_registry(None).expect_err("missing registry should fail");
        assert!(err.to_string().contains("skills registry unavailable"));
    }
}
