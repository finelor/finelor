#[cfg(feature = "ssr")]
use crate::db::DbPool;
#[cfg(feature = "ssr")]
use argon2::{
    Argon2, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
#[cfg(feature = "ssr")]
use axum::http::HeaderMap;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(feature = "ssr")]
use sqlx::Row;
#[cfg(feature = "ssr")]
use tower_sessions::Session;
use uuid::Uuid;

#[cfg(feature = "ssr")]
pub use crate::web::server::channels::connect_telegram_for_workspace;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AuthUser {
    pub id: i64,
    pub email: String,
    pub display_name: Option<String>,
    pub workspace_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(feature = "ssr")]
pub fn pool() -> DbPool {
    crate::web::pool::get_pool()
}

#[cfg(feature = "ssr")]
pub async fn require_session_workspace_id(session: &Session) -> Result<Uuid, ServerFnError> {
    let user_id: Option<String> = session
        .get("user_id")
        .await
        .map_err(|e| ServerFnError::new(format!("session get user_id: {}", e)))?;
    if user_id.is_none() {
        return Err(ServerFnError::new("Not authenticated."));
    }
    Ok(crate::workspace::active_workspace_id())
}

#[server(Signup, "/api")]
pub async fn signup(
    full_name: String,
    email: String,
    password: String,
) -> Result<String, ServerFnError> {
    let headers: HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract headers: {}", e)))?;
    let full_name = full_name.trim();
    if full_name.is_empty() {
        return Err(ServerFnError::new("Full name is required."));
    }
    if email.trim().is_empty() || !email.contains('@') {
        return Err(ServerFnError::new("Invalid email address."));
    }
    if password.len() < 6 {
        return Err(ServerFnError::new(
            "Password must be at least 6 characters.",
        ));
    }
    let pool = pool();
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ServerFnError::new(format!("begin tx: {}", e)))?;

    let admin_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users)")
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| ServerFnError::new(format!("check existing admin: {}", e)))?;
    if admin_exists {
        return Err(ServerFnError::new(
            "Signup is closed: this deployment already has an admin account.",
        ));
    }

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| ServerFnError::new(format!("Password hashing failed: {}", e)))?
        .to_string();

    let user: crate::db::User = sqlx::query_as(
        r#"
        INSERT INTO users (role, email, password_hash, display_name)
        VALUES ('admin', $1, $2, $3)
        RETURNING id, role, email, password_hash, display_name, created_at, updated_at
        "#,
    )
    .bind(email.trim().to_lowercase())
    .bind(password_hash)
    .bind(Some(full_name.to_string()))
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| ServerFnError::new(format!("Failed to create user: {}", e)))?;

    sqlx::query(
        r#"
        INSERT INTO company_profile (singleton, display_name)
        VALUES (TRUE, NULL)
        ON CONFLICT (singleton) DO NOTHING
        "#,
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| ServerFnError::new(format!("Failed to initialize company profile: {}", e)))?;

    tx.commit()
        .await
        .map_err(|e| ServerFnError::new(format!("commit tx: {}", e)))?;

    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    session
        .insert("user_id", user.id.to_string())
        .await
        .map_err(|e| ServerFnError::new(format!("session insert user_id: {}", e)))?;

    let accepts_html = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);
    if accepts_html {
        leptos_axum::redirect("/dashboard");
    }

    Ok(user.id.to_string())
}

#[server(Login, "/api")]
pub async fn login(email: String, password: String) -> Result<String, ServerFnError> {
    let headers: HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract headers: {}", e)))?;
    let pool = pool();

    let user: Option<crate::db::User> = sqlx::query_as(
        r#"
        SELECT id, role, email, password_hash, display_name, created_at, updated_at
        FROM users WHERE email = $1
        "#,
    )
    .bind(email.trim().to_lowercase())
    .fetch_optional(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    let Some(user) = user else {
        return Err(ServerFnError::new("Invalid email or password."));
    };

    let Some(ref hash) = user.password_hash else {
        return Err(ServerFnError::new("Invalid email or password."));
    };

    let parsed_hash = argon2::PasswordHash::new(hash)
        .map_err(|_| ServerFnError::new("Invalid password hash in database."))?;
    let argon2 = Argon2::default();
    argon2
        .verify_password(password.as_bytes(), &parsed_hash)
        .map_err(|_| ServerFnError::new("Invalid email or password."))?;

    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    session
        .insert("user_id", user.id.to_string())
        .await
        .map_err(|e| ServerFnError::new(format!("session insert user_id: {}", e)))?;

    let accepts_html = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);
    if accepts_html {
        leptos_axum::redirect("/dashboard");
    }

    Ok(user.id.to_string())
}

