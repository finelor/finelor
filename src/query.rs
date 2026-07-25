use crate::db::DbPool;
use crate::document_state::DocumentStateSnapshot;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct WorkspaceProfile {
    pub id: i64,
    pub display_name: String,
    pub jurisdiction: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct WorkspaceIdentity {
    pub workspace_name: Option<String>,
    pub jurisdiction: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DocumentStatusCounts {
    pub total_count: i64,
    pub processing_count: i64,
    pub pending_count: i64,
    pub ready_count: i64,
    pub exported_count: i64,
    pub failed_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct GroupedDocumentStatusCounts {
    pub documents_total: i64,
    pub intake_processing_count: i64,
    pub intake_ingested_count: i64,
    pub intake_failed_count: i64,
    pub accounting_processing_count: i64,
    pub accounting_pending_review_count: i64,
    pub accounting_ready_for_export_count: i64,
    pub accounting_exported_count: i64,
    pub accounting_failed_count: i64,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DocumentSummary {
    pub id: i64,
    pub short_ref: String,
    pub intake_status: Option<String>,
    pub accounting_status: Option<String>,
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
    pub intake_status: Option<String>,
    pub accounting_status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AccountingProcessingCandidate {
    pub id: i64,
    pub short_ref: String,
    pub intake_status: String,
    pub accounting_status: String,
    pub accounting_requested_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RetryDocument {
    pub id: i64,
    pub short_ref: String,
    pub intake_status: Option<String>,
    pub accounting_status: Option<String>,
    pub review_reason: Option<String>,
    pub original_path: Option<String>,
    pub mime_type: Option<String>,
    pub filename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DocumentWhyDetails {
    pub id: i64,
    pub short_ref: String,
    pub intake_status: Option<String>,
    pub accounting_status: Option<String>,
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

#[derive(Debug, Clone, sqlx::FromRow)]
struct DocumentSummaryRow {
    id: i64,
    short_ref: String,
    supplier_name: Option<String>,
    invoice_date: Option<String>,
    total_amount: Option<String>,
    confidence_score: Option<f64>,
    review_reason: Option<String>,
    intake_status: Option<String>,
    accounting_status: Option<String>,
    accounting_requested_at: Option<String>,
    accounting_review_reason: Option<String>,
    accounting_export_batch_id: Option<i64>,
    latest_accounting_run_kind: Option<String>,
    latest_accounting_run_status: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct DocumentStatusRow {
    id: i64,
    short_ref: String,
    intake_status: Option<String>,
    accounting_status: Option<String>,
    accounting_requested_at: Option<String>,
    accounting_review_reason: Option<String>,
    accounting_export_batch_id: Option<i64>,
    latest_accounting_run_kind: Option<String>,
    latest_accounting_run_status: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct RetryDocumentRow {
    id: i64,
    short_ref: String,
    original_path: Option<String>,
    mime_type: Option<String>,
    filename: Option<String>,
    review_reason: Option<String>,
    intake_status: Option<String>,
    accounting_status: Option<String>,
    accounting_requested_at: Option<String>,
    accounting_review_reason: Option<String>,
    accounting_export_batch_id: Option<i64>,
    latest_accounting_run_kind: Option<String>,
    latest_accounting_run_status: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct DocumentWhyDetailsRow {
    id: i64,
    short_ref: String,
    decision_type: Option<String>,
    confidence_score: Option<f64>,
    review_reason: Option<String>,
    invoice_status: Option<String>,
    intake_status: Option<String>,
    accounting_status: Option<String>,
    accounting_requested_at: Option<String>,
    accounting_review_reason: Option<String>,
    accounting_export_batch_id: Option<i64>,
    latest_accounting_run_kind: Option<String>,
    latest_accounting_run_status: Option<String>,
}

struct DocumentStateFields {
    document_id: i64,
    intake_status: Option<String>,
    accounting_status: Option<String>,
    accounting_requested_at: Option<String>,
    accounting_review_reason: Option<String>,
    accounting_export_batch_id: Option<i64>,
    latest_accounting_run_kind: Option<String>,
    latest_accounting_run_status: Option<String>,
}

fn document_state_from_fields(fields: DocumentStateFields) -> DocumentStateSnapshot {
    DocumentStateSnapshot {
        document_id: fields.document_id,
        intake_status: fields.intake_status,
        intake_started_at: None,
        intake_completed_at: None,
        intake_failure_reason: None,
        accounting_status: fields.accounting_status,
        accounting_requested_at: fields.accounting_requested_at,
        accounting_started_at: None,
        accounting_completed_at: None,
        accounting_failure_reason: None,
        accounting_review_reason: fields.accounting_review_reason,
        accounting_export_batch_id: fields.accounting_export_batch_id,
        latest_accounting_run_kind: fields.latest_accounting_run_kind,
        latest_accounting_run_status: fields.latest_accounting_run_status,
    }
}

fn document_summary_from_row(row: DocumentSummaryRow) -> DocumentSummary {
    let snapshot = document_state_from_fields(DocumentStateFields {
        document_id: row.id,
        intake_status: row.intake_status,
        accounting_status: row.accounting_status,
        accounting_requested_at: row.accounting_requested_at,
        accounting_review_reason: row.accounting_review_reason,
        accounting_export_batch_id: row.accounting_export_batch_id,
        latest_accounting_run_kind: row.latest_accounting_run_kind,
        latest_accounting_run_status: row.latest_accounting_run_status,
    });
    DocumentSummary {
        id: row.id,
        short_ref: row.short_ref,
        intake_status: snapshot.intake_status.clone(),
        accounting_status: snapshot.accounting_status.clone(),
        supplier_name: row.supplier_name,
        invoice_date: row.invoice_date,
        total_amount: row.total_amount,
        confidence_score: row.confidence_score,
        review_reason: row
            .review_reason
            .or(snapshot.accounting_review_reason.clone()),
    }
}

fn document_ref_from_row(row: DocumentStatusRow) -> DocumentRef {
    let snapshot = document_state_from_fields(DocumentStateFields {
        document_id: row.id,
        intake_status: row.intake_status,
        accounting_status: row.accounting_status,
        accounting_requested_at: row.accounting_requested_at,
        accounting_review_reason: row.accounting_review_reason,
        accounting_export_batch_id: row.accounting_export_batch_id,
        latest_accounting_run_kind: row.latest_accounting_run_kind,
        latest_accounting_run_status: row.latest_accounting_run_status,
    });
    DocumentRef {
        id: row.id,
        short_ref: row.short_ref,
        intake_status: snapshot.intake_status.clone(),
        accounting_status: snapshot.accounting_status.clone(),
    }
}

fn accounting_candidate_from_row(row: DocumentStatusRow) -> AccountingProcessingCandidate {
    AccountingProcessingCandidate {
        id: row.id,
        short_ref: row.short_ref,
        intake_status: row.intake_status.unwrap_or_default(),
        accounting_status: row.accounting_status.unwrap_or_default(),
        accounting_requested_at: row.accounting_requested_at,
    }
}

fn retry_document_from_row(row: RetryDocumentRow) -> RetryDocument {
    let snapshot = document_state_from_fields(DocumentStateFields {
        document_id: row.id,
        intake_status: row.intake_status,
        accounting_status: row.accounting_status,
        accounting_requested_at: row.accounting_requested_at,
        accounting_review_reason: row.accounting_review_reason,
        accounting_export_batch_id: row.accounting_export_batch_id,
        latest_accounting_run_kind: row.latest_accounting_run_kind,
        latest_accounting_run_status: row.latest_accounting_run_status,
    });
    RetryDocument {
        id: row.id,
        short_ref: row.short_ref,
        intake_status: snapshot.intake_status.clone(),
        accounting_status: snapshot.accounting_status.clone(),
        review_reason: row
            .review_reason
            .or(snapshot.accounting_review_reason.clone()),
        original_path: row.original_path,
        mime_type: row.mime_type,
        filename: row.filename,
    }
}

fn document_why_from_row(row: DocumentWhyDetailsRow) -> DocumentWhyDetails {
    let snapshot = document_state_from_fields(DocumentStateFields {
        document_id: row.id,
        intake_status: row.intake_status,
        accounting_status: row.accounting_status,
        accounting_requested_at: row.accounting_requested_at,
        accounting_review_reason: row.accounting_review_reason,
        accounting_export_batch_id: row.accounting_export_batch_id,
        latest_accounting_run_kind: row.latest_accounting_run_kind,
        latest_accounting_run_status: row.latest_accounting_run_status,
    });
    DocumentWhyDetails {
        id: row.id,
        short_ref: row.short_ref,
        intake_status: snapshot.intake_status.clone(),
        accounting_status: snapshot.accounting_status.clone(),
        decision_type: row.decision_type,
        confidence_score: row.confidence_score,
        review_reason: row
            .review_reason
            .or(snapshot.accounting_review_reason.clone()),
        invoice_status: row.invoice_status,
    }
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

pub async fn workspace_identity(pool: &DbPool) -> anyhow::Result<Option<WorkspaceIdentity>> {
    let row = sqlx::query_as::<_, WorkspaceIdentity>(
        r#"
        SELECT
            NULLIF(TRIM(display_name), '') AS workspace_name,
            NULLIF(TRIM(jurisdiction), '') AS jurisdiction
        FROM company_profile
        WHERE singleton = TRUE
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

pub async fn document_status_counts(pool: &DbPool) -> anyhow::Result<DocumentStatusCounts> {
    let counts = sqlx::query_as(
        r#"
        SELECT
            COUNT(*) AS total_count,
            COALESCE(SUM(CASE WHEN dis.status = 'PROCESSING' THEN 1 ELSE 0 END), 0)
              + COALESCE(SUM(CASE WHEN das.status IN ('REQUESTED', 'ACCOUNTING', 'VALIDATING', 'EXPORTING') THEN 1 ELSE 0 END), 0) AS processing_count,
            COALESCE(SUM(CASE WHEN das.status = 'PENDING_REVIEW' THEN 1 ELSE 0 END), 0) AS pending_count,
            COALESCE(SUM(CASE WHEN das.status = 'READY_FOR_EXPORT' THEN 1 ELSE 0 END), 0) AS ready_count,
            COALESCE(SUM(CASE WHEN das.status = 'EXPORTED' THEN 1 ELSE 0 END), 0) AS exported_count,
            COALESCE(SUM(CASE WHEN dis.status = 'FAILED' THEN 1 ELSE 0 END), 0)
              + COALESCE(SUM(CASE WHEN das.status = 'FAILED' THEN 1 ELSE 0 END), 0) AS failed_count
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(counts)
}

pub async fn grouped_document_status_counts(
    pool: &DbPool,
) -> anyhow::Result<GroupedDocumentStatusCounts> {
    let counts = sqlx::query_as(
        r#"
        SELECT
            COUNT(*) AS documents_total,
            COALESCE(SUM(CASE WHEN dis.status = 'PROCESSING' THEN 1 ELSE 0 END), 0) AS intake_processing_count,
            COALESCE(SUM(CASE WHEN dis.status = 'INGESTED' THEN 1 ELSE 0 END), 0) AS intake_ingested_count,
            COALESCE(SUM(CASE WHEN dis.status = 'FAILED' THEN 1 ELSE 0 END), 0) AS intake_failed_count,
            COALESCE(SUM(CASE WHEN das.status IN ('REQUESTED', 'ACCOUNTING', 'VALIDATING', 'EXPORTING') THEN 1 ELSE 0 END), 0) AS accounting_processing_count,
            COALESCE(SUM(CASE WHEN das.status = 'PENDING_REVIEW' THEN 1 ELSE 0 END), 0) AS accounting_pending_review_count,
            COALESCE(SUM(CASE WHEN das.status = 'READY_FOR_EXPORT' THEN 1 ELSE 0 END), 0) AS accounting_ready_for_export_count,
            COALESCE(SUM(CASE WHEN das.status = 'EXPORTED' THEN 1 ELSE 0 END), 0) AS accounting_exported_count,
            COALESCE(SUM(CASE WHEN das.status = 'FAILED' THEN 1 ELSE 0 END), 0) AS accounting_failed_count
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(counts)
}

pub async fn list_documents_by_accounting_status(
    pool: &DbPool,
    accounting_status: &str,
    limit: i64,
) -> anyhow::Result<Vec<DocumentSummary>> {
    let query = document_summary_query(
        "EXISTS (
            SELECT 1 FROM document_accounting_state das
            WHERE das.document_id = d.id
              AND das.status = $1
        )",
        "$2",
    );
    let rows = sqlx::query_as::<_, DocumentSummaryRow>(&query)
        .bind(accounting_status)
        .bind(limit)
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(document_summary_from_row).collect())
}

pub async fn count_documents_by_accounting_status(
    pool: &DbPool,
    accounting_status: &str,
) -> anyhow::Result<i64> {
    let count = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM documents d
        JOIN document_accounting_state das ON das.document_id = d.id
        WHERE das.status = $1
        "#,
    )
    .bind(accounting_status)
    .fetch_one(pool)
    .await?;

    Ok(count)
}

pub async fn list_documents_requiring_attention(
    pool: &DbPool,
    limit: i64,
) -> anyhow::Result<Vec<DocumentSummary>> {
    let query = document_summary_query(
        "EXISTS (
            SELECT 1
            FROM document_intake_state dis
            LEFT JOIN document_accounting_state das ON das.document_id = dis.document_id
            WHERE dis.document_id = d.id
              AND (
                das.status = 'PENDING_REVIEW'
                OR dis.status = 'FAILED'
                OR das.status = 'FAILED'
              )
        )",
        "$1",
    );
    let rows = sqlx::query_as::<_, DocumentSummaryRow>(&query)
        .bind(limit)
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(document_summary_from_row).collect())
}

pub async fn count_documents_requiring_attention(pool: &DbPool) -> anyhow::Result<i64> {
    let count = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        WHERE das.status = 'PENDING_REVIEW'
           OR dis.status = 'FAILED'
           OR das.status = 'FAILED'
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
    let rows = sqlx::query_as::<_, DocumentSummaryRow>(&query)
        .bind(limit)
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(document_summary_from_row).collect())
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
    let row = sqlx::query_as::<_, DocumentSummaryRow>(&query)
        .bind(short_ref)
        .bind(1_i64)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(document_summary_from_row))
}

pub async fn document_ref_by_short_ref(
    pool: &DbPool,
    short_ref: &str,
) -> anyhow::Result<Option<DocumentRef>> {
    let row = sqlx::query_as::<_, DocumentStatusRow>(
        r#"
        SELECT
            d.id,
            d.short_ref,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.requested_at AS accounting_requested_at,
            das.review_reason AS accounting_review_reason,
            das.export_batch_id AS accounting_export_batch_id,
            (
                SELECT dar.run_kind
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_kind,
            (
                SELECT dar.run_status
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_status
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        WHERE d.short_ref = $1
        LIMIT 1
        "#,
    )
    .bind(short_ref)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(document_ref_from_row))
}

pub async fn accounting_processing_candidate_by_short_ref(
    pool: &DbPool,
    short_ref: &str,
) -> anyhow::Result<Option<AccountingProcessingCandidate>> {
    let row = sqlx::query_as::<_, DocumentStatusRow>(
        r#"
        SELECT
            d.id,
            d.short_ref,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.requested_at AS accounting_requested_at,
            das.review_reason AS accounting_review_reason,
            das.export_batch_id AS accounting_export_batch_id,
            (
                SELECT dar.run_kind
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_kind,
            (
                SELECT dar.run_status
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_status
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        WHERE short_ref = $1
        LIMIT 1
        "#,
    )
    .bind(short_ref)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(accounting_candidate_from_row))
}

pub async fn accounting_processing_candidates_by_short_refs(
    pool: &DbPool,
    short_refs: &[String],
) -> anyhow::Result<Vec<AccountingProcessingCandidate>> {
    if short_refs.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = std::iter::repeat_n("?", short_refs.len())
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        r#"
        SELECT
            d.id,
            d.short_ref,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.requested_at AS accounting_requested_at,
            das.review_reason AS accounting_review_reason,
            das.export_batch_id AS accounting_export_batch_id,
            (
                SELECT dar.run_kind
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_kind,
            (
                SELECT dar.run_status
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_status
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        WHERE d.short_ref IN ({placeholders})
        ORDER BY d.received_at DESC, d.id DESC
        "#
    );

    let mut built = sqlx::query_as::<_, DocumentStatusRow>(&query);
    for short_ref in short_refs {
        built = built.bind(short_ref);
    }

    Ok(built
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(accounting_candidate_from_row)
        .collect())
}

pub async fn accounting_processing_candidates_by_ids(
    pool: &DbPool,
    document_ids: &[i64],
) -> anyhow::Result<Vec<AccountingProcessingCandidate>> {
    if document_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = std::iter::repeat_n("?", document_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        r#"
        SELECT
            d.id,
            d.short_ref,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.requested_at AS accounting_requested_at,
            das.review_reason AS accounting_review_reason,
            das.export_batch_id AS accounting_export_batch_id,
            (
                SELECT dar.run_kind
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_kind,
            (
                SELECT dar.run_status
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_status
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        WHERE d.id IN ({placeholders})
        ORDER BY d.received_at DESC, d.id DESC
        "#
    );

    let mut built = sqlx::query_as::<_, DocumentStatusRow>(&query);
    for document_id in document_ids {
        built = built.bind(document_id);
    }

    Ok(built
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(accounting_candidate_from_row)
        .collect())
}

pub async fn list_accounting_eligible_documents(
    pool: &DbPool,
    limit: i64,
) -> anyhow::Result<Vec<DocumentSummary>> {
    let query = document_summary_query(
        "EXISTS (
            SELECT 1
            FROM document_intake_state dis
            JOIN document_accounting_state das ON das.document_id = dis.document_id
            WHERE dis.document_id = d.id
              AND dis.status = 'INGESTED'
              AND das.status = 'NOT_REQUESTED'
        )",
        "$1",
    );
    Ok(sqlx::query_as::<_, DocumentSummaryRow>(&query)
        .bind(limit)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(document_summary_from_row)
        .collect())
}

pub async fn count_accounting_eligible_documents(pool: &DbPool) -> anyhow::Result<i64> {
    let count = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM documents d
        JOIN document_intake_state dis ON dis.document_id = d.id
        JOIN document_accounting_state das ON das.document_id = d.id
        WHERE dis.status = 'INGESTED'
          AND das.status = 'NOT_REQUESTED'
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(count)
}

pub async fn accounting_eligible_candidates(
    pool: &DbPool,
) -> anyhow::Result<Vec<AccountingProcessingCandidate>> {
    let rows = sqlx::query_as::<_, DocumentStatusRow>(
        r#"
        SELECT
            d.id,
            d.short_ref,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.requested_at AS accounting_requested_at,
            das.review_reason AS accounting_review_reason,
            das.export_batch_id AS accounting_export_batch_id,
            (
                SELECT dar.run_kind
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_kind,
            (
                SELECT dar.run_status
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_status
        FROM documents d
        JOIN document_intake_state dis ON dis.document_id = d.id
        JOIN document_accounting_state das ON das.document_id = d.id
        WHERE dis.status = 'INGESTED'
          AND das.status = 'NOT_REQUESTED'
        ORDER BY d.received_at DESC, d.id DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(accounting_candidate_from_row)
        .collect())
}

pub async fn retry_document_by_short_ref(
    pool: &DbPool,
    short_ref: &str,
) -> anyhow::Result<Option<RetryDocument>> {
    let row = sqlx::query_as::<_, RetryDocumentRow>(
        r#"
        SELECT
            d.id,
            d.short_ref,
            d.original_path,
            d.mime_type,
            d.filename,
            (
                SELECT review_reason
                FROM review_decisions rd
                WHERE rd.document_id = d.id
                ORDER BY reviewed_at DESC, created_at DESC
                LIMIT 1
            ) AS review_reason,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.requested_at AS accounting_requested_at,
            das.review_reason AS accounting_review_reason,
            das.export_batch_id AS accounting_export_batch_id,
            (
                SELECT dar.run_kind
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_kind,
            (
                SELECT dar.run_status
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_status
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        WHERE d.short_ref = $1
        LIMIT 1
        "#,
    )
    .bind(short_ref)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(retry_document_from_row))
}

pub async fn document_why_details(
    pool: &DbPool,
    short_ref: &str,
) -> anyhow::Result<Option<DocumentWhyDetails>> {
    let row = sqlx::query_as::<_, DocumentWhyDetailsRow>(
        r#"
        SELECT
            d.id,
            d.short_ref,
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
            ) AS invoice_status,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.requested_at AS accounting_requested_at,
            das.review_reason AS accounting_review_reason,
            das.export_batch_id AS accounting_export_batch_id,
            (
                SELECT dar.run_kind
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_kind,
            (
                SELECT dar.run_status
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_status
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        WHERE d.short_ref = $1
        LIMIT 1
        "#,
    )
    .bind(short_ref)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(document_why_from_row))
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
            ) AS review_reason,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.requested_at AS accounting_requested_at,
            das.review_reason AS accounting_review_reason,
            das.export_batch_id AS accounting_export_batch_id,
            (
                SELECT dar.run_kind
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_kind,
            (
                SELECT dar.run_status
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_status
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
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
