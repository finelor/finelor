use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, FromRef, FromRequestParts, Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode, request::Parts},
    response::Response,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    config::AppConfig,
    db::{self, ApiKey, DbPool},
    document_state::fetch_document_state_by_short_ref,
    ingestion::{self, DocumentArtifactInput, IngestionInput},
    messaging::{commands::describe_document_why, intents::normalize_short_ref},
    query::{document_ref_by_short_ref, grouped_document_status_counts},
    queue::QueueProducer,
    web::events::AppEventBus,
};

const DOCUMENT_UPLOAD_BODY_LIMIT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone)]
pub struct PublicApiState {
    pub config: Arc<AppConfig>,
    pub pool: DbPool,
    pub queue_producer: QueueProducer,
    pub events: AppEventBus,
}

pub fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    PublicApiState: FromRef<S>,
{
    Router::new()
        .route(
            "/documents",
            post(create_document)
                .layer(DefaultBodyLimit::max(DOCUMENT_UPLOAD_BODY_LIMIT_BYTES))
                .get(list_documents),
        )
        .route("/documents/status", get(document_status))
        .route("/documents/{short_ref}", get(get_document))
        .route("/documents/{short_ref}/explain", get(explain_document))
        .route("/documents/{short_ref}/file", get(get_document_file))
}

#[derive(Clone)]
struct ApiKeyAuth {
    _key: ApiKey,
}

impl<S> FromRequestParts<S> for ApiKeyAuth
where
    S: Send + Sync,
    PublicApiState: FromRef<S>,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = PublicApiState::from_ref(state);
        let token = bearer_token(&parts.headers).ok_or_else(unauthorized)?;
        let key = db::authenticate_api_key(&state.pool, token)
            .await
            .map_err(|_| server_error("API key authentication failed"))?
            .ok_or_else(unauthorized)?;
        Ok(Self { _key: key })
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn unauthorized() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized"})),
    )
}

fn server_error(message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": message})),
    )
}

fn bad_request(message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({"error": message})))
}

#[derive(Debug, Serialize)]
pub struct CreateDocumentResponse {
    pub document_id: i64,
    pub short_ref: String,
    pub status: DocumentStatusSummary,
}

