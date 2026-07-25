use crate::db::DbPool;
use crate::error::AppResult;
use crate::queue::JobType;
use sqlx::{Sqlite, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountingRunKind {
    Accounting,
    Validation,
    Review,
    Export,
}

impl AccountingRunKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountingRunKind::Accounting => "ACCOUNTING",
            AccountingRunKind::Validation => "VALIDATION",
            AccountingRunKind::Review => "REVIEW",
            AccountingRunKind::Export => "EXPORT",
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DocumentStateSnapshot {
    pub document_id: i64,
    pub intake_status: Option<String>,
    pub intake_started_at: Option<String>,
    pub intake_completed_at: Option<String>,
    pub intake_failure_reason: Option<String>,
    pub accounting_status: Option<String>,
    pub accounting_requested_at: Option<String>,
    pub accounting_started_at: Option<String>,
    pub accounting_completed_at: Option<String>,
    pub accounting_failure_reason: Option<String>,
    pub accounting_review_reason: Option<String>,
    pub accounting_export_batch_id: Option<i64>,
    pub latest_accounting_run_kind: Option<String>,
    pub latest_accounting_run_status: Option<String>,
}

fn document_state_select() -> &'static str {
    r#"
    SELECT
      d.id AS document_id,
      dis.status AS intake_status,
      dis.started_at AS intake_started_at,
      dis.completed_at AS intake_completed_at,
      dis.failure_reason AS intake_failure_reason,
      das.status AS accounting_status,
      das.requested_at AS accounting_requested_at,
      das.started_at AS accounting_started_at,
      das.completed_at AS accounting_completed_at,
      das.failure_reason AS accounting_failure_reason,
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
    "#
}

async fn complete_latest_run(
    pool: &DbPool,
    table: &str,
    document_id: i64,
    extra_where: &str,
) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    complete_latest_run_tx(&mut tx, table, document_id, extra_where).await?;
    tx.commit().await?;
    Ok(())
}

