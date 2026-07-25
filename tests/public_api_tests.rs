mod common;

use std::sync::Arc;

use axum::Router;
use common::TestDocumentStatePreset as Preset;
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

    let token = finelor::db::create_api_key(pool, "Test key", user_id)
        .await
        .expect("create api key")
        .token;
    assert!(token.starts_with("finelor_api_"));
    token
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
async fn public_api_ingestion_accepts_uploads_over_axum_default_body_limit() {
    let pool = common::in_memory_pool().await;
    let token = create_test_api_key(&pool).await;
    let app = public_api_app(pool);

    let boundary = "----FinelorLargeBoundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        b"------FinelorLargeBoundary\r\n\
Content-Disposition: form-data; name=\"file\"; filename=\"large-invoice.pdf\"\r\n\
Content-Type: application/pdf\r\n\r\n",
    );
    body.extend(std::iter::repeat_n(b'a', 2 * 1024 * 1024 + 1));
    body.extend_from_slice(b"\r\n------FinelorLargeBoundary--\r\n");

    let response = app
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
        "large upload failed: {}",
        response.status()
    );
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

    let status_response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/documents/status")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(status_response.status().is_success());
    let status_body = axum::body::to_bytes(status_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
    assert_eq!(status["documents_total"].as_i64(), Some(1));
    assert_eq!(status["intake"]["processing"].as_i64(), Some(1));
    assert_eq!(status["intake"]["ingested"].as_i64(), Some(0));
    assert_eq!(status["intake"]["failed"].as_i64(), Some(0));
    assert_eq!(status["accounting"]["processing"].as_i64(), Some(0));
    assert_eq!(status["accounting"]["pending_review"].as_i64(), Some(0));
    assert_eq!(status["accounting"]["ready_for_export"].as_i64(), Some(0));
    assert_eq!(status["accounting"]["exported"].as_i64(), Some(0));
    assert_eq!(status["accounting"]["failed"].as_i64(), Some(0));

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
    assert_eq!(detail["filename"].as_str(), Some("invoice.pdf"));
    assert_eq!(
        detail["download_url"].as_str(),
        Some(expected_download_url.as_str())
    );
    assert!(
        detail.get("original_path").is_none(),
        "public detail must not expose original_path"
    );

    let explain_response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri(format!("/documents/{short_ref}/explain"))
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(explain_response.status().is_success());
    let explain_body = axum::body::to_bytes(explain_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let explain: serde_json::Value = serde_json::from_slice(&explain_body).unwrap();
    assert_eq!(explain["short_ref"].as_str(), Some(short_ref.as_str()));
    assert!(
        explain["explanation"]
            .as_str()
            .unwrap_or_default()
            .contains(&format!("{short_ref} is currently"))
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
    assert_eq!(
        file_response
            .headers()
            .get(axum::http::header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok()),
        Some("attachment; filename=\"invoice.pdf\"")
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
async fn public_api_lists_documents_with_domain_filters() {
    let pool = common::in_memory_pool().await;
    let token = create_test_api_key(&pool).await;
    let app = public_api_app(pool.clone());

    let _ = common::create_document_with_state_preset(
        &pool,
        Preset::ReadyForExport,
        None,
        "application/pdf",
    )
    .await;
    let (pending_id, pending_ref) = common::create_document_with_state_preset(
        &pool,
        Preset::PendingReview,
        None,
        "application/pdf",
    )
    .await;

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/documents?accounting_status=PENDING_REVIEW")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(response.status().is_success());
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["total"].as_i64(), Some(1));
    assert_eq!(
        list["items"][0]["short_ref"].as_str(),
        Some(pending_ref.as_str())
    );
    assert_eq!(
        list["items"][0]["status"]["accounting"]["status"].as_str(),
        Some("PENDING_REVIEW")
    );

    let _ = sqlx::query("DELETE FROM document_artifacts WHERE document_id = $1")
        .bind(pending_id)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn public_api_filename_field_overrides_upload_part_filename_for_display() {
    let pool = common::in_memory_pool().await;
    let token = create_test_api_key(&pool).await;
    let app = public_api_app(pool.clone());

    let boundary = "----FinelorBoundary";
    let body = concat!(
        "------FinelorBoundary\r\n",
        "Content-Disposition: form-data; name=\"file\"; filename=\"invoice.pdf\"\r\n",
        "Content-Type: application/pdf\r\n\r\n",
        "%PDF-1.4 public api filename override test\r\n",
        "------FinelorBoundary\r\n",
        "Content-Disposition: form-data; name=\"filename\"\r\n\r\n",
        "custom-name.pdf\r\n",
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
    assert_eq!(detail["filename"].as_str(), Some("custom-name.pdf"));

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
            .get(axum::http::header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok()),
        Some("attachment; filename=\"custom-name.pdf\"")
    );

    let stored_filename: String =
        sqlx::query_scalar("SELECT filename FROM documents WHERE short_ref = $1")
            .bind(&short_ref)
            .fetch_one(&pool)
            .await
            .expect("stored filename");
    assert_ne!(stored_filename, "custom-name.pdf");

    let artifact_filename: String = sqlx::query_scalar(
        r#"
        SELECT original_filename
        FROM document_artifacts
        WHERE document_id = (SELECT id FROM documents WHERE short_ref = $1)
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(&short_ref)
    .fetch_one(&pool)
    .await
    .expect("artifact filename");
    assert_eq!(artifact_filename, "custom-name.pdf");
}

#[tokio::test]
async fn public_api_explain_returns_not_found_for_unknown_document() {
    let pool = common::in_memory_pool().await;
    let token = create_test_api_key(&pool).await;
    let app = public_api_app(pool);

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/documents/D999999/explain")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
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
