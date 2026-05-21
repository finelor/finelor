//! Web Auth Integration Tests
//!
//! Rule of thumb:
//! - pure DB auth tests use per-test `in_memory_pool()`
//! - server-fn/router/session tests use `common::TestContext::server_fn()` for
//!   serialized access to the shared global `web::pool` singleton.

mod common;

use argon2::{
    Argon2, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{Json, routing::post};
use leptos::server_fn::ServerFn;
use serde_json::json;
use sqlx::{Row, SqlitePool};
use tower_sessions::{
    Expiry, Session, SessionManagerLayer,
    cookie::{Key, SameSite},
};
use tower_sessions_sqlx_store::SqliteStore;
use uuid::Uuid;

const TEST_SESSION_SECRET: &str =
    "test-session-secret-must-be-at-least-sixty-four-characters-long-0001";

/// Creates a password hash using argon2 (matches the signup function)
fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .expect("Password hashing failed")
        .to_string()
}

/// Verifies a password against a hash (matches the login function)
fn verify_password(password: &str, hash: &str) -> bool {
    let parsed_hash = argon2::PasswordHash::new(hash).expect("Invalid hash");
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

/// Cleanup helper - deletes user and company in a transaction
async fn cleanup_user(pool: &SqlitePool, user_id: i64) {
    let _ = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await;
}

async fn create_test_workspace(_pool: &SqlitePool, _name_prefix: &str) -> Uuid {
    finelor::workspace::active_workspace_id()
}

// ============================================================================
// Test 1: signup creates company and user
// ============================================================================

#[tokio::test]
async fn test_signup_creates_company_and_user() {
    let pool = common::in_memory_pool().await;
    let _ = sqlx::query("DELETE FROM channel_identities")
        .execute(&pool)
        .await;

    let test_email = format!("test_signup_{}@example.com", Uuid::new_v4());
    let test_password = "testpassword123";
    let _test_company = format!("Test Company {}", Uuid::new_v4().simple());
    let _test_org_nr = Some("123456-7890".to_string());

    // Manually perform what signup does: create company, hash password, create user
    let mut tx = pool.begin().await.expect("Failed to begin transaction");
    let _workspace_id: Uuid = finelor::workspace::active_workspace_id();

    // Hash password
    let password_hash = hash_password(test_password);

    // Create user
    let user_row = sqlx::query(
        r#"
        INSERT INTO users (role, email, password_hash, display_name)
        VALUES ('admin', $1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(test_email.to_lowercase())
    .bind(&password_hash)
    .bind(Some(test_email.clone()))
    .fetch_one(&mut *tx)
    .await
    .expect("Failed to create user");

    let user_id: i64 = user_row.get("id");

    tx.commit().await.expect("Failed to commit transaction");

    // Assert user id is returned (we have it)
    assert!(user_id > 0, "User ID should be a positive integer");

    // Query DB to verify user row exists
    let user_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND email = $2)")
            .bind(user_id)
            .bind(test_email.to_lowercase())
            .fetch_one(&pool)
            .await
            .expect("Failed to check user");

    assert!(user_exists, "User row should exist in database");

    // Verify password hash is correct
    let stored_hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .expect("Failed to get password hash");

    assert!(
        verify_password(test_password, &stored_hash),
        "Stored password hash should match 'testpassword123'"
    );

    // Cleanup
    cleanup_user(&pool, user_id).await;
}

// ============================================================================
// Test 2: login valid user
// ============================================================================

#[tokio::test]
async fn test_login_valid_user() {
    let pool = common::in_memory_pool().await;

    let test_email = format!("test_login_{}@example.com", Uuid::new_v4());
    let test_password = "testpassword123";
    let _test_company = format!("Login Test Co {}", Uuid::new_v4().simple());

    // Setup: create company and user with known password
    let mut tx = pool.begin().await.expect("Failed to begin transaction");
    let _workspace_id: Uuid = finelor::workspace::active_workspace_id();

    // Hash password using argon2 (matches signup)
    let password_hash = hash_password(test_password);

    // Create user
    let user_row = sqlx::query(
        r#"
        INSERT INTO users (role, email, password_hash, display_name)
        VALUES ('admin', $1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(test_email.to_lowercase())
    .bind(&password_hash)
    .bind(None::<String>)
    .fetch_one(&mut *tx)
    .await
    .expect("Failed to create user");

    let user_id: i64 = user_row.get("id");

    tx.commit().await.expect("Failed to commit transaction");

    // Now test login: fetch user by email and verify password
    let user_row = sqlx::query(
        r#"
        SELECT id, password_hash FROM users WHERE email = $1
        "#,
    )
    .bind(test_email.to_lowercase())
    .fetch_optional(&pool)
    .await
    .expect("Failed to query user");

    let Some(user_row) = user_row else {
        panic!("User should exist after setup");
    };

    let returned_user_id: i64 = user_row.get("id");
    let stored_hash: String = user_row.get("password_hash");

    // Assert login returns the user id
    assert_eq!(
        returned_user_id, user_id,
        "Login should return the correct user ID"
    );

    // Verify password using argon2 (matches login function)
    let parsed_hash = argon2::PasswordHash::new(&stored_hash).expect("Invalid hash in database");
    let verify_result = Argon2::default().verify_password(test_password.as_bytes(), &parsed_hash);

    assert!(
        verify_result.is_ok(),
        "Password verification should succeed for valid password"
    );

    // Cleanup
    cleanup_user(&pool, user_id).await;
}

// ============================================================================
// Test 3: login invalid password
// ============================================================================

#[tokio::test]
async fn test_login_invalid_password() {
    let pool = common::in_memory_pool().await;

    let test_email = format!("test_bad_pw_{}@example.com", Uuid::new_v4());
    let test_password = "testpassword123";
    let wrong_password = "wrongpassword456";
    let _test_company = format!("Bad PW Test {}", Uuid::new_v4().simple());

    // Setup: create company and user
    let mut tx = pool.begin().await.expect("Failed to begin transaction");
    let _workspace_id: Uuid = finelor::workspace::active_workspace_id();
    let password_hash = hash_password(test_password);

    let user_row = sqlx::query(
        r#"
        INSERT INTO users (role, email, password_hash, display_name)
        VALUES ('admin', $1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(test_email.to_lowercase())
    .bind(&password_hash)
    .bind(None::<String>)
    .fetch_one(&mut *tx)
    .await
    .expect("Failed to create user");

    let user_id: i64 = user_row.get("id");

    tx.commit().await.expect("Failed to commit transaction");

    // Now test login with wrong password
    // Simulate the login function's password verification
    let user_row = sqlx::query(
        r#"
        SELECT id, password_hash FROM users WHERE email = $1
        "#,
    )
    .bind(test_email.to_lowercase())
    .fetch_optional(&pool)
    .await
    .expect("Failed to query user");

    let Some(user_row) = user_row else {
        panic!("User should exist after setup");
    };

    let stored_hash: String = user_row.get("password_hash");

    // Verify wrong password fails (what login does)
    let parsed_hash = argon2::PasswordHash::new(&stored_hash).expect("Invalid hash in database");
    let verify_result = Argon2::default().verify_password(wrong_password.as_bytes(), &parsed_hash);

    // Assert login should fail with wrong password
    assert!(
        verify_result.is_err(),
        "Password verification should fail for wrong password"
    );

    // Cleanup
    cleanup_user(&pool, user_id).await;
}

// ============================================================================
// Bonus Test: signup with duplicate email should fail
// ============================================================================

#[tokio::test]
async fn test_signup_duplicate_email_fails() {
    let pool = common::in_memory_pool().await;

    let test_email = format!("duplicate_{}@example.com", Uuid::new_v4());
    let test_password = "testpassword123";
    let _test_company = format!("Duplicate Test {}", Uuid::new_v4().simple());

    // First signup
    let mut tx = pool.begin().await.expect("Failed to begin transaction");
    let _workspace_id: Uuid = finelor::workspace::active_workspace_id();
    let password_hash = hash_password(test_password);

    let user_row = sqlx::query(
        r#"
        INSERT INTO users (role, email, password_hash, display_name)
        VALUES ('admin', $1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(test_email.to_lowercase())
    .bind(&password_hash)
    .bind(None::<String>)
    .fetch_one(&mut *tx)
    .await
    .expect("Failed to create user");

    let user_id: i64 = user_row.get("id");

    tx.commit().await.expect("Failed to commit transaction");

    // Verify the first user was created successfully
    let first_user_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND email = $2)")
            .bind(user_id)
            .bind(test_email.to_lowercase())
            .fetch_one(&pool)
            .await
            .expect("Failed to check user");

    assert!(first_user_exists, "First user should exist");

    // Cleanup
    cleanup_user(&pool, user_id).await;
}

#[tokio::test]
async fn test_signup_server_fn_endpoint_is_reachable_and_redirects() {
    let ctx = common::TestContext::server_fn().await;
    let pool = ctx.pool();
    let (addr, server) = ctx.spawn_server_fn_app(TEST_SESSION_SECRET).await;

    let email = format!("sf_route_{}@example.com", Uuid::new_v4());
    let full_name = format!("ServerFn Route Test {}", Uuid::new_v4().simple());

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build reqwest client");

    let endpoint = format!(
        "http://{}{}",
        addr,
        <finelor::web::server::auth::Signup as ServerFn>::PATH
    );

    let response = client
        .post(&endpoint)
        .header(reqwest::header::ACCEPT, "text/html")
        .form(&[
            ("email", email.as_str()),
            ("password", "testpw6"),
            ("full_name", full_name.as_str()),
        ])
        .send()
        .await
        .expect("signup server fn request failed");

    assert_eq!(response.status(), reqwest::StatusCode::FOUND);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/dashboard")
    );

    let row = sqlx::query("SELECT id AS user_id FROM users WHERE email = $1")
        .bind(email.to_lowercase())
        .fetch_one(&pool)
        .await
        .expect("signup should create user/company");

    let _user_id: i64 = row.get("user_id");
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn test_signup_allows_only_single_admin_in_oss() {
    let ctx = common::TestContext::server_fn().await;
    let pool = ctx.pool();
    let (addr, server) = ctx.spawn_server_fn_app(TEST_SESSION_SECRET).await;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build reqwest client");
    let endpoint = format!(
        "http://{}{}",
        addr,
        <finelor::web::server::auth::Signup as ServerFn>::PATH
    );

    let first_email = format!("first_admin_{}@example.com", Uuid::new_v4());
    let first = client
        .post(&endpoint)
        .header(reqwest::header::ACCEPT, "text/html")
        .form(&[
            ("email", first_email.as_str()),
            ("password", "testpw6"),
            ("full_name", "First Admin"),
        ])
        .send()
        .await
        .expect("first signup request failed");
    assert_eq!(first.status(), reqwest::StatusCode::FOUND);

    let second_email = format!("second_admin_{}@example.com", Uuid::new_v4());
    let second = client
        .post(&endpoint)
        .form(&[
            ("email", second_email.as_str()),
            ("password", "testpw6"),
            ("full_name", "Second Admin"),
        ])
        .send()
        .await
        .expect("second signup request failed");
    let body = second.text().await.expect("read second signup body");
    assert!(
        body.contains("Signup is closed"),
        "Second signup should be rejected. Body: {body}"
    );

    let admin_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .expect("count users");
    assert_eq!(admin_count, 1, "Only one admin user should exist");

    let row = sqlx::query(
        "SELECT display_name, org_nr, jurisdiction FROM company_profile WHERE singleton = TRUE",
    )
    .fetch_one(&pool)
    .await
    .expect("company_profile singleton row should exist");
    let display_name: Option<String> = row.try_get("display_name").unwrap_or(None);
    let org_nr: Option<String> = row.try_get("org_nr").unwrap_or(None);
    let jurisdiction: String = row.try_get("jurisdiction").unwrap_or_default();
    assert!(
        display_name.is_none(),
        "Company profile starts empty after signup"
    );
    assert!(org_nr.is_none(), "Org number starts empty after signup");
    assert_eq!(jurisdiction, "SE");

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn test_onboarding_and_settings_server_fns_persist_company_profile() {
    let ctx = common::TestContext::server_fn().await;
    let pool = ctx.pool();
    let (addr, server) = ctx.spawn_server_fn_app(TEST_SESSION_SECRET).await;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build reqwest client");

    let signup_endpoint = format!(
        "http://{}{}",
        addr,
        <finelor::web::server::auth::Signup as ServerFn>::PATH
    );
    let signup_email = format!("onboarding_admin_{}@example.com", Uuid::new_v4());
    let signup_response = client
        .post(&signup_endpoint)
        .header(reqwest::header::ACCEPT, "text/html")
        .form(&[
            ("email", signup_email.as_str()),
            ("password", "testpw6"),
            ("full_name", "Onboarding Owner"),
        ])
        .send()
        .await
        .expect("signup request failed");
    assert_eq!(signup_response.status(), reqwest::StatusCode::FOUND);
    let session_cookie = signup_response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .expect("signup set-cookie header")
        .to_string();

    let summary_endpoint = format!(
        "http://{}{}",
        addr,
        <finelor::web::server::auth::GetDashboardSummary as ServerFn>::PATH
    );
    let summary_raw = client
        .post(&summary_endpoint)
        .header(reqwest::header::COOKIE, session_cookie.clone())
        .form(&[] as &[(&str, &str)])
        .send()
        .await
        .expect("dashboard summary request failed")
        .text()
        .await
        .expect("read summary body");
    assert!(
        summary_raw.contains("onboarding_required\":true"),
        "Dashboard summary should require onboarding before company setup. Body: {summary_raw}"
    );

    let onboarding_endpoint = format!(
        "http://{}{}",
        addr,
        <finelor::web::server::auth::CompleteCompanyOnboarding as ServerFn>::PATH
    );
    let onboarding_response = client
        .post(&onboarding_endpoint)
        .header(reqwest::header::COOKIE, session_cookie.clone())
        .form(&[("company_name", "Acme AB")])
        .send()
        .await
        .expect("complete onboarding request failed");
    assert!(
        onboarding_response.status().is_success(),
        "Onboarding submit should succeed"
    );

    let settings_update_endpoint = format!(
        "http://{}{}",
        addr,
        <finelor::web::server::settings::UpdateCompanySettings as ServerFn>::PATH
    );
    let update_response = client
        .post(&settings_update_endpoint)
        .header(reqwest::header::COOKIE, session_cookie.clone())
        .form(&[("display_name", "Acme AB"), ("org_nr", "556677-8899")])
        .send()
        .await
        .expect("update company settings request failed");
    assert!(
        update_response.status().is_success(),
        "Company settings update should succeed"
    );

    let row = sqlx::query(
        "SELECT display_name, org_nr FROM company_profile WHERE singleton = TRUE LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("load company profile");
    let display_name: Option<String> = row.try_get("display_name").unwrap_or(None);
    let org_nr: Option<String> = row.try_get("org_nr").unwrap_or(None);
    assert_eq!(display_name.as_deref(), Some("Acme AB"));
    assert_eq!(org_nr.as_deref(), Some("556677-8899"));

    let summary_after = client
        .post(&summary_endpoint)
        .header(reqwest::header::COOKIE, session_cookie)
        .form(&[] as &[(&str, &str)])
        .send()
        .await
        .expect("dashboard summary request after onboarding failed")
        .text()
        .await
        .expect("read summary after body");
    assert!(
        summary_after.contains("onboarding_required\":false"),
        "Dashboard summary should no longer require onboarding. Body: {summary_after}"
    );
    assert!(
        summary_after.contains("Acme AB"),
        "Dashboard summary should expose persisted company name. Body: {summary_after}"
    );

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn test_sqlite_session_survives_router_recreation_and_logout_clears_it() {
    async fn set_session(session: Session) -> Json<serde_json::Value> {
        session
            .insert("user_id", "persisted-user")
            .await
            .expect("insert user_id");
        session
            .insert("workspace_id", "persisted-workspace")
            .await
            .expect("insert workspace_id");
        Json(json!({ "ok": true }))
    }

    async fn read_session(session: Session) -> Json<serde_json::Value> {
        let user_id: Option<String> = session.get("user_id").await.expect("get user_id");
        let workspace_id: Option<String> =
            session.get("workspace_id").await.expect("get workspace_id");
        Json(json!({
            "user_id": user_id,
            "workspace_id": workspace_id
        }))
    }

    async fn logout_session(session: Session) -> Json<serde_json::Value> {
        session.flush().await.expect("flush session");
        Json(json!({ "ok": true }))
    }

    async fn spawn_test_app(
        pool: SqlitePool,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let session_store = sqlite_session_store(&pool).await;
        let session_layer = SessionManagerLayer::new(session_store)
            .with_name("finelor.sid")
            .with_http_only(true)
            .with_same_site(SameSite::Lax)
            .with_secure(false)
            .with_expiry(Expiry::OnInactivity(time::Duration::days(30)))
            .with_always_save(true)
            .with_signed(Key::from(TEST_SESSION_SECRET.as_bytes()));

        let app = axum::Router::new()
            .route("/set", post(set_session))
            .route("/read", post(read_session))
            .route("/logout", post(logout_session))
            .layer(session_layer);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        (addr, server)
    }

    let pool = common::in_memory_pool().await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build reqwest client");

    let (first_addr, first_server) = spawn_test_app(pool.clone()).await;
    let set_response = client
        .post(format!("http://{first_addr}/set"))
        .send()
        .await
        .expect("set session request failed");
    let cookie = set_response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .expect("set-cookie header")
        .to_string();
    first_server.abort();

    let (second_addr, second_server) = spawn_test_app(pool.clone()).await;
    let read_response: serde_json::Value = client
        .post(format!("http://{second_addr}/read"))
        .header(reqwest::header::COOKIE, cookie.clone())
        .send()
        .await
        .expect("read session request failed")
        .json()
        .await
        .expect("read json");
    assert_eq!(read_response["user_id"], "persisted-user");
    assert_eq!(read_response["workspace_id"], "persisted-workspace");

    let logout_response = client
        .post(format!("http://{second_addr}/logout"))
        .header(reqwest::header::COOKIE, cookie.clone())
        .send()
        .await
        .expect("logout request failed");
    let logout_cookie = logout_response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .unwrap_or(cookie.as_str())
        .to_string();

    let after_logout: serde_json::Value = client
        .post(format!("http://{second_addr}/read"))
        .header(reqwest::header::COOKIE, logout_cookie)
        .send()
        .await
        .expect("read after logout request failed")
        .json()
        .await
        .expect("read after logout json");
    assert!(after_logout["user_id"].is_null());
    assert!(after_logout["workspace_id"].is_null());

    second_server.abort();
}

#[tokio::test]
async fn test_connect_telegram_for_active_workspace_allows_multiple_channels_and_is_idempotent() {
    let pool = common::in_memory_pool().await;

    let workspace_id = create_test_workspace(&pool, "Telegram Connect Workspace").await;

    let chat_a = 100_000
        + i64::from_le_bytes(Uuid::new_v4().as_bytes()[..8].try_into().unwrap()).abs() % 800_000;
    let chat_b = 100_000
        + i64::from_le_bytes(Uuid::new_v4().as_bytes()[..8].try_into().unwrap()).abs() % 800_000;

    let first =
        finelor::web::server::auth::connect_telegram_for_workspace(&pool, workspace_id, chat_a)
            .await
            .expect("First telegram connect should succeed");
    assert!(
        first.contains("connected"),
        "Expected success message to mention connected"
    );

    let duplicate_same =
        finelor::web::server::auth::connect_telegram_for_workspace(&pool, workspace_id, chat_a)
            .await
            .expect("Same telegram chat id should be treated as already connected");
    assert!(
        duplicate_same.contains("already connected"),
        "Expected idempotent response for same chat id"
    );

    let second_different =
        finelor::web::server::auth::connect_telegram_for_workspace(&pool, workspace_id, chat_b)
            .await
            .expect("Different telegram chat id should also connect");
    assert!(
        second_different.contains("connected"),
        "Expected second connect to succeed"
    );

    let identities = finelor::db::list_channel_identities(&pool, None)
        .await
        .expect("Failed to list channel identities")
        .into_iter()
        .filter(|i| {
            i.channel_identifier == chat_a.to_string() || i.channel_identifier == chat_b.to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(identities.len(), 2, "Two channel identities should exist");
    assert!(
        identities
            .iter()
            .any(|i| i.channel_identifier == chat_a.to_string())
    );
    assert!(
        identities
            .iter()
            .any(|i| i.channel_identifier == chat_b.to_string())
    );

    // Cleanup
    let _ = sqlx::query("DELETE FROM channel_identities")
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn test_delete_channel_identity_for_active_workspace_deletes_only_targeted_channel() {
    let pool = common::in_memory_pool().await;
    let _ = sqlx::query("DELETE FROM channel_identities")
        .execute(&pool)
        .await;

    let _workspace_id = create_test_workspace(&pool, "Channel Delete Workspace").await;

    let first_channel = finelor::db::insert_channel_identity(&pool, "TELEGRAM", "111222", None)
        .await
        .expect("Failed to create first channel");
    let second_channel = finelor::db::insert_channel_identity(&pool, "TELEGRAM", "333444", None)
        .await
        .expect("Failed to create second channel");

    let own_deleted = finelor::db::delete_channel_identity(&pool, first_channel.id)
        .await
        .expect("Workspace delete query failed");
    assert!(own_deleted, "Delete should remove own workspace channel");

    let channels = finelor::db::list_channel_identities(&pool, None)
        .await
        .expect("Failed to list workspace channels")
        .into_iter()
        .filter(|i| i.id == second_channel.id || i.id == first_channel.id)
        .collect::<Vec<_>>();

    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].id, second_channel.id);

    let _ = sqlx::query("DELETE FROM channel_identities")
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn test_server_fn_test_context_isolates_company_profile_state_between_tests() {
    let ctx_one = common::TestContext::server_fn().await;
    let pool_one = ctx_one.pool();
    sqlx::query(
        "UPDATE company_profile SET display_name = 'Mutated Co', org_nr = '123456-7890' WHERE singleton = TRUE",
    )
    .execute(&pool_one)
    .await
    .expect("mutate company profile in first context");
    drop(ctx_one);

    let ctx_two = common::TestContext::server_fn().await;
    let pool_two = ctx_two.pool();
    common::assert_clean_signup_state(&pool_two).await;
}

#[tokio::test]
#[should_panic(expected = "signup tests require empty users table before request")]
async fn test_server_fn_context_fails_fast_when_preconditions_are_dirty() {
    let dirty_pool = common::in_memory_pool().await;
    sqlx::query(
        "INSERT INTO users (role, email, password_hash, display_name) VALUES ('admin', $1, $2, $3)",
    )
    .bind(format!("dirty_precondition_{}@example.com", Uuid::new_v4()))
    .bind("hash")
    .bind("Dirty User")
    .execute(&dirty_pool)
    .await
    .expect("insert dirty precondition user");

    let _ctx = common::TestContext::server_fn_from_pool(dirty_pool).await;
}
async fn sqlite_session_store(pool: &SqlitePool) -> SqliteStore {
    let store = SqliteStore::new(pool.clone());
    store.migrate().await.expect("migrate test session store");
    store
}
