use crate::db::DbPool;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct WorkspaceProfile {
    pub id: i64,
    pub display_name: String,
    pub jurisdiction: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DocumentStatusCounts {
    pub processing_count: i64,
    pub pending_count: i64,
    pub ready_count: i64,
    pub exported_count: i64,
    pub failed_count: i64,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DocumentSummary {
    pub id: i64,
    pub short_ref: String,
    pub status: String,
    pub supplier_name: Option<String>,
    pub invoice_date: Option<String>,
    pub total_amount: Option<String>,
    pub confidence_score: Option<f64>,
    pub review_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DocumentRef {
    pub id: i64,
    pub short_ref: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RetryDocument {
    pub id: i64,
    pub short_ref: String,
    pub status: String,
    pub original_path: Option<String>,
    pub mime_type: Option<String>,
    pub filename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DocumentWhyDetails {
    pub id: i64,
    pub short_ref: String,
    pub status: String,
    pub decision_type: Option<String>,
    pub confidence_score: Option<f64>,
    pub review_reason: Option<String>,
    pub invoice_status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DocumentFailureEvent {
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ReviewSummary {
    pub id: i64,
    pub short_ref: String,
    pub supplier_name: Option<String>,
    pub invoice_number: Option<String>,
    pub invoice_date: Option<String>,
    pub total_amount: Option<String>,
    pub vat_amount: Option<String>,
    pub account_code: Option<String>,
    pub confidence_score: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SourceMediaArtifact {
    pub channel_type: String,
    pub external_file_id: Option<String>,
    pub mime_type: Option<String>,
    pub filename: Option<String>,
}

pub async fn workspace_profile(pool: &DbPool) -> anyhow::Result<Option<WorkspaceProfile>> {
    let row = sqlx::query_as::<_, WorkspaceProfile>(
        r#"
        SELECT
            $1 AS id,
            COALESCE(NULLIF(TRIM(display_name), ''), 'Company setup required') AS display_name,
            jurisdiction
        FROM company_profile
        WHERE singleton = TRUE
        LIMIT 1
        "#,
    )
    .bind(1_i64)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

pub async fn company_profile(pool: &DbPool) -> anyhow::Result<Option<WorkspaceProfile>> {
    workspace_profile(pool).await
}

pub async fn document_status_counts(pool: &DbPool) -> anyhow::Result<DocumentStatusCounts> {
    let counts = sqlx::query_as(
        r#"
        SELECT
            SUM(CASE WHEN status IN ('RECEIVED', 'PROCESSING_VISION', 'VISION_COMPLETE', 'PROCESSING_ACCOUNTANT', 'ACCOUNTANT_REVIEWED', 'PROCESSING_VALIDATOR', 'VALIDATED', 'GENERATING_SIE4', 'REVIEW_COMPLETED') THEN 1 ELSE 0 END) AS processing_count,
            SUM(CASE WHEN status = 'PENDING_HUMAN_REVIEW' THEN 1 ELSE 0 END) AS pending_count,
            SUM(CASE WHEN status = 'EXPORT_READY' THEN 1 ELSE 0 END) AS ready_count,
            SUM(CASE WHEN status = 'EXPORTED' THEN 1 ELSE 0 END) AS exported_count,
            SUM(CASE WHEN status = 'FAILED' THEN 1 ELSE 0 END) AS failed_count
        FROM documents
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(counts)
}

pub async fn list_documents_by_status(
    pool: &DbPool,
    status: &str,
    limit: i64,
) -> anyhow::Result<Vec<DocumentSummary>> {
    let query = document_summary_query("d.status = $1", "$2");
    let rows = sqlx::query_as(&query)
        .bind(status)
        .bind(limit)
        .fetch_all(pool)
        .await?;

    Ok(rows)
}

pub async fn count_documents_by_status(pool: &DbPool, status: &str) -> anyhow::Result<i64> {
    let count = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM documents
        WHERE status = $1
        "#,
    )
    .bind(status)
    .fetch_one(pool)
    .await?;

    Ok(count)
}

pub async fn list_documents_requiring_attention(
    pool: &DbPool,
    limit: i64,
) -> anyhow::Result<Vec<DocumentSummary>> {
    let query = document_summary_query("d.status IN ('PENDING_HUMAN_REVIEW', 'FAILED')", "$1");
    let rows = sqlx::query_as(&query).bind(limit).fetch_all(pool).await?;

    Ok(rows)
}

pub async fn count_documents_requiring_attention(pool: &DbPool) -> anyhow::Result<i64> {
    let count = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM documents
        WHERE status IN ('PENDING_HUMAN_REVIEW', 'FAILED')
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(count)
}

pub async fn list_recent_documents(
    pool: &DbPool,
    limit: i64,
) -> anyhow::Result<Vec<DocumentSummary>> {
    let query = document_summary_query("TRUE", "$1");
    let rows = sqlx::query_as(&query).bind(limit).fetch_all(pool).await?;

    Ok(rows)
}

pub async fn count_documents(pool: &DbPool) -> anyhow::Result<i64> {
    let count = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM documents
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(count)
}

pub async fn latest_document(pool: &DbPool) -> anyhow::Result<Option<DocumentSummary>> {
    Ok(list_recent_documents(pool, 1).await?.into_iter().next())
}

pub async fn document_summary_by_short_ref(
    pool: &DbPool,
    short_ref: &str,
) -> anyhow::Result<Option<DocumentSummary>> {
    let query = document_summary_query("d.short_ref = $1", "$2");
    let row = sqlx::query_as(&query)
        .bind(short_ref)
        .bind(1_i64)
        .fetch_optional(pool)
        .await?;

    Ok(row)
}

pub async fn document_ref_by_short_ref(
    pool: &DbPool,
    short_ref: &str,
) -> anyhow::Result<Option<DocumentRef>> {
    let row = sqlx::query_as(
        r#"
        SELECT id, short_ref, status
        FROM documents d
        WHERE d.short_ref = $1
        LIMIT 1
        "#,
    )
    .bind(short_ref)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

pub async fn retry_document_by_short_ref(
    pool: &DbPool,
    short_ref: &str,
) -> anyhow::Result<Option<RetryDocument>> {
    let row = sqlx::query_as(
        r#"
        SELECT id, short_ref, status, original_path, mime_type, filename
        FROM documents d
        WHERE d.short_ref = $1
        LIMIT 1
        "#,
    )
    .bind(short_ref)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

pub async fn document_why_details(
    pool: &DbPool,
    short_ref: &str,
) -> anyhow::Result<Option<DocumentWhyDetails>> {
    let row = sqlx::query_as(
        r#"
        SELECT
            d.id,
            d.short_ref,
            d.status,
            (
                SELECT decision_type
                FROM review_decisions rd
                WHERE rd.document_id = d.id
                ORDER BY reviewed_at DESC, created_at DESC
                LIMIT 1
            ) AS decision_type,
            (
                SELECT confidence_score
                FROM review_decisions rd
                WHERE rd.document_id = d.id
                ORDER BY reviewed_at DESC, created_at DESC
                LIMIT 1
            ) AS confidence_score,
            (
                SELECT review_reason
                FROM review_decisions rd
                WHERE rd.document_id = d.id
                ORDER BY reviewed_at DESC, created_at DESC
                LIMIT 1
            ) AS review_reason,
            (
                SELECT status
                FROM invoices i
                WHERE i.document_id = d.id
                ORDER BY updated_at DESC
                LIMIT 1
            ) AS invoice_status
        FROM documents d
        WHERE d.short_ref = $1
        LIMIT 1
        "#,
    )
    .bind(short_ref)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

pub async fn latest_failure_event_for_document(
    pool: &DbPool,
    document_id: i64,
) -> anyhow::Result<Option<DocumentFailureEvent>> {
    let row = sqlx::query_as(
        r#"
        SELECT payload
        FROM document_events
        WHERE document_id = $1
          AND event_type = 'DOCUMENT_FAILED'
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(document_id)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

pub async fn review_summary(pool: &DbPool, document_id: i64) -> anyhow::Result<ReviewSummary> {
    let summary = sqlx::query_as(
        r#"
        SELECT
            d.id,
            d.short_ref,
            (
                SELECT parsed_value FROM extracted_fields
                WHERE document_id = d.id AND field_type = 'supplier_name'
                ORDER BY created_at DESC LIMIT 1
            ) AS supplier_name,
            (
                SELECT parsed_value FROM extracted_fields
                WHERE document_id = d.id AND field_type = 'invoice_number'
                ORDER BY created_at DESC LIMIT 1
            ) AS invoice_number,
            (
                SELECT parsed_value FROM extracted_fields
                WHERE document_id = d.id AND field_type = 'transaction_date'
                ORDER BY created_at DESC LIMIT 1
            ) AS invoice_date,
            (
                SELECT parsed_value FROM extracted_fields
                WHERE document_id = d.id AND field_type = 'total_amount'
                ORDER BY created_at DESC LIMIT 1
            ) AS total_amount,
            (
                SELECT parsed_value FROM extracted_fields
                WHERE document_id = d.id AND field_type = 'vat_amount'
                ORDER BY created_at DESC LIMIT 1
            ) AS vat_amount,
            (
                SELECT assigned_account_code FROM accounting_decisions
                WHERE document_id = d.id ORDER BY created_at DESC LIMIT 1
            ) AS account_code,
            (
                SELECT confidence_score FROM review_decisions
                WHERE document_id = d.id ORDER BY reviewed_at DESC, created_at DESC LIMIT 1
            ) AS confidence_score
        FROM documents d
        WHERE d.id = $1
        "#,
    )
    .bind(document_id)
    .fetch_one(pool)
    .await?;

    Ok(summary)
}

pub async fn latest_source_media_artifact(
    pool: &DbPool,
    document_id: i64,
) -> anyhow::Result<Option<SourceMediaArtifact>> {
    let artifact = sqlx::query_as(
        r#"
        SELECT
            da.channel_type,
            NULLIF(json_extract(da.metadata, '$.file_id'), '') AS external_file_id,
            da.mime_type,
            da.original_filename AS filename
        FROM document_artifacts da
        WHERE da.document_id = $1
        ORDER BY da.created_at DESC
        LIMIT 1
        "#,
    )
    .bind(document_id)
    .fetch_optional(pool)
    .await?;

    Ok(artifact)
}

fn document_summary_query(status_clause: &str, limit_placeholder: &str) -> String {
    format!(
        r#"
        SELECT
            d.id,
            d.short_ref,
            d.status,
            (
                SELECT parsed_value
                FROM extracted_fields ef
                WHERE ef.document_id = d.id AND ef.field_type = 'supplier_name'
                ORDER BY created_at DESC
                LIMIT 1
            ) AS supplier_name,
            (
                SELECT parsed_value
                FROM extracted_fields ef
                WHERE ef.document_id = d.id AND ef.field_type = 'transaction_date'
                ORDER BY created_at DESC
                LIMIT 1
            ) AS invoice_date,
            (
                SELECT parsed_value
                FROM extracted_fields ef
                WHERE ef.document_id = d.id AND ef.field_type = 'total_amount'
                ORDER BY created_at DESC
                LIMIT 1
            ) AS total_amount,
            (
                SELECT confidence_score
                FROM review_decisions rd
                WHERE rd.document_id = d.id
                ORDER BY reviewed_at DESC, created_at DESC
                LIMIT 1
            ) AS confidence_score,
            (
                SELECT review_reason
                FROM review_decisions rd
                WHERE rd.document_id = d.id
                ORDER BY reviewed_at DESC, created_at DESC
                LIMIT 1
            ) AS review_reason
        FROM documents d
        WHERE {status_clause}
        ORDER BY d.created_at DESC
        LIMIT {limit_placeholder}
        "#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_summary_query_has_status_clause() {
        let query = document_summary_query("TRUE", "$1");
        assert!(query.contains("WHERE TRUE"));
    }
}