#[derive(Debug, Serialize)]
pub struct DocumentListResponse {
    pub items: Vec<DocumentListItem>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct DocumentListItem {
    pub short_ref: String,
    pub status: DocumentStatusSummary,
    pub supplier_name: Option<String>,
    pub transaction_date: Option<String>,
    pub total_amount: Option<String>,
    pub received_date: Option<String>,
    pub ai_confidence: Option<f64>,
    pub model_used: Option<String>,
    pub has_download: bool,
    pub download_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DocumentDetailsResponse {
    pub short_ref: String,
    pub status: Option<DocumentStatusResponse>,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub supplier_name: Option<String>,
    pub transaction_date: Option<String>,
    pub total_amount: Option<String>,
    pub vat_amount: Option<String>,
    pub subtotal_amount: Option<String>,
    pub net_amount: Option<String>,
    pub assigned_account_code: Option<String>,
    pub account_name: Option<String>,
    pub ai_confidence: Option<f64>,
    pub model_used: Option<String>,
    pub document_type: Option<String>,
    pub review_confidence: Option<f64>,
    pub review_decision_type: Option<String>,
    pub review_reason: Option<String>,
    pub validation_errors: Option<serde_json::Value>,
    pub accounting_rows: Vec<AccountingRowResponse>,
    pub has_download: bool,
    pub download_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AccountingRowResponse {
    pub account_code: String,
    pub description: Option<String>,
    pub amount: Option<String>,
    pub vat_code: Option<String>,
    pub is_debit: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct DomainCountSummary {
    pub processing: i64,
    pub ingested: i64,
    pub failed: i64,
}

#[derive(Debug, Serialize)]
pub struct AccountingCountSummary {
    pub processing: i64,
    pub pending_review: i64,
    pub ready_for_export: i64,
    pub exported: i64,
    pub failed: i64,
}

#[derive(Debug, Serialize)]
pub struct DocumentStatusCountsResponse {
    pub documents_total: i64,
    pub intake: DomainCountSummary,
    pub accounting: AccountingCountSummary,
}

#[derive(Debug, Serialize)]
pub struct DocumentStatusSummary {
    pub intake: DomainStatusSummary,
    pub accounting: AccountingStatusSummary,
}

#[derive(Debug, Serialize)]
pub struct DocumentExplainResponse {
    pub short_ref: String,
    pub explanation: String,
}

#[derive(Debug, Serialize)]
pub struct DomainStatusDetail {
    pub status: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AccountingStatusDetail {
    pub status: Option<String>,
    pub requested_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub failure_reason: Option<String>,
    pub review_reason: Option<String>,
    pub export_batch_id: Option<i64>,
    pub latest_run_kind: Option<String>,
    pub latest_run_status: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DomainStatusSummary {
    pub status: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AccountingStatusSummary {
    pub status: Option<String>,
    pub review_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DocumentStatusResponse {
    pub intake: DomainStatusDetail,
    pub accounting: AccountingStatusDetail,
}

#[derive(Debug, Deserialize)]
pub struct DocumentListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub intake_status: Option<String>,
    pub accounting_status: Option<String>,
    pub month: Option<String>,
    pub search: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct ApiDocumentListRow {
    short_ref: String,
    received_date: Option<String>,
    mime_type: Option<String>,
    original_path: Option<String>,
    supplier_name: Option<String>,
    transaction_date: Option<String>,
    total_amount: Option<String>,
    ai_confidence: Option<f64>,
    model_used: Option<String>,
    intake_status: Option<String>,
    accounting_status: Option<String>,
    accounting_review_reason: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct ApiDocumentDetailsRow {
    id: i64,
    short_ref: String,
    filename: Option<String>,
    mime_type: Option<String>,
    original_path: Option<String>,
    supplier_name: Option<String>,
    transaction_date: Option<String>,
    total_amount: Option<String>,
    vat_amount: Option<String>,
    subtotal_amount: Option<String>,
    document_type: Option<String>,
    assigned_account_code: Option<String>,
    ai_confidence: Option<f64>,
    model_used: Option<String>,
    account_name: Option<String>,
    net_amount: Option<String>,
    review_confidence: Option<f64>,
    review_decision_type: Option<String>,
    review_reason: Option<String>,
    validation_errors: Option<serde_json::Value>,
}

async fn document_status(
    _auth: ApiKeyAuth,
    State(state): State<PublicApiState>,
) -> Result<Json<DocumentStatusCountsResponse>, (StatusCode, Json<serde_json::Value>)> {
    let counts = grouped_document_status_counts(&state.pool)
        .await
        .map_err(|_| server_error("Document status query failed"))?;
    Ok(Json(DocumentStatusCountsResponse {
        documents_total: counts.documents_total,
        intake: DomainCountSummary {
            processing: counts.intake_processing_count,
            ingested: counts.intake_ingested_count,
            failed: counts.intake_failed_count,
        },
        accounting: AccountingCountSummary {
            processing: counts.accounting_processing_count,
            pending_review: counts.accounting_pending_review_count,
            ready_for_export: counts.accounting_ready_for_export_count,
            exported: counts.accounting_exported_count,
            failed: counts.accounting_failed_count,
        },
    }))
}

async fn create_document(
    _auth: ApiKeyAuth,
    State(state): State<PublicApiState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<CreateDocumentResponse>, (StatusCode, Json<serde_json::Value>)> {
    let mut file_bytes = Vec::new();
    let mut filename_override = None;
    let mut original_filename = None;
    let mut mime_type = None;
    let mut source_id = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| bad_request(&format!("Invalid multipart body: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                if original_filename.is_none() {
                    original_filename = field.file_name().map(ToOwned::to_owned);
                }
                if mime_type.is_none() {
                    mime_type = field.content_type().map(ToOwned::to_owned);
                }
                file_bytes = field
                    .bytes()
                    .await
                    .map_err(|e| bad_request(&format!("Invalid file field: {e}")))?
                    .to_vec();
            }
            "filename" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| bad_request(&format!("Invalid filename field: {e}")))?;
                filename_override = Some(String::from_utf8_lossy(&bytes).trim().to_string());
            }
            "mime_type" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| bad_request(&format!("Invalid mime_type field: {e}")))?;
                mime_type = Some(String::from_utf8_lossy(&bytes).trim().to_string());
            }
            "source_id" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| bad_request(&format!("Invalid source_id field: {e}")))?;
                source_id = Some(String::from_utf8_lossy(&bytes).trim().to_string());
            }
            _ => {}
        }
    }

    if file_bytes.is_empty() {
        return Err(bad_request("Missing 'file' field"));
    }

    let mime_type = mime_type
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let ext = extension_for_mime(&mime_type);
    let doc_id = Uuid::new_v4();
    let stored_filename = format!(
        "{}_{}.{}",
        chrono::Utc::now().timestamp_millis(),
        doc_id,
        ext
    );
    let source_id = source_id.filter(|value| !value.is_empty());
    let source_timestamp = headers
        .get("X-Source-Timestamp")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let profile_identifier = headers
        .get("X-Submitter-ID")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let channel_identifier = source_id.clone().unwrap_or_else(|| "api".to_string());
    let external_artifact_id = source_id
        .as_ref()
        .map(|value| format!("API_{value}"))
        .or_else(|| Some(format!("api_{doc_id}")));
    let display_filename = filename_override
        .filter(|value| !value.is_empty())
        .or(original_filename);

    let result = ingestion::ingest_document(
        &state.pool,
        state.config.as_ref(),
        &state.queue_producer,
        Some(&state.events),
        IngestionInput {
            file_bytes,
            filename: stored_filename,
            mime_type: mime_type.clone(),
            document_type: "INVOICE".to_string(),
            artifact: DocumentArtifactInput {
                channel_type: "API".to_string(),
                channel_identifier,
                profile_identifier: profile_identifier.clone(),
                external_artifact_id,
                source_timestamp,
                original_filename: display_filename,
                metadata: json!({
                    "source": "API",
                    "source_id": source_id,
                    "profile_identifier": profile_identifier,
                }),
            },
        },
    )
    .await
    .map_err(|_| server_error("Document ingestion failed"))?;
    let saved = result.document_ref();

    Ok(Json(CreateDocumentResponse {
        document_id: saved.id,
        short_ref: saved.short_ref.clone(),
        status: DocumentStatusSummary {
            intake: DomainStatusSummary {
                status: Some("RECEIVED".to_string()),
            },
            accounting: AccountingStatusSummary {
                status: Some("NOT_REQUESTED".to_string()),
                review_reason: None,
            },
        },
    }))
}

async fn list_documents(
    _auth: ApiKeyAuth,
    State(state): State<PublicApiState>,
    Query(query): Query<DocumentListQuery>,
) -> Result<Json<DocumentListResponse>, (StatusCode, Json<serde_json::Value>)> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0).max(0);
    let intake_status = clean_filter(query.intake_status);
    let accounting_status = clean_filter(query.accounting_status);
    let month = clean_filter(query.month);
    let search = clean_filter(query.search);

