#[cfg(feature = "ssr")]
use crate::db::DbPool;
#[cfg(feature = "ssr")]
use crate::document_state::fetch_document_state_by_short_ref;
#[cfg(feature = "ssr")]
use crate::web::server::auth::{pool, require_session_workspace_id};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(feature = "ssr")]
use sqlx::Row;
#[cfg(feature = "ssr")]
use tower_sessions::Session;

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct DocumentListFilters {
    pub month: Option<String>,
    pub intake_status: Option<String>,
    pub accounting_status: Option<String>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct DashboardSummary {
    pub company_name: String,
    pub company_org_nr: Option<String>,
    pub onboarding_required: bool,
    pub document_counts: DocumentCounts,
    pub recent_transactions: Vec<WebDocumentItem>,
    pub monthly_closes: Vec<MonthlyCloseItem>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct DocumentCounts {
    pub total: i64,
    pub completed: i64,
    pub completion_rate: i64,
    pub processing: i64,
    pub pending: i64,
    pub ready: i64,
    pub exported: i64,
    pub failed: i64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct MonthlyCloseItem {
    pub month_key: String,
    pub month_label: String,
    pub total_documents: i64,
    pub completed_documents: i64,
    pub completion_rate: i64,
    pub closed: bool,
}

#[server(GetDashboardSummary, "/_server_fn")]
pub async fn get_dashboard_summary() -> Result<DashboardSummary, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let row = sqlx::query(
        r#"
        SELECT
            COUNT(*) AS total_documents,
            COALESCE(SUM(CASE WHEN dis.status = 'PROCESSING' THEN 1 ELSE 0 END), 0)
              + COALESCE(SUM(CASE WHEN das.status IN ('REQUESTED', 'ACCOUNTING', 'VALIDATING', 'EXPORTING') THEN 1 ELSE 0 END), 0) AS processing,
            COALESCE(SUM(CASE WHEN das.status = 'PENDING_REVIEW' THEN 1 ELSE 0 END), 0) AS pending,
            COALESCE(SUM(CASE WHEN das.status = 'READY_FOR_EXPORT' THEN 1 ELSE 0 END), 0) AS ready,
            COALESCE(SUM(CASE WHEN das.status = 'EXPORTED' THEN 1 ELSE 0 END), 0) AS exported,
            COALESCE(SUM(CASE WHEN dis.status = 'FAILED' THEN 1 ELSE 0 END), 0)
              + COALESCE(SUM(CASE WHEN das.status = 'FAILED' THEN 1 ELSE 0 END), 0) AS failed
        FROM documents d
        LEFT JOIN document_intake_state dis ON dis.document_id = d.id
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        "#,
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    let total = row.try_get::<i64, _>("total_documents").unwrap_or(0);
    let processing = row.try_get::<i64, _>("processing").unwrap_or(0);
    let pending = row.try_get::<i64, _>("pending").unwrap_or(0);
    let ready = row.try_get::<i64, _>("ready").unwrap_or(0);
    let exported = row.try_get::<i64, _>("exported").unwrap_or(0);
    let failed = row.try_get::<i64, _>("failed").unwrap_or(0);
    let completed = ready + exported;
    let completion_rate = if total == 0 {
        0
    } else {
        (completed * 100) / total
    };

    let counts = DocumentCounts {
        total,
        completed,
        completion_rate,
        processing,
        pending,
        ready,
        exported,
        failed,
    };

    let recent_transactions = query_documents(&pool, &DocumentListFilters::default(), 5, 0).await?;

    let monthly_rows = sqlx::query(
        r#"
        SELECT
            strftime('%Y-%m', received_at) AS month_key,
            strftime('%Y-%m', received_at) AS month_label,
            COUNT(*) AS total_documents,
            COUNT(*) FILTER (WHERE das.status IN ('READY_FOR_EXPORT', 'EXPORTED')) AS completed_documents
        FROM documents d
        LEFT JOIN document_accounting_state das ON das.document_id = d.id
        GROUP BY 1, 2
        ORDER BY month_key DESC
        LIMIT 8
        "#,
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    let monthly_closes = monthly_rows
        .into_iter()
        .map(|row| {
            let total_documents = row.try_get::<i64, _>("total_documents").unwrap_or(0);
            let completed_documents = row.try_get::<i64, _>("completed_documents").unwrap_or(0);
            let completion_rate = if total_documents == 0 {
                0
            } else {
                (completed_documents * 100) / total_documents
            };
            MonthlyCloseItem {
                month_key: row.try_get("month_key").unwrap_or_default(),
                month_label: row.try_get("month_label").unwrap_or_default(),
                total_documents,
                completed_documents,
                completion_rate,
                closed: completion_rate == 100 && total_documents > 0,
            }
        })
        .collect::<Vec<_>>();

    let company_row = sqlx::query(
        r#"
        SELECT display_name, org_nr
        FROM company_profile
        WHERE singleton = TRUE
        LIMIT 1
        "#,
    )
    .fetch_optional(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;
    let company_name_raw = company_row
        .as_ref()
        .and_then(|row| row.try_get::<String, _>("display_name").ok())
        .unwrap_or_default();
    let onboarding_required = company_name_raw.trim().is_empty();
    let company_name = if onboarding_required {
        "Company setup required".to_string()
    } else {
        company_name_raw
    };
    let company_org_nr = company_row
        .and_then(|row| row.try_get::<Option<String>, _>("org_nr").ok())
        .flatten();
    Ok(DashboardSummary {
        company_name,
        company_org_nr,
        onboarding_required,
        document_counts: counts,
        recent_transactions,
        monthly_closes,
    })
}

#[cfg(feature = "ssr")]
async fn query_documents(
    pool: &DbPool,
    filters: &DocumentListFilters,
    limit: i64,
    offset: i64,
) -> Result<Vec<WebDocumentItem>, ServerFnError> {
    let search = filters
        .search
        .clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let rows = sqlx::query_as::<_, WebDocumentDbRow>(
        r#"
        SELECT
            d.id,
            d.short_ref,
            date(d.received_at) AS received_date,
            (
                SELECT parsed_value
                FROM extracted_fields ef
                WHERE ef.document_id = d.id AND ef.field_type = 'supplier_name'
                ORDER BY ef.updated_at DESC, ef.created_at DESC
                LIMIT 1
            ) AS supplier_name,
            (
                SELECT parsed_value
                FROM extracted_fields ef
                WHERE ef.document_id = d.id AND ef.field_type = 'transaction_date'
                ORDER BY ef.updated_at DESC, ef.created_at DESC
                LIMIT 1
            ) AS invoice_date,
            (
                SELECT parsed_value
                FROM extracted_fields ef
                WHERE ef.document_id = d.id AND ef.field_type = 'total_amount'
                ORDER BY ef.updated_at DESC, ef.created_at DESC
                LIMIT 1
            ) AS total_amount,
            (
                SELECT ai_confidence
                FROM accounting_decisions ad
                WHERE ad.document_id = d.id
                ORDER BY ad.created_at DESC
                LIMIT 1
            ) AS ai_confidence,
            (
                SELECT model_used
                FROM accounting_decisions ad
                WHERE ad.document_id = d.id
                ORDER BY ad.created_at DESC
                LIMIT 1
            ) AS model_used,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.review_reason AS review_reason
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
        LIMIT $5 OFFSET $6
        "#,
    )
    .bind(filters.month.clone())
    .bind(search)
    .bind(filters.intake_status.clone())
    .bind(filters.accounting_status.clone())
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    Ok(rows
        .into_iter()
        .map(|r| WebDocumentItem {
            id: Some(r.id),
            short_ref: r.short_ref,
            status: WebDocumentStatusSummary {
                intake: WebDomainStatusSummary {
                    status: r.intake_status,
                },
                accounting: WebAccountingStatusSummary {
                    status: r.accounting_status,
                    review_reason: r.review_reason,
                },
            },
            supplier_name: r.supplier_name,
            invoice_date: r.invoice_date,
            total_amount: r.total_amount,
            received_date: r.received_date,
            ai_confidence: r.ai_confidence,
            model_used: r.model_used,
        })
        .skip(offset as usize)
        .take(limit as usize)
        .collect())
}

#[server(CompleteCompanyOnboarding, "/_server_fn")]
pub async fn complete_company_onboarding(company_name: String) -> Result<(), ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let company_name = company_name.trim();
    if company_name.is_empty() {
        return Err(ServerFnError::new("Company name is required."));
    }

    sqlx::query(
        r#"
        UPDATE company_profile
        SET display_name = $1, updated_at = CURRENT_TIMESTAMP
        WHERE singleton = TRUE
        "#,
    )
    .bind(company_name)
    .execute(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    Ok(())
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WebDocumentItem {
    pub id: Option<i64>,
    pub short_ref: String,
    pub status: WebDocumentStatusSummary,
    pub supplier_name: Option<String>,
    pub invoice_date: Option<String>,
    pub total_amount: Option<String>,
    pub received_date: Option<String>,
    pub ai_confidence: Option<f64>,
    pub model_used: Option<String>,
}

#[cfg(feature = "ssr")]
#[derive(Clone, Debug, sqlx::FromRow)]
struct WebDocumentDbRow {
    id: i64,
    short_ref: String,
    received_date: Option<String>,
    supplier_name: Option<String>,
    invoice_date: Option<String>,
    total_amount: Option<String>,
    ai_confidence: Option<f64>,
    model_used: Option<String>,
    intake_status: Option<String>,
    accounting_status: Option<String>,
    review_reason: Option<String>,
}

#[cfg(feature = "ssr")]
#[derive(Clone, Debug, sqlx::FromRow)]
struct WebDocumentDetailsRow {
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
    net_amount: Option<String>,
    assigned_account_code: Option<String>,
    account_name: Option<String>,
    ai_confidence: Option<f64>,
    model_used: Option<String>,
    document_type: Option<String>,
    review_confidence: Option<f64>,
    review_decision_type: Option<String>,
    review_reason: Option<String>,
    validation_errors: Option<serde_json::Value>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct WebDocumentStatusSummary {
    pub intake: WebDomainStatusSummary,
    pub accounting: WebAccountingStatusSummary,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct WebDomainStatusSummary {
    pub status: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct WebAccountingStatusSummary {
    pub status: Option<String>,
    pub review_reason: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct TransactionsResponse {
    pub items: Vec<WebDocumentItem>,
    pub total_documents: i64,
    pub pending_documents: i64,
    pub done_documents: i64,
    pub completion_rate: i64,
}

#[server(GetDocumentList, "/_server_fn")]
pub async fn get_document_list(
    filters: DocumentListFilters,
) -> Result<TransactionsResponse, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let month_clean = filters
        .month
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    let intake_status_clean = filters
        .intake_status
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let accounting_status_clean = filters
        .accounting_status
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let search_clean = filters
        .search
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let limit = filters.limit.unwrap_or(100).clamp(1, 200);
    let offset = filters.offset.unwrap_or(0).max(0);

    let normalized_filters = DocumentListFilters {
        month: month_clean,
        intake_status: intake_status_clean,
        accounting_status: accounting_status_clean,
        search: search_clean,
        limit: Some(limit),
        offset: Some(offset),
    };

    let all_items = query_documents(&pool, &normalized_filters, i64::MAX, 0).await?;
    let items = query_documents(&pool, &normalized_filters, limit, offset).await?;

    let total_documents = all_items.len() as i64;
    let pending_documents = all_items
        .iter()
        .filter(|item| {
            matches!(
                item.status.accounting.status.as_deref(),
                Some("PENDING_REVIEW")
            )
        })
        .count() as i64;
    let done_documents = all_items
        .iter()
        .filter(|item| {
            matches!(
                item.status.accounting.status.as_deref(),
                Some("READY_FOR_EXPORT" | "EXPORTED")
            )
        })
        .count() as i64;
    let completion_rate = if total_documents == 0 {
        0
    } else {
        (done_documents * 100) / total_documents
    };

    Ok(TransactionsResponse {
        items,
        total_documents,
        pending_documents,
        done_documents,
        completion_rate,
    })
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct DocumentDetails {
    pub short_ref: String,
    pub status: WebDocumentStatusSummary,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub original_path: Option<String>,
    pub image_url: Option<String>,
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
    pub accounting_rows: Vec<AccountingRow>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct AccountingRow {
    pub account_code: String,
    pub description: Option<String>,
    pub amount: Option<String>,
    pub vat_code: Option<String>,
    pub is_debit: Option<bool>,
}

#[server(GetDocumentDetails, "/_server_fn")]
pub async fn get_document_details(
    short_ref: String,
) -> Result<Option<DocumentDetails>, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let short_ref = short_ref.trim().to_string();
    if short_ref.is_empty() {
        return Ok(None);
    }

    let row = sqlx::query_as::<_, WebDocumentDetailsRow>(
        r#"
        SELECT
            d.id,
            d.short_ref,
            d.filename,
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
    .bind(&short_ref)
    .fetch_optional(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    let Some(r) = row else {
        return Ok(None);
    };

    let document_state = fetch_document_state_by_short_ref(&pool, &short_ref)
        .await
        .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;
    let accounting_rows = sqlx::query(
        r#"
        SELECT aa.account_code, aa.description, CAST(aa.amount AS TEXT) AS amount, aa.vat_code, aa.is_debit
        FROM invoices i
        JOIN account_assignments aa ON aa.invoice_id = i.id
        WHERE i.document_id = $1
        ORDER BY aa.sort_order ASC, aa.created_at ASC
        "#,
    )
    .bind(r.id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?
    .into_iter()
    .map(|row| AccountingRow {
        account_code: row.try_get("account_code").unwrap_or_default(),
        description: row.try_get("description").ok(),
        amount: row.try_get("amount").ok(),
        vat_code: row.try_get("vat_code").ok(),
        is_debit: row.try_get("is_debit").ok(),
    })
    .collect::<Vec<_>>();

    Ok(Some(DocumentDetails {
        short_ref: r.short_ref.clone(),
        status: document_state
            .as_ref()
            .map(|snapshot| WebDocumentStatusSummary {
                intake: WebDomainStatusSummary {
                    status: snapshot.intake_status.clone(),
                },
                accounting: WebAccountingStatusSummary {
                    status: snapshot.accounting_status.clone(),
                    review_reason: snapshot.accounting_review_reason.clone(),
                },
            })
            .unwrap_or_default(),
        filename: r.filename,
        mime_type: r.mime_type,
        original_path: r.original_path,
        image_url: Some(format!("/_documents/{}/image", r.short_ref)),
        supplier_name: r.supplier_name,
        transaction_date: r.transaction_date,
        total_amount: r.total_amount,
        vat_amount: r.vat_amount,
        subtotal_amount: r.subtotal_amount,
        net_amount: r.net_amount,
        assigned_account_code: r.assigned_account_code,
        account_name: r.account_name,
        ai_confidence: r.ai_confidence,
        model_used: r.model_used,
        document_type: r.document_type,
        review_confidence: r.review_confidence,
        review_decision_type: r.review_decision_type,
        review_reason: r.review_reason,
        validation_errors: r.validation_errors,
        accounting_rows,
    }))
}