#[server(Logout, "/api")]
pub async fn logout() -> Result<(), ServerFnError> {
    let headers: HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract headers: {}", e)))?;
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    session
        .flush()
        .await
        .map_err(|e| ServerFnError::new(format!("session flush: {}", e)))?;

    let accepts_html = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);
    if accepts_html {
        leptos_axum::redirect("/login");
    }
    Ok(())
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

#[server(GetDashboardSummary, "/api")]
pub async fn get_dashboard_summary() -> Result<DashboardSummary, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let row = sqlx::query(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE status IN ('RECEIVED', 'PROCESSING_VISION', 'VISION_COMPLETE', 'PROCESSING_ACCOUNTANT', 'ACCOUNTANT_REVIEWED', 'PROCESSING_VALIDATOR', 'VALIDATED', 'GENERATING_SIE4', 'REVIEW_COMPLETED')) AS processing,
            COUNT(*) FILTER (WHERE status = 'PENDING_HUMAN_REVIEW') AS pending,
            COUNT(*) FILTER (WHERE status = 'EXPORT_READY') AS ready,
            COUNT(*) FILTER (WHERE status = 'EXPORTED') AS exported,
            COUNT(*) FILTER (WHERE status = 'FAILED') AS failed
        FROM documents
        "#,
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    let processing = row.try_get::<i64, _>("processing").unwrap_or(0);
    let pending = row.try_get::<i64, _>("pending").unwrap_or(0);
    let ready = row.try_get::<i64, _>("ready").unwrap_or(0);
    let exported = row.try_get::<i64, _>("exported").unwrap_or(0);
    let failed = row.try_get::<i64, _>("failed").unwrap_or(0);
    let total = processing + pending + ready + exported + failed;
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

    let recent_transactions = query_documents(&pool, None, None, None, 5, 0).await?;

    let monthly_rows = sqlx::query(
        r#"
        SELECT
            strftime('%Y-%m', received_at) AS month_key,
            strftime('%Y-%m', received_at) AS month_label,
            COUNT(*) AS total_documents,
            COUNT(*) FILTER (WHERE status IN ('EXPORT_READY', 'EXPORTED')) AS completed_documents
        FROM documents
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
    month: Option<String>,
    status_filter: Option<String>,
    search: Option<String>,
    limit: i64,
    offset: i64,
) -> Result<Vec<WebDocumentItem>, ServerFnError> {
    let search = search
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let rows = sqlx::query(
        r#"
        SELECT
            d.id,
            d.short_ref,
            d.status,
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
            ) AS model_used
        FROM documents d
        WHERE ($1 IS NULL OR strftime('%Y-%m', d.received_at) = $1)
            AND ($2 IS NULL OR d.status = $2)
            AND (
                $3 IS NULL
                OR LOWER(d.short_ref) LIKE ('%' || LOWER($3) || '%')
                OR EXISTS (
                    SELECT 1 FROM extracted_fields ef
                    WHERE ef.document_id = d.id
                        AND ef.field_type = 'supplier_name'
                        AND LOWER(ef.parsed_value) LIKE ('%' || LOWER($3) || '%')
                )
            )
        ORDER BY d.received_at DESC
        LIMIT $4 OFFSET $5
        "#,
    )
    .bind(month)
    .bind(status_filter)
    .bind(search)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    Ok(rows
        .into_iter()
        .map(|r| WebDocumentItem {
            id: r.try_get("id").ok(),
            short_ref: r.try_get("short_ref").unwrap_or_default(),
            status: r.try_get("status").unwrap_or_default(),
            supplier_name: r.try_get("supplier_name").ok(),
            invoice_date: r.try_get("invoice_date").ok(),
            total_amount: r.try_get("total_amount").ok(),
            received_date: r.try_get("received_date").ok(),
            ai_confidence: r.try_get("ai_confidence").ok(),
            model_used: r.try_get("model_used").ok(),
        })
        .collect())
}

#[server(CompleteCompanyOnboarding, "/api")]
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