    let rows = sqlx::query_as::<_, ApiDocumentListRow>(
        r#"
        SELECT
          d.short_ref,
          date(d.received_at) AS received_date,
          d.mime_type,
          d.original_path,
          (
            SELECT parsed_value FROM extracted_fields ef
            WHERE ef.document_id = d.id AND ef.field_type = 'supplier_name'
            ORDER BY ef.updated_at DESC, ef.created_at DESC
            LIMIT 1
          ) AS supplier_name,
          (
            SELECT parsed_value FROM extracted_fields ef
            WHERE ef.document_id = d.id AND ef.field_type = 'transaction_date'
            ORDER BY ef.updated_at DESC, ef.created_at DESC
            LIMIT 1
          ) AS transaction_date,
          (
            SELECT parsed_value FROM extracted_fields ef
            WHERE ef.document_id = d.id AND ef.field_type = 'total_amount'
            ORDER BY ef.updated_at DESC, ef.created_at DESC
            LIMIT 1
          ) AS total_amount,
          (
            SELECT ai_confidence FROM accounting_decisions ad
            WHERE ad.document_id = d.id
            ORDER BY ad.created_at DESC
            LIMIT 1
          ) AS ai_confidence,
          (
            SELECT model_used FROM accounting_decisions ad
            WHERE ad.document_id = d.id
            ORDER BY ad.created_at DESC
            LIMIT 1
          ) AS model_used,
          dis.status AS intake_status,
          das.status AS accounting_status,
          das.review_reason AS accounting_review_reason
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        WHERE ($1 IS NULL OR strftime('%Y-%m', d.received_at) = $1)
          AND (
            $2 IS NULL
            OR LOWER(d.short_ref) LIKE ('%' || LOWER($2) || '%')
            OR EXISTS (
              SELECT 1 FROM extracted_fields ef
              WHERE ef.document_id = d.id
                AND ef.field_type = 'supplier_name'
                AND LOWER(ef.parsed_value) LIKE ('%' || LOWER($2) || '%')
            )
          )
          AND ($3 IS NULL OR dis.status = $3)
          AND ($4 IS NULL OR das.status = $4)
        ORDER BY d.received_at DESC
        "#,
    )
    .bind(&month)
    .bind(&search)
    .bind(&intake_status)
    .bind(&accounting_status)
    .fetch_all(&state.pool)
    .await
    .map_err(|_| server_error("Document query failed"))?;

