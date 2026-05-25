mod common;

use std::sync::Arc;

use axum::Router;
use tower::util::ServiceExt;

async fn create_test_api_key(pool: &sqlx::SqlitePool) -> String {
    let user_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO users (role, email, password_hash, display_name)
        VALUES ('admin', $1, 'hash', 'API Tester')
        RETURNING id
        "#,
    )
    .bind(format!("api-test-{}@example.com", uuid::Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .expect("insert test user");

    finelor::db::create_api_key(pool, "Test key", user_id)
        .await
        .expect("create api key")
        .token
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

#[tokio::test]
async fn public_api_rejects_missing_api_key() {
    let pool = common::in_memory_pool().await;
    let app = public_api_app(pool);

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/documents")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn public_api_ingests_lists_details_and_downloads_with_valid_key() {
    let pool = common::in_memory_pool().await;
    let token = create_test_api_key(&pool).await;
    let app = public_api_app(pool.clone());

    let boundary = "----FinelorBoundary";
    let body = concat!(
        "------FinelorBoundary\r\n",
        "Content-Disposition: form-data; name=\"file\"; filename=\"invoice.pdf\"\r\n",
        "Content-Type: application/pdf\r\n\r\n",
        "%PDF-1.4 public api test\r\n",
        "------FinelorBoundary--\r\n"
    );

    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/documents")
                .header("Authorization", format!("Bearer {token}"))
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(axum::body::Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "upload failed: {}",
        response.status()
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let short_ref = created["short_ref"].as_str().unwrap().to_string();

    let list_response = app
        .clone()
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
    assert!(list_response.status().is_success());
    let list_body = axum::body::to_bytes(list_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: serde_json::Value = serde_json::from_slice(&list_body).unwrap();
    assert_eq!(list["total"].as_i64(), Some(1));
    assert_eq!(
        list["items"][0]["short_ref"].as_str(),
        Some(short_ref.as_str())
    );
    assert_eq!(list["items"][0]["has_download"].as_bool(), Some(true));
    let expected_download_url = format!("/api/v1/documents/{short_ref}/file");
    assert_eq!(
        list["items"][0]["download_url"].as_str(),
        Some(expected_download_url.as_str())
    );

    let detail_response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri(format!("/documents/{short_ref}"))
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(detail_response.status().is_success());
    let detail_body = axum::body::to_bytes(detail_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let detail: serde_json::Value = serde_json::from_slice(&detail_body).unwrap();
    assert_eq!(
        detail["download_url"].as_str(),
        Some(expected_download_url.as_str())
    );
    assert!(
        detail.get("original_path").is_none(),
        "public detail must not expose original_path"
    );

    let file_response = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri(format!("/documents/{short_ref}/file"))
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(file_response.status().is_success());
    assert_eq!(
        file_response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/pdf")
    );
    let file_body = axum::body::to_bytes(file_response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&file_body).contains("%PDF-1.4"));

    let keys = finelor::db::list_visible_api_keys(&pool)
        .await
        .expect("list keys");
    assert!(
        keys[0].last_used_at.is_some(),
        "API auth should update last_used_at"
    );
    assert_eq!(
        finelor::db::reveal_api_key_token(&pool, keys[0].id)
            .await
            .expect("reveal key token")
            .as_deref(),
        Some(token.as_str())
    );
}

#[tokio::test]
async fn public_api_rejects_revoked_api_key() {
    let pool = common::in_memory_pool().await;
    let token = create_test_api_key(&pool).await;
    let key = finelor::db::list_visible_api_keys(&pool)
        .await
        .expect("list keys")
        .pop()
        .expect("created key");
    finelor::db::revoke_api_key(&pool, key.id)
        .await
        .expect("revoke key");
    let app = public_api_app(pool);

    let response = app
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
async fn unrevoked_api_key_can_authenticate_again() {
    let pool = common::in_memory_pool().await;
    let token = create_test_api_key(&pool).await;
    let key = finelor::db::list_visible_api_keys(&pool)
        .await
        .expect("list keys")
        .pop()
        .expect("created key");
    finelor::db::revoke_api_key(&pool, key.id)
        .await
        .expect("revoke key");
    finelor::db::unrevoke_api_key(&pool, key.id)
        .await
        .expect("unrevoke key");
    let app = public_api_app(pool);

    let response = app
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

    assert!(response.status().is_success());
}

#[tokio::test]
async fn removed_api_key_is_hidden_and_cannot_authenticate() {
    let pool = common::in_memory_pool().await;
    let token = create_test_api_key(&pool).await;
    let key = finelor::db::list_visible_api_keys(&pool)
        .await
        .expect("list keys")
        .pop()
        .expect("created key");
    finelor::db::hide_api_key(&pool, key.id)
        .await
        .expect("hide key");
    assert!(
        finelor::db::list_visible_api_keys(&pool)
            .await
            .expect("list keys after hide")
            .is_empty(),
        "hidden keys should not appear in the management list"
    );
    assert!(
        finelor::db::reveal_api_key_token(&pool, key.id)
            .await
            .expect("reveal hidden key token")
            .is_none(),
        "hidden keys should not be revealable"
    );
    let app = public_api_app(pool);

    let response = app
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