#[server(GetSessionUser, "/api")]
pub async fn get_session_user() -> Result<Option<AuthUser>, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;

    let user_id: Option<String> = session
        .get("user_id")
        .await
        .map_err(|e| ServerFnError::new(format!("session get user_id: {}", e)))?;

    let Some(user_id) = user_id else {
        return Ok(None);
    };

    let user_id = user_id
        .parse::<i64>()
        .map_err(|e| ServerFnError::new(format!("Invalid session: {}", e)))?;

    let user: Option<crate::db::User> = sqlx::query_as(
        r#"
        SELECT id, role, email, password_hash, display_name, created_at, updated_at
        FROM users WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    Ok(user.map(|r| AuthUser {
        id: r.id,
        email: r.email.unwrap_or_default(),
        display_name: r.display_name,
        workspace_id: crate::workspace::active_workspace_id(),
        created_at: r.created_at,
    }))
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WebDocumentItem {
    pub id: Option<i64>,
    pub short_ref: String,
    pub status: String,
    pub supplier_name: Option<String>,
    pub invoice_date: Option<String>,
    pub total_amount: Option<String>,
    pub received_date: Option<String>,
    pub ai_confidence: Option<f64>,
    pub model_used: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct TransactionsResponse {
    pub items: Vec<WebDocumentItem>,
    pub total_documents: i64,
    pub pending_documents: i64,
    pub done_documents: i64,
    pub completion_rate: i64,
}

#[server(GetDocumentList, "/api")]
pub async fn get_document_list(
    month: Option<String>,
    status_filter: Option<String>,
    search: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<TransactionsResponse, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let month_clean = month
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    let status_clean = status_filter
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let limit = limit.unwrap_or(100).clamp(1, 200);
    let offset = offset.unwrap_or(0).max(0);

    let items = query_documents(
        &pool,
        month_clean.clone(),
        status_clean.clone(),
        search,
        limit,
        offset,
    )
    .await?;

    let aggregate = sqlx::query(
        r#"
        SELECT
            COUNT(*) AS total_documents,
            COUNT(*) FILTER (WHERE status = 'PENDING_HUMAN_REVIEW') AS pending_documents,
            COUNT(*) FILTER (WHERE status IN ('EXPORT_READY', 'EXPORTED')) AS done_documents
        FROM documents
        WHERE ($1 IS NULL OR strftime('%Y-%m', received_at) = $1)
            AND ($2 IS NULL OR status = $2)
        "#,
    )
    .bind(month_clean)
    .bind(status_clean)
    .fetch_one(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    let total_documents = aggregate.try_get::<i64, _>("total_documents").unwrap_or(0);
    let pending_documents = aggregate
        .try_get::<i64, _>("pending_documents")
        .unwrap_or(0);
    let done_documents = aggregate.try_get::<i64, _>("done_documents").unwrap_or(0);
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
    pub status: String,
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

#[server(GetDocumentDetails, "/api")]
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

    let row = sqlx::query(
        r#"
        SELECT
            d.id,
            d.short_ref,
            d.status,
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
    .bind(short_ref)
    .fetch_optional(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    let Some(r) = row else {
        return Ok(None);
    };

    let document_id: i64 = r
        .try_get("id")
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
        .bind(document_id)
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
        short_ref: r.try_get("short_ref").unwrap_or_default(),
        status: r.try_get("status").unwrap_or_default(),
        filename: r.try_get("filename").ok(),
        mime_type: r.try_get("mime_type").ok(),
        original_path: r.try_get("original_path").ok(),
        image_url: Some(format!(
            "/api/documents/{}/image",
            r.try_get::<String, _>("short_ref").unwrap_or_default()
        )),
        supplier_name: r.try_get("supplier_name").ok(),
        transaction_date: r.try_get("transaction_date").ok(),
        total_amount: r.try_get("total_amount").ok(),
        vat_amount: r.try_get("vat_amount").ok(),
        subtotal_amount: r.try_get("subtotal_amount").ok(),
        net_amount: r.try_get("net_amount").ok(),
        assigned_account_code: r.try_get("assigned_account_code").ok(),
        account_name: r.try_get("account_name").ok(),
        ai_confidence: r.try_get("ai_confidence").ok(),
        model_used: r.try_get("model_used").ok(),
        document_type: r.try_get("document_type").ok(),
        review_confidence: r.try_get("review_confidence").ok(),
        review_decision_type: r.try_get("review_decision_type").ok(),
        review_reason: r.try_get("review_reason").ok(),
        validation_errors: r.try_get("validation_errors").ok(),
        accounting_rows,
    }))
}