    let mut items: Vec<DocumentListItem> =
        rows.into_iter().map(document_list_item_from_row).collect();
    let total = items.len() as i64;
    let start = offset as usize;
    let end = start.saturating_add(limit as usize).min(items.len());
    let items = if start >= items.len() {
        Vec::new()
    } else {
        items.drain(start..end).collect()
    };

    Ok(Json(DocumentListResponse {
        items,
        limit,
        offset,
        total,
    }))
}

async fn get_document(
    _auth: ApiKeyAuth,
    State(state): State<PublicApiState>,
    Path(short_ref): Path<String>,
) -> Result<Json<DocumentDetailsResponse>, (StatusCode, Json<serde_json::Value>)> {
    let row = document_details_row(&state.pool, &short_ref)
        .await?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))))?;
    let accounting_rows = accounting_rows(&state.pool, row.id).await?;
    let document_state = fetch_document_state_by_short_ref(&state.pool, short_ref.trim())
        .await
        .map_err(|_| server_error("Document status query failed"))?;
    Ok(Json(document_details_from_row(
        row,
        accounting_rows,
        document_state,
    )))
}

async fn explain_document(
    _auth: ApiKeyAuth,
    State(state): State<PublicApiState>,
    Path(short_ref): Path<String>,
) -> Result<Json<DocumentExplainResponse>, (StatusCode, Json<serde_json::Value>)> {
    let Some(short_ref) = normalize_short_ref(&short_ref) else {
        return Err(bad_request("Invalid document reference"));
    };
    if document_ref_by_short_ref(&state.pool, &short_ref)
        .await
        .map_err(|_| server_error("Document query failed"))?
        .is_none()
    {
        return Err((StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))));
    }

    let explanation = describe_document_why(&state.pool, &short_ref)
        .await
        .map_err(|_| server_error("Document explanation failed"))?;
    Ok(Json(DocumentExplainResponse {
        short_ref,
        explanation,
    }))
}

async fn get_document_file(
    _auth: ApiKeyAuth,
    State(state): State<PublicApiState>,
    Path(short_ref): Path<String>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let row = sqlx::query(
        r#"
        SELECT
          d.original_path,
          d.mime_type,
          COALESCE(
            (
              SELECT NULLIF(TRIM(da.original_filename), '')
              FROM document_artifacts da
              WHERE da.document_id = d.id
              ORDER BY da.created_at DESC, da.id DESC
              LIMIT 1
            ),
            d.filename
          ) AS filename
        FROM documents d
        WHERE d.short_ref = $1
        LIMIT 1
        "#,
    )
    .bind(short_ref.trim())
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| server_error("File lookup failed"))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))))?;

    let original_path: String = row
        .try_get("original_path")
        .map_err(|_| server_error("File lookup failed"))?;
    let content_type: String = row
        .try_get::<Option<String>, _>("mime_type")
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());

    let upload_root = std::fs::canonicalize(&state.config.upload.storage_path)
        .map_err(|_| server_error("Invalid upload root"))?;
    let file_path = std::fs::canonicalize(&original_path).map_err(|_| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "file_not_found"})),
        )
    })?;
    if !file_path.starts_with(&upload_root) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "access_denied"})),
        ));
    }

    let content = tokio::fs::read(&file_path).await.map_err(|_| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "file_not_found"})),
        )
    })?;
    let filename: Option<String> = row.try_get("filename").ok();
    let disposition = filename
        .as_deref()
        .map(|value| value.replace('"', ""))
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("attachment; filename=\"{value}\""))
        .unwrap_or_else(|| "attachment".to_string());
    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .header(axum::http::header::CONTENT_DISPOSITION, disposition)
        .body(Body::from(content))
        .map_err(|_| server_error("File response failed"))
}

