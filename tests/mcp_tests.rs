mod common;

use std::sync::Arc;

use axum::Router;
use tower::util::ServiceExt;

async fn create_test_user(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (role, email, password_hash, display_name)
        VALUES ('admin', $1, 'hash', 'MCP Tester')
        RETURNING id
        "#,
    )
    .bind(format!("mcp-test-{}@example.com", uuid::Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .expect("insert test user")
}

async fn create_test_mcp_key(pool: &sqlx::SqlitePool) -> String {
    let user_id = create_test_user(pool).await;
    let token = finelor::db::create_mcp_key(pool, "Test MCP key", user_id)
        .await
        .expect("create mcp key")
        .token;
    assert!(token.starts_with("finelor_mcp_"));
    token
}

async fn create_test_api_key(pool: &sqlx::SqlitePool) -> String {
    let user_id = create_test_user(pool).await;
    let token = finelor::db::create_api_key(pool, "Test API key", user_id)
        .await
        .expect("create api key")
        .token;
    assert!(token.starts_with("finelor_api_"));
    token
}

fn mcp_app(pool: sqlx::SqlitePool) -> Router {
    finelor::mcp::router(pool, common::test_config().web)
}

fn mcp_app_with_hosts(pool: sqlx::SqlitePool, allowed_hosts: Vec<String>) -> Router {
    finelor::mcp::router(
        pool,
        finelor::config::WebConfig {
            allowed_hosts,
            allowed_origins: vec![],
        },
    )
}

fn public_api_app(pool: sqlx::SqlitePool) -> Router {
    let config = Arc::new(common::test_config());
    let queue = finelor::queue::InMemoryJobQueue::default();
    let queue_producer = finelor::queue::QueueProducer::new(Arc::new(queue));
    let state = finelor::api::PublicApiState {
        config,
        pool,
        queue_producer,
        events: finelor::web::events::AppEventBus::new(16),
    };

    finelor::api::router::<finelor::api::PublicApiState>().with_state(state)
}

async fn post_mcp(
    app: Router,
    token: Option<&str>,
    body: serde_json::Value,
    origin: Option<&str>,
) -> axum::response::Response {
    post_mcp_with_host(app, token, body, "localhost", origin).await
}

async fn post_mcp_with_host(
    app: Router,
    token: Option<&str>,
    body: serde_json::Value,
    host: &str,
    origin: Option<&str>,
) -> axum::response::Response {
    let mut builder = axum::http::Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", host)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    if let Some(origin) = origin {
        builder = builder.header("Origin", origin);
    }

    app.oneshot(
        builder
            .body(axum::body::Body::from(body.to_string()))
            .unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn mcp_rejects_missing_key() {
    let pool = common::in_memory_pool().await;
    let response = post_mcp(
        mcp_app(pool),
        None,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }),
        None,
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_rejects_public_api_key() {
    let pool = common::in_memory_pool().await;
    let token = create_test_api_key(&pool).await;
    let response = post_mcp(
        mcp_app(pool),
        Some(&token),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }),
        None,
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn public_api_rejects_mcp_key() {
    let pool = common::in_memory_pool().await;
    let token = create_test_mcp_key(&pool).await;
    let response = public_api_app(pool)
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/documents")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_lists_only_initial_tools() {
    let pool = common::in_memory_pool().await;
    let token = create_test_mcp_key(&pool).await;
    let response = post_mcp(
        mcp_app(pool),
        Some(&token),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }),
        None,
    )
    .await;

    assert!(
        response.status().is_success(),
        "status {}",
        response.status()
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let names: Vec<&str> = value["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();

    assert_eq!(names.len(), 4);
    assert!(names.contains(&"document_status_summary"));
    assert!(names.contains(&"list_documents"));
    assert!(names.contains(&"get_document"));
    assert!(names.contains(&"explain_document"));
    assert!(!names.contains(&"list_pending_reviews"));
    assert!(!names.contains(&"list_export_ready"));
}

#[tokio::test]
async fn mcp_tool_call_updates_last_used() {
    let pool = common::in_memory_pool().await;
    let token = create_test_mcp_key(&pool).await;
    let response = post_mcp(
        mcp_app(pool.clone()),
        Some(&token),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "document_status_summary",
                "arguments": {}
            }
        }),
        None,
    )
    .await;

    assert!(
        response.status().is_success(),
        "status {}",
        response.status()
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["result"]["structuredContent"]["ok"], true);

    let key = finelor::db::list_visible_mcp_keys(&pool)
        .await
        .expect("list mcp keys")
        .pop()
        .expect("created mcp key");
    assert!(key.last_used_at.is_some());
}

