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
use tower_sessions::Session;
use uuid::Uuid;

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

#[server(Signup, "/_server_fn")]
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

#[server(Login, "/_server_fn")]
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

#[server(Logout, "/_server_fn")]
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

#[server(GetSessionUser, "/_server_fn")]
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
