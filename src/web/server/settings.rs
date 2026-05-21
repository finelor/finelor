#[cfg(feature = "ssr")]
use crate::db::DbPool;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(feature = "ssr")]
use sqlx::Row;
#[cfg(feature = "ssr")]
use tower_sessions::Session;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct CompanySettings {
    pub display_name: String,
    pub org_nr: Option<String>,
    pub jurisdiction: String,
}

#[cfg(feature = "ssr")]
fn pool() -> DbPool {
    crate::web::pool::get_pool()
}

#[cfg(feature = "ssr")]
async fn require_session_workspace_id(session: &Session) -> Result<uuid::Uuid, ServerFnError> {
    let user_id: Option<String> = session
        .get("user_id")
        .await
        .map_err(|e| ServerFnError::new(format!("session get user_id: {}", e)))?;
    if user_id.is_none() {
        return Err(ServerFnError::new("Not authenticated."));
    }
    Ok(crate::workspace::active_workspace_id())
}

#[server(GetCompanySettings, "/api")]
pub async fn get_company_settings() -> Result<CompanySettings, ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let row = sqlx::query(
        r#"
        SELECT display_name, org_nr, jurisdiction
        FROM company_profile
        WHERE singleton = TRUE
        LIMIT 1
        "#,
    )
    .fetch_optional(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    let row = match row {
        Some(row) => row,
        None => {
            sqlx::query(
                r#"
                INSERT INTO company_profile (singleton)
                VALUES (TRUE)
                ON CONFLICT (singleton) DO NOTHING
                "#,
            )
            .execute(&pool)
            .await
            .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;
            sqlx::query(
                r#"
                SELECT display_name, org_nr, jurisdiction
                FROM company_profile
                WHERE singleton = TRUE
                LIMIT 1
                "#,
            )
            .fetch_one(&pool)
            .await
            .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?
        }
    };

    let display_name: String = row.try_get("display_name").unwrap_or_default();
    let org_nr: Option<String> = row.try_get::<Option<String>, _>("org_nr").unwrap_or(None);
    let jurisdiction: String = row
        .try_get("jurisdiction")
        .unwrap_or_else(|_| "SE".to_string());

    Ok(CompanySettings {
        display_name,
        org_nr,
        jurisdiction,
    })
}

#[server(UpdateCompanySettings, "/api")]
pub async fn update_company_settings(
    display_name: String,
    org_nr: Option<String>,
) -> Result<(), ServerFnError> {
    let pool = pool();
    let session: Session = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("extract session: {}", e)))?;
    require_session_workspace_id(&session).await?;

    let display_name = display_name.trim();
    if display_name.is_empty() {
        return Err(ServerFnError::new("Company name is required."));
    }

    let trimmed_org = org_nr
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    sqlx::query(
        r#"
        UPDATE company_profile
        SET display_name = $1, org_nr = $2, updated_at = CURRENT_TIMESTAMP
        WHERE singleton = TRUE
        "#,
    )
    .bind(display_name)
    .bind(trimmed_org)
    .execute(&pool)
    .await
    .map_err(|e| ServerFnError::new(format!("Database error: {}", e)))?;

    Ok(())
}