async fn complete_latest_run_tx(
    tx: &mut Transaction<'_, Sqlite>,
    table: &str,
    document_id: i64,
    extra_where: &str,
) -> AppResult<()> {
    let sql = format!(
        r#"
        UPDATE {table}
        SET run_status = 'COMPLETED',
            completed_at = COALESCE(completed_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = (
            SELECT id
            FROM {table}
            WHERE document_id = $1
              AND run_status IN ('REQUESTED', 'RUNNING')
              {extra_where}
            ORDER BY created_at DESC, id DESC
            LIMIT 1
        )
        "#,
    );
    sqlx::query(&sql)
        .bind(document_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn fail_latest_run(
    pool: &DbPool,
    table: &str,
    document_id: i64,
    failure_reason: Option<&str>,
    extra_where: &str,
) -> AppResult<()> {
    let sql = format!(
        r#"
        UPDATE {table}
        SET run_status = 'FAILED',
            failure_reason = COALESCE($2, failure_reason),
            completed_at = COALESCE(completed_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = (
            SELECT id
            FROM {table}
            WHERE document_id = $1
              AND run_status IN ('REQUESTED', 'RUNNING')
              {extra_where}
            ORDER BY created_at DESC, id DESC
            LIMIT 1
        )
        "#,
    );
    sqlx::query(&sql)
        .bind(document_id)
        .bind(failure_reason)
        .execute(pool)
        .await?;
    Ok(())
}

async fn cancel_pending_accounting_run_tx(
    tx: &mut Transaction<'_, Sqlite>,
    document_id: i64,
    run_kind: AccountingRunKind,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_runs
        SET run_status = 'CANCELLED',
            completed_at = COALESCE(completed_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = (
            SELECT id
            FROM document_accounting_runs
            WHERE document_id = $1
              AND run_kind = $2
              AND run_status IN ('REQUESTED', 'RUNNING')
            ORDER BY created_at DESC, id DESC
            LIMIT 1
        )
        "#,
    )
    .bind(document_id)
    .bind(run_kind.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn fetch_document_state(
    pool: &DbPool,
    document_id: i64,
) -> AppResult<Option<DocumentStateSnapshot>> {
    let query = format!("{} WHERE d.id = $1", document_state_select());
    let snapshot = sqlx::query_as::<_, DocumentStateSnapshot>(&query)
        .bind(document_id)
        .fetch_optional(pool)
        .await?;
    Ok(snapshot)
}

pub async fn fetch_document_state_by_short_ref(
    pool: &DbPool,
    short_ref: &str,
) -> AppResult<Option<DocumentStateSnapshot>> {
    let query = format!("{} WHERE d.short_ref = $1 LIMIT 1", document_state_select());
    let snapshot = sqlx::query_as::<_, DocumentStateSnapshot>(&query)
        .bind(short_ref)
        .fetch_optional(pool)
        .await?;
    Ok(snapshot)
}

pub async fn initialize_document_state(pool: &DbPool, document_id: i64) -> AppResult<()> {
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO document_intake_state (
            document_id, status, created_at, updated_at
        )
        VALUES ($1, 'RECEIVED', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT OR IGNORE INTO document_accounting_state (
            document_id, status, created_at, updated_at
        )
        VALUES ($1, 'NOT_REQUESTED', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn mark_intake_received(pool: &DbPool, document_id: i64) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    mark_intake_received_in_tx(&mut tx, document_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn mark_intake_received_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    document_id: i64,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_intake_state
        SET status = 'RECEIVED',
            failure_reason = NULL,
            started_at = NULL,
            completed_at = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

pub async fn mark_intake_processing(pool: &DbPool, document_id: i64) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_intake_state
        SET status = 'PROCESSING',
            failure_reason = NULL,
            started_at = COALESCE(started_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO document_intake_runs (
            document_id,
            run_status,
            metadata,
            started_at,
            created_at,
            updated_at
        )
        SELECT $1, 'RUNNING', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        WHERE NOT EXISTS (
            SELECT 1
            FROM document_intake_runs
            WHERE document_id = $1
              AND run_status = 'RUNNING'
        )
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn mark_intake_ingested(pool: &DbPool, document_id: i64) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_intake_state
        SET status = 'INGESTED',
            failure_reason = NULL,
            completed_at = COALESCE(completed_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await?;

    complete_latest_run(pool, "document_intake_runs", document_id, "").await?;
    Ok(())
}

pub async fn mark_intake_failed(
    pool: &DbPool,
    document_id: i64,
    failure_reason: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_intake_state
        SET status = 'FAILED',
            failure_reason = COALESCE($2, failure_reason),
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .bind(failure_reason)
    .execute(pool)
    .await?;

    fail_latest_run(
        pool,
        "document_intake_runs",
        document_id,
        failure_reason,
        "",
    )
    .await?;
    Ok(())
}

pub async fn request_accounting(pool: &DbPool, document_id: i64) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    request_accounting_in_tx(&mut tx, document_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn request_accounting_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    document_id: i64,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET requested_at = COALESCE(requested_at, CURRENT_TIMESTAMP),
            status = CASE
                WHEN status = 'NOT_REQUESTED' THEN 'REQUESTED'
                ELSE status
            END,
            failure_reason = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO document_accounting_runs (
            document_id,
            run_kind,
            run_status,
            metadata,
            created_at,
            updated_at
        )
        SELECT $1, 'ACCOUNTING', 'REQUESTED', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        WHERE NOT EXISTS (
            SELECT 1
            FROM document_accounting_runs
            WHERE document_id = $1
              AND run_kind = 'ACCOUNTING'
              AND run_status IN ('REQUESTED', 'RUNNING')
        )
        "#,
    )
    .bind(document_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

pub async fn start_accounting(pool: &DbPool, document_id: i64) -> AppResult<()> {
    start_accounting_with_metadata(pool, document_id, None, None, None).await
}

pub async fn start_accounting_with_metadata(
    pool: &DbPool,
    document_id: i64,
    provider: Option<&str>,
    model_used: Option<&str>,
    prompt_path: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'ACCOUNTING',
            requested_at = COALESCE(requested_at, CURRENT_TIMESTAMP),
            started_at = COALESCE(started_at, CURRENT_TIMESTAMP),
            failure_reason = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO document_accounting_runs (
            document_id,
            run_kind,
            run_status,
            provider,
            model_used,
            prompt_path,
            metadata,
            started_at,
            created_at,
            updated_at
        )
        SELECT $1, 'ACCOUNTING', 'RUNNING', $2, $3, $4, '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        WHERE NOT EXISTS (
            SELECT 1
            FROM document_accounting_runs
            WHERE document_id = $1
              AND run_kind = 'ACCOUNTING'
              AND run_status = 'RUNNING'
        )
        "#,
    )
    .bind(document_id)
    .bind(provider)
    .bind(model_used)
    .bind(prompt_path)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn start_validation(pool: &DbPool, document_id: i64) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    start_validation_in_tx(&mut tx, document_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn start_validation_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    document_id: i64,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'VALIDATING',
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO document_accounting_runs (
            document_id,
            run_kind,
            run_status,
            metadata,
            started_at,
            created_at,
            updated_at
        )
        SELECT $1, 'VALIDATION', 'RUNNING', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        WHERE NOT EXISTS (
            SELECT 1
            FROM document_accounting_runs
            WHERE document_id = $1
              AND run_kind = 'VALIDATION'
              AND run_status = 'RUNNING'
        )
        "#,
    )
    .bind(document_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

pub async fn complete_accountant_review(pool: &DbPool, document_id: i64) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'ACCOUNTING',
            requested_at = COALESCE(requested_at, CURRENT_TIMESTAMP),
            started_at = COALESCE(started_at, CURRENT_TIMESTAMP),
            completed_at = COALESCE(completed_at, CURRENT_TIMESTAMP),
            failure_reason = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await?;

    complete_latest_run(
        pool,
        "document_accounting_runs",
        document_id,
        "AND run_kind = 'ACCOUNTING'",
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO document_accounting_runs (
            document_id,
            run_kind,
            run_status,
            metadata,
            started_at,
            completed_at,
            created_at,
            updated_at
        )
        SELECT $1, 'ACCOUNTING', 'COMPLETED', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        WHERE NOT EXISTS (
            SELECT 1
            FROM document_accounting_runs
            WHERE document_id = $1
              AND run_kind = 'ACCOUNTING'
        )
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn complete_validation(pool: &DbPool, document_id: i64) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'VALIDATING',
            requested_at = COALESCE(requested_at, CURRENT_TIMESTAMP),
            started_at = COALESCE(started_at, CURRENT_TIMESTAMP),
            completed_at = COALESCE(completed_at, CURRENT_TIMESTAMP),
            failure_reason = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await?;

    complete_latest_run(
        pool,
        "document_accounting_runs",
        document_id,
        "AND run_kind = 'VALIDATION'",
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO document_accounting_runs (
            document_id,
            run_kind,
            run_status,
            metadata,
            started_at,
            completed_at,
            created_at,
            updated_at
        )
        SELECT $1, 'VALIDATION', 'COMPLETED', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        WHERE NOT EXISTS (
            SELECT 1
            FROM document_accounting_runs
            WHERE document_id = $1
              AND run_kind = 'VALIDATION'
        )
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_accounting_failed(
    pool: &DbPool,
    document_id: i64,
    failure_reason: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'FAILED',
            failure_reason = COALESCE($2, failure_reason),
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .bind(failure_reason)
    .execute(pool)
    .await?;

    fail_latest_run(
        pool,
        "document_accounting_runs",
        document_id,
        failure_reason,
        "",
    )
    .await?;
    Ok(())
}

pub async fn request_human_review(
    pool: &DbPool,
    document_id: i64,
    review_reason: Option<&str>,
) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    request_human_review_in_tx(&mut tx, document_id, review_reason).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn request_human_review_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    document_id: i64,
    review_reason: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'PENDING_REVIEW',
            requested_at = COALESCE(requested_at, CURRENT_TIMESTAMP),
            review_reason = COALESCE($2, review_reason),
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .bind(review_reason)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO document_accounting_runs (
            document_id,
            run_kind,
            actor_type,
            action_type,
            run_status,
            metadata,
            started_at,
            created_at,
            updated_at
        )
        SELECT $1, 'REVIEW', 'HUMAN', 'REQUEST', 'REQUESTED', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        WHERE NOT EXISTS (
            SELECT 1
            FROM document_accounting_runs
            WHERE document_id = $1
              AND run_kind = 'REVIEW'
              AND run_status IN ('REQUESTED', 'RUNNING')
        )
        "#,
    )
    .bind(document_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

pub async fn clear_human_review(pool: &DbPool, document_id: i64) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    clear_human_review_in_tx(&mut tx, document_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn clear_human_review_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    document_id: i64,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'VALIDATING',
            review_reason = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .execute(&mut **tx)
    .await?;

    cancel_pending_accounting_run_tx(tx, document_id, AccountingRunKind::Review).await?;
    Ok(())
}

pub async fn complete_review(
    pool: &DbPool,
    document_id: i64,
    review_reason: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'VALIDATING',
            completed_at = COALESCE(completed_at, CURRENT_TIMESTAMP),
            review_reason = COALESCE($2, review_reason),
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .bind(review_reason)
    .execute(pool)
    .await?;

    complete_latest_run(
        pool,
        "document_accounting_runs",
        document_id,
        "AND run_kind = 'REVIEW'",
    )
    .await?;
    Ok(())
}

pub async fn mark_export_not_ready(pool: &DbPool, document_id: i64) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    mark_export_not_ready_in_tx(&mut tx, document_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn mark_export_not_ready_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    document_id: i64,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'VALIDATING',
            export_batch_id = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

pub async fn mark_export_ready(pool: &DbPool, document_id: i64) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'READY_FOR_EXPORT',
            requested_at = COALESCE(requested_at, CURRENT_TIMESTAMP),
            review_reason = NULL,
            failure_reason = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO document_accounting_runs (
            document_id,
            run_kind,
            run_status,
            metadata,
            started_at,
            created_at,
            updated_at
        )
        SELECT $1, 'EXPORT', 'REQUESTED', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        WHERE NOT EXISTS (
            SELECT 1
            FROM document_accounting_runs
            WHERE document_id = $1
              AND run_kind = 'EXPORT'
              AND run_status IN ('REQUESTED', 'RUNNING')
        )
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn start_exporting(
    pool: &DbPool,
    document_id: i64,
    export_batch_id: Option<i64>,
) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    start_exporting_in_tx(&mut tx, document_id, export_batch_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn start_exporting_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    document_id: i64,
    export_batch_id: Option<i64>,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'EXPORTING',
            export_batch_id = COALESCE($2, export_batch_id),
            requested_at = COALESCE(requested_at, CURRENT_TIMESTAMP),
            started_at = COALESCE(started_at, CURRENT_TIMESTAMP),
            failure_reason = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .bind(export_batch_id)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE document_accounting_runs
        SET run_status = 'RUNNING',
            export_batch_id = COALESCE($2, export_batch_id),
            started_at = COALESCE(started_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = (
            SELECT id
            FROM document_accounting_runs
            WHERE document_id = $1
              AND run_kind = 'EXPORT'
              AND run_status = 'REQUESTED'
            ORDER BY created_at DESC, id DESC
            LIMIT 1
        )
        "#,
    )
    .bind(document_id)
    .bind(export_batch_id)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO document_accounting_runs (
            document_id,
            run_kind,
            export_batch_id,
            run_status,
            metadata,
            started_at,
            created_at,
            updated_at
        )
        SELECT $1, 'EXPORT', $2, 'RUNNING', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        WHERE NOT EXISTS (
            SELECT 1
            FROM document_accounting_runs
            WHERE document_id = $1
              AND run_kind = 'EXPORT'
              AND run_status = 'RUNNING'
        )
        "#,
    )
    .bind(document_id)
    .bind(export_batch_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

pub async fn mark_exported(pool: &DbPool, document_id: i64, export_batch_id: i64) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    mark_exported_in_tx(&mut tx, document_id, export_batch_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn mark_exported_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    document_id: i64,
    export_batch_id: i64,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'EXPORTED',
            export_batch_id = $2,
            requested_at = COALESCE(requested_at, CURRENT_TIMESTAMP),
            started_at = COALESCE(started_at, CURRENT_TIMESTAMP),
            completed_at = CURRENT_TIMESTAMP,
            failure_reason = NULL,
            review_reason = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .bind(export_batch_id)
    .execute(&mut **tx)
    .await?;

    complete_latest_run_tx(
        tx,
        "document_accounting_runs",
        document_id,
        "AND run_kind = 'EXPORT'",
    )
    .await?;
    sqlx::query(
        r#"
        UPDATE document_accounting_runs
        SET export_batch_id = COALESCE($2, export_batch_id),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = (
            SELECT id
            FROM document_accounting_runs
            WHERE document_id = $1
              AND run_kind = 'EXPORT'
            ORDER BY created_at DESC, id DESC
            LIMIT 1
        )
        "#,
    )
    .bind(document_id)
    .bind(export_batch_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn mark_export_failed(
    pool: &DbPool,
    document_id: i64,
    failure_reason: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'FAILED',
            failure_reason = COALESCE($2, failure_reason),
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .bind(failure_reason)
    .execute(pool)
    .await?;

    fail_latest_run(
        pool,
        "document_accounting_runs",
        document_id,
        failure_reason,
        "AND run_kind = 'EXPORT'",
    )
    .await?;
    Ok(())
}

pub async fn reset_document_state(pool: &DbPool, document_id: i64) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    reset_document_state_in_tx(&mut tx, document_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn reset_document_state_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    document_id: i64,
) -> AppResult<()> {
    mark_intake_received_in_tx(tx, document_id).await?;
    sqlx::query(
        r#"
        UPDATE document_accounting_state
        SET status = 'NOT_REQUESTED',
            requested_at = NULL,
            started_at = NULL,
            completed_at = NULL,
            failure_reason = NULL,
            review_reason = NULL,
            export_batch_id = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE document_id = $1
        "#,
    )
    .bind(document_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn mark_job_failed(
    pool: &DbPool,
    document_id: i64,
    job_type: JobType,
    failure_reason: Option<&str>,
) -> AppResult<()> {
    match job_type {
        JobType::Vision => mark_intake_failed(pool, document_id, failure_reason).await,
        JobType::Accountant | JobType::Validator | JobType::Review => {
            mark_accounting_failed(pool, document_id, failure_reason).await
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestDocumentStatePreset {
    IntakeReceived,
    IntakeProcessing,
    IntakeIngested,
    IntakeFailed,
    AccountingRequested,
    AccountingRunning,
    AccountingCompleted,
    ValidationRunning,
    ValidationCompleted,
    PendingReview,
    ReadyForExport,
    Exporting,
    Exported,
    AccountingFailed,
}

#[cfg(test)]
pub async fn seed_document_state_preset(
    pool: &DbPool,
    document_id: i64,
    preset: TestDocumentStatePreset,
) -> AppResult<()> {
    async fn seed_accountant_reviewed(pool: &DbPool, document_id: i64) -> AppResult<()> {
        mark_intake_processing(pool, document_id).await?;
        mark_intake_ingested(pool, document_id).await?;
        request_accounting(pool, document_id).await?;
        start_accounting_with_metadata(pool, document_id, None, None, None).await?;
        complete_accountant_review(pool, document_id).await?;
        Ok(())
    }

    async fn seed_validated(pool: &DbPool, document_id: i64) -> AppResult<()> {
        seed_accountant_reviewed(pool, document_id).await?;
        start_validation(pool, document_id).await?;
        complete_validation(pool, document_id).await?;
        Ok(())
    }

    initialize_document_state(pool, document_id).await?;
    reset_document_state(pool, document_id).await?;

    match preset {
        TestDocumentStatePreset::IntakeReceived => {}
        TestDocumentStatePreset::IntakeProcessing => {
            mark_intake_processing(pool, document_id).await?
        }
        TestDocumentStatePreset::IntakeIngested => {
            mark_intake_processing(pool, document_id).await?;
            mark_intake_ingested(pool, document_id).await?;
        }
        TestDocumentStatePreset::IntakeFailed => {
            mark_intake_processing(pool, document_id).await?;
            mark_intake_failed(pool, document_id, Some("test_failed")).await?;
        }
        TestDocumentStatePreset::AccountingRequested => {
            mark_intake_processing(pool, document_id).await?;
            mark_intake_ingested(pool, document_id).await?;
            request_accounting(pool, document_id).await?;
        }
        TestDocumentStatePreset::AccountingRunning => {
            mark_intake_processing(pool, document_id).await?;
            mark_intake_ingested(pool, document_id).await?;
            request_accounting(pool, document_id).await?;
            start_accounting_with_metadata(pool, document_id, None, None, None).await?;
        }
        TestDocumentStatePreset::AccountingCompleted => {
            seed_accountant_reviewed(pool, document_id).await?
        }
        TestDocumentStatePreset::ValidationRunning => {
            seed_accountant_reviewed(pool, document_id).await?;
            start_validation(pool, document_id).await?;
        }
        TestDocumentStatePreset::ValidationCompleted => seed_validated(pool, document_id).await?,
        TestDocumentStatePreset::PendingReview => {
            seed_validated(pool, document_id).await?;
            request_human_review(pool, document_id, None).await?;
        }
        TestDocumentStatePreset::ReadyForExport => {
            seed_validated(pool, document_id).await?;
            complete_review(pool, document_id, None).await?;
            mark_export_ready(pool, document_id).await?;
        }
        TestDocumentStatePreset::Exporting => {
            seed_validated(pool, document_id).await?;
            complete_review(pool, document_id, None).await?;
            mark_export_ready(pool, document_id).await?;
            start_exporting(pool, document_id, None).await?;
        }
        TestDocumentStatePreset::Exported => {
            seed_validated(pool, document_id).await?;
            complete_review(pool, document_id, None).await?;
            mark_export_ready(pool, document_id).await?;
            sqlx::query(
                "INSERT OR IGNORE INTO users (id, role, email, password_hash) VALUES (1, 'admin', 'test@example.com', 'hash')",
            )
            .execute(pool)
            .await?;
            let export_batch_id: i64 = sqlx::query_scalar(
                "INSERT INTO export_batches (user_id, document_count, filter_criteria) VALUES (1, 1, '{}') RETURNING id",
            )
            .fetch_one(pool)
            .await?;
            mark_exported(pool, document_id, export_batch_id).await?;
        }
        TestDocumentStatePreset::AccountingFailed => {
            mark_intake_processing(pool, document_id).await?;
            mark_intake_ingested(pool, document_id).await?;
            request_accounting(pool, document_id).await?;
            start_accounting_with_metadata(pool, document_id, None, None, None).await?;
            mark_accounting_failed(pool, document_id, Some("test_failed")).await?;
        }
    }

    Ok(())
}