fn extension_for_mime(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "application/pdf" => "pdf",
        _ => "bin",
    }
}

fn clean_filter(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn has_download(mime_type: Option<&str>, original_path: Option<&str>) -> bool {
    original_path.is_some() && mime_type.is_some_and(|value| !value.trim().is_empty())
}

fn download_url(short_ref: &str, has_download: bool) -> Option<String> {
    has_download.then(|| format!("/api/v1/documents/{short_ref}/file"))
}

fn document_list_item_from_row(row: ApiDocumentListRow) -> DocumentListItem {
    let short_ref = row.short_ref;
    let mime_type = row.mime_type;
    let original_path = row.original_path;
    let has_download = has_download(mime_type.as_deref(), original_path.as_deref());
    DocumentListItem {
        download_url: download_url(&short_ref, has_download),
        short_ref,
        status: DocumentStatusSummary {
            intake: DomainStatusSummary {
                status: row.intake_status,
            },
            accounting: AccountingStatusSummary {
                status: row.accounting_status,
                review_reason: row.accounting_review_reason,
            },
        },
        supplier_name: row.supplier_name,
        transaction_date: row.transaction_date,
        total_amount: row.total_amount,
        received_date: row.received_date,
        ai_confidence: row.ai_confidence,
        model_used: row.model_used,
        has_download,
    }
}

async fn document_details_row(
    pool: &DbPool,
    short_ref: &str,
) -> Result<Option<ApiDocumentDetailsRow>, (StatusCode, Json<serde_json::Value>)> {
    sqlx::query_as(
        r#"
        SELECT
          d.id, d.short_ref, d.mime_type, d.original_path,
          COALESCE(
            (
              SELECT NULLIF(TRIM(da.original_filename), '')
              FROM document_artifacts da
              WHERE da.document_id = d.id
              ORDER BY da.created_at DESC, da.id DESC
              LIMIT 1
            ),
            d.filename
          ) AS filename,
          (
            SELECT parsed_value FROM extracted_fields ef
            WHERE ef.document_id = d.id AND ef.field_type = 'supplier_name'
            ORDER BY ef.updated_at DESC, ef.created_at DESC
            LIMIT 1
          ) AS supplier_name,
          (
            SELECT parsed_value FROM extracted_fields ef
            WHERE ef.document_id = d.id AND ef.field_type = 'transaction_date'
            ORDER BY ef.updated_at DESC, ef.created_at DESC
            LIMIT 1
          ) AS transaction_date,
          (
            SELECT parsed_value FROM extracted_fields ef
            WHERE ef.document_id = d.id AND ef.field_type = 'total_amount'
            ORDER BY ef.updated_at DESC, ef.created_at DESC
            LIMIT 1
          ) AS total_amount,
          (
            SELECT parsed_value FROM extracted_fields ef
            WHERE ef.document_id = d.id AND ef.field_type = 'vat_amount'
            ORDER BY ef.updated_at DESC, ef.created_at DESC
            LIMIT 1
          ) AS vat_amount,
          (
            SELECT parsed_value FROM extracted_fields ef
            WHERE ef.document_id = d.id AND ef.field_type = 'subtotal_amount'
            ORDER BY ef.updated_at DESC, ef.created_at DESC
            LIMIT 1
          ) AS subtotal_amount,
          (
            SELECT parsed_value FROM extracted_fields ef
            WHERE ef.document_id = d.id AND ef.field_type = 'document_type'
            ORDER BY ef.updated_at DESC, ef.created_at DESC
            LIMIT 1
          ) AS document_type,
          (
            SELECT assigned_account_code FROM accounting_decisions ad
            WHERE ad.document_id = d.id
            ORDER BY ad.created_at DESC
            LIMIT 1
          ) AS assigned_account_code,
          (
            SELECT ai_confidence FROM accounting_decisions ad
            WHERE ad.document_id = d.id
            ORDER BY ad.created_at DESC
            LIMIT 1
          ) AS ai_confidence,
          (
            SELECT model_used FROM accounting_decisions ad
            WHERE ad.document_id = d.id
            ORDER BY ad.created_at DESC
            LIMIT 1
          ) AS model_used,
          (
            SELECT account_name FROM accounting_decisions ad
            WHERE ad.document_id = d.id
            ORDER BY ad.created_at DESC
            LIMIT 1
          ) AS account_name,
          (
            SELECT CAST(net_amount AS TEXT) FROM accounting_decisions ad
            WHERE ad.document_id = d.id
            ORDER BY ad.created_at DESC
            LIMIT 1
          ) AS net_amount,
          (
            SELECT confidence_score FROM review_decisions rd
            WHERE rd.document_id = d.id
            ORDER BY rd.reviewed_at DESC
            LIMIT 1
          ) AS review_confidence,
          (
            SELECT decision_type FROM review_decisions rd
            WHERE rd.document_id = d.id
            ORDER BY rd.reviewed_at DESC
            LIMIT 1
          ) AS review_decision_type,
          (
            SELECT review_reason FROM review_decisions rd
            WHERE rd.document_id = d.id
            ORDER BY rd.reviewed_at DESC
            LIMIT 1
          ) AS review_reason,
          (
            SELECT validation_errors FROM validation_results vr
            WHERE vr.document_id = d.id
            ORDER BY vr.checked_at DESC
            LIMIT 1
          ) AS validation_errors
        FROM documents d
        WHERE d.short_ref = $1
        LIMIT 1
        "#,
    )
    .bind(short_ref.trim())
    .fetch_optional(pool)
    .await
    .map_err(|_| server_error("Document query failed"))
}

async fn accounting_rows(
    pool: &DbPool,
    document_id: i64,
) -> Result<Vec<AccountingRowResponse>, (StatusCode, Json<serde_json::Value>)> {
    let rows = sqlx::query(
        r#"
        SELECT aa.account_code, aa.description, CAST(aa.amount AS TEXT) AS amount, aa.vat_code, aa.is_debit
        FROM invoices i
        JOIN account_assignments aa ON aa.invoice_id = i.id
        WHERE i.document_id = $1
        ORDER BY aa.sort_order ASC, aa.created_at ASC
        "#,
    )
    .bind(document_id)
    .fetch_all(pool)
    .await
    .map_err(|_| server_error("Document query failed"))?;

    Ok(rows
        .into_iter()
        .map(|row| AccountingRowResponse {
            account_code: row.try_get("account_code").unwrap_or_default(),
            description: row.try_get("description").ok(),
            amount: row.try_get("amount").ok(),
            vat_code: row.try_get("vat_code").ok(),
            is_debit: row.try_get("is_debit").ok(),
        })
        .collect())
}

fn document_details_from_row(
    row: ApiDocumentDetailsRow,
    accounting_rows: Vec<AccountingRowResponse>,
    document_state: Option<crate::document_state::DocumentStateSnapshot>,
) -> DocumentDetailsResponse {
    let short_ref = row.short_ref;
    let mime_type = row.mime_type;
    let original_path = row.original_path;
    let has_download = has_download(mime_type.as_deref(), original_path.as_deref());
    DocumentDetailsResponse {
        download_url: download_url(&short_ref, has_download),
        short_ref,
        status: document_state.map(|snapshot| DocumentStatusResponse {
            intake: DomainStatusDetail {
                status: snapshot.intake_status,
                started_at: snapshot.intake_started_at,
                completed_at: snapshot.intake_completed_at,
                failure_reason: snapshot.intake_failure_reason,
            },
            accounting: AccountingStatusDetail {
                status: snapshot.accounting_status,
                requested_at: snapshot.accounting_requested_at,
                started_at: snapshot.accounting_started_at,
                completed_at: snapshot.accounting_completed_at,
                failure_reason: snapshot.accounting_failure_reason,
                review_reason: snapshot.accounting_review_reason,
                export_batch_id: snapshot.accounting_export_batch_id,
                latest_run_kind: snapshot.latest_accounting_run_kind,
                latest_run_status: snapshot.latest_accounting_run_status,
            },
        }),
        filename: row.filename,
        mime_type,
        supplier_name: row.supplier_name,
        transaction_date: row.transaction_date,
        total_amount: row.total_amount,
        vat_amount: row.vat_amount,
        subtotal_amount: row.subtotal_amount,
        net_amount: row.net_amount,
        assigned_account_code: row.assigned_account_code,
        account_name: row.account_name,
        ai_confidence: row.ai_confidence,
        model_used: row.model_used,
        document_type: row.document_type,
        review_confidence: row.review_confidence,
        review_decision_type: row.review_decision_type,
        review_reason: row.review_reason,
        validation_errors: row.validation_errors,
        accounting_rows,
        has_download,
    }
}