#[tokio::test]
async fn mcp_capability_checks_fail_closed() {
    let pool = common::in_memory_pool().await;
    let token = create_test_mcp_key(&pool).await;
    let key = finelor::db::list_visible_mcp_keys(&pool)
        .await
        .expect("list mcp keys")
        .pop()
        .expect("created mcp key");
    sqlx::query("UPDATE mcp_keys SET capabilities = $1 WHERE id = $2")
        .bind(r#"["documents:read"]"#)
        .bind(key.id)
        .execute(&pool)
        .await
        .expect("update capabilities");

    let response = post_mcp(
        mcp_app(pool),
        Some(&token),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "explain_document",
                "arguments": { "short_ref": "D000001" }
            }
        }),
        None,
    )
    .await;

    assert!(
        response.status().is_success(),
        "status {}",
        response.status()
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("documents:explain")
    );
}

#[tokio::test]
async fn revoked_mcp_key_can_be_unrevoked() {
    let pool = common::in_memory_pool().await;
    let token = create_test_mcp_key(&pool).await;
    let key = finelor::db::list_visible_mcp_keys(&pool)
        .await
        .expect("list mcp keys")
        .pop()
        .expect("created mcp key");
    finelor::db::revoke_mcp_key(&pool, key.id)
        .await
        .expect("revoke mcp key");
    let revoked_response = post_mcp(
        mcp_app(pool.clone()),
        Some(&token),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }),
        None,
    )
    .await;
    assert_eq!(
        revoked_response.status(),
        axum::http::StatusCode::UNAUTHORIZED
    );

    finelor::db::unrevoke_mcp_key(&pool, key.id)
        .await
        .expect("unrevoke mcp key");
    let restored_response = post_mcp(
        mcp_app(pool),
        Some(&token),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        None,
    )
    .await;
    assert!(restored_response.status().is_success());
}

#[tokio::test]
async fn mcp_allows_arbitrary_origin_with_valid_host_and_key() {
    let pool = common::in_memory_pool().await;
    let token = create_test_mcp_key(&pool).await;
    let response = post_mcp(
        mcp_app(pool),
        Some(&token),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }),
        Some("https://evil.example"),
    )
    .await;

    assert!(response.status().is_success());
}

#[tokio::test]
async fn mcp_accepts_configured_public_host() {
    let pool = common::in_memory_pool().await;
    let token = create_test_mcp_key(&pool).await;
    let response = post_mcp_with_host(
        mcp_app_with_hosts(pool, vec!["finelor.example".to_string()]),
        Some(&token),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }),
        "finelor.example",
        None,
    )
    .await;

    assert!(response.status().is_success());
}

#[tokio::test]
async fn mcp_rejects_unconfigured_public_host() {
    let pool = common::in_memory_pool().await;
    let token = create_test_mcp_key(&pool).await;
    let response = post_mcp_with_host(
        mcp_app_with_hosts(pool, vec!["finelor.example".to_string()]),
        Some(&token),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }),
        "other.example",
        None,
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn removed_mcp_key_is_hidden_and_cannot_authenticate() {
    let pool = common::in_memory_pool().await;
    let token = create_test_mcp_key(&pool).await;
    let key = finelor::db::list_visible_mcp_keys(&pool)
        .await
        .expect("list mcp keys")
        .pop()
        .expect("created mcp key");
    finelor::db::hide_mcp_key(&pool, key.id)
        .await
        .expect("hide mcp key");
    assert!(
        finelor::db::reveal_mcp_key_token(&pool, key.id)
            .await
            .expect("reveal hidden mcp key")
            .is_none()
    );

    let response = post_mcp(
        mcp_app(pool),
        Some(&token),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }),
        None,
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
}
