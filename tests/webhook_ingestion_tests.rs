//! Webhook ingestion integration tests.
//!
//! These tests use an isolated in-memory SQLite database per test.

mod common;

use std::sync::Arc;

use axum::{Json, Router, extract::State};
use sqlx::SqlitePool;
use tower::util::ServiceExt;

#[tokio::test]
async fn test_webhook_ingest_creates_document() {
    let pool = common::in_memory_pool().await;

    let config = Arc::new(common::test_config());

    let queue = finelor::queue::InMemoryJobQueue::default();
    let queue_producer = finelor::queue::QueueProducer::new(Arc::new(queue));

    let app_state = AppState {
        config: config.clone(),
        pool: pool.clone(),
        queue_producer,
    };

    let app = build_test_router(app_state.clone());

    // Build multipart request with a small fake PDF file
    let boundary = "----WebKitFormBoundary7MA4YWxkTrZu0gW";
    let body = "------WebKitFormBoundary7MA4YWxkTrZu0gW\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.4 fake\r\n------WebKitFormBoundary7MA4YWxkTrZu0gW--\r\n".to_string();

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/ingest")
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={}", boundary),
                )
                .body(axum::body::Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let response = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        status.is_success(),
        "Ingest failed with status {status}: {}",
        String::from_utf8_lossy(&response)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&response).expect("Response should be JSON");
    let short_ref = json["short_ref"].as_str().expect("short_ref should exist");
    assert!(short_ref.starts_with('D'));

    // Verify DB record exists
    let row: (String,) = sqlx::query_as("SELECT status FROM documents WHERE short_ref = $1")
        .bind(short_ref)
        .fetch_one(&pool)
        .await
        .expect("Document should exist in DB");

    assert!(
        matches!(row.0.as_str(), "RECEIVED" | "PROCESSING_VISION"),
        "unexpected document status after ingest: {}",
        row.0
    );

    // Cleanup
    let _ = sqlx::query("DELETE FROM documents WHERE short_ref = $1")
        .bind(short_ref)
        .execute(&pool)
        .await;
}

#[derive(Clone)]
struct AppState {
    config: Arc<finelor::config::AppConfig>,
    pool: SqlitePool,
    queue_producer: finelor::queue::QueueProducer,
}

fn build_test_router(state: AppState) -> Router {
    Router::new()
        .route("/ingest", axum::routing::post(ingest_handler))
        .with_state(state)
}

async fn ingest_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let mut file_bytes = Vec::new();
    let mut filename = None;
    let mut mime_type = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e.to_string()))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            if filename.is_none() {
                filename = field.file_name().map(|s| s.to_string());
            }
            if mime_type.is_none() {
                mime_type = field.content_type().map(|s| s.to_string());
            }
            file_bytes = field
                .bytes()
                .await
                .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e.to_string()))?
                .to_vec();
        }
    }

    if file_bytes.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Missing 'file' field".to_string(),
        ));
    }

    let mime_type = mime_type.unwrap_or_else(|| "application/octet-stream".to_string());
    let original_filename = filename.clone();
    let filename = filename.unwrap_or_else(|| "upload.bin".to_string());

    let source_timestamp = headers
        .get("X-Source-Timestamp")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let profile_identifier = headers
        .get("X-Submitter-ID")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let result = finelor::ingestion::ingest_document(
        &state.pool,
        state.config.as_ref(),
        &state.queue_producer,
        None,
        finelor::ingestion::IngestionInput {
            file_bytes,
            filename,
            mime_type,
            document_type: "INVOICE".to_string(),
            artifact: finelor::ingestion::DocumentArtifactInput {
                channel_type: "API".to_string(),
                channel_identifier: "test_webhook_001".to_string(),
                profile_identifier,
                external_artifact_id: Some("test_webhook_001".to_string()),
                source_timestamp,
                original_filename,
                metadata: serde_json::json!({"test": true}),
            },
        },
    )
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "document_id": result.id,
        "short_ref": result.short_ref,
        "status": "RECEIVED",
    })))
}

#[tokio::test]
async fn test_webhook_ingest_persists_provenance_fields() {
    let pool = common::in_memory_pool().await;

    let config = Arc::new(common::test_config());

    let queue = finelor::queue::InMemoryJobQueue::default();
    let queue_producer = finelor::queue::QueueProducer::new(Arc::new(queue));

    let app_state = AppState {
        config: config.clone(),
        pool: pool.clone(),
        queue_producer,
    };

    let app = build_test_router(app_state.clone());

    // Build multipart request with provenance headers
    let boundary = "----WebKitFormBoundary7MA4YWxkTrZu0gW";
    let body = "------WebKitFormBoundary7MA4YWxkTrZu0gW\r\nContent-Disposition: form-data; name=\"file\"; filename=\"my-invoice.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.4 fake provenance unique\r\n------WebKitFormBoundary7MA4YWxkTrZu0gW--\r\n".to_string();

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/ingest")
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={}", boundary),
                )
                .header("X-Source-Timestamp", "2026-05-03T10:00:00Z")
                .header("X-Submitter-ID", "submitter-42")
                .body(axum::body::Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let response = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        status.is_success(),
        "Ingest failed with status {status}: {}",
        String::from_utf8_lossy(&response)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&response).expect("Response should be JSON");
    let short_ref = json["short_ref"].as_str().expect("short_ref should exist");
    assert!(short_ref.starts_with('D'));

    // Verify DB records contain document state and artifact provenance
    let status: String = sqlx::query_scalar("SELECT status FROM documents WHERE short_ref = $1")
        .bind(short_ref)
        .fetch_one(&pool)
        .await
        .expect("Document should exist in DB");
    let row: (Option<String>, Option<String>, Option<String>, String) = sqlx::query_as(
        r#"
        SELECT da.original_filename, da.source_timestamp, da.profile_identifier, da.channel_type
        FROM document_artifacts da
        JOIN documents d ON d.id = da.document_id
        WHERE d.short_ref = $1
        ORDER BY da.created_at DESC
        LIMIT 1
        "#,
    )
    .bind(short_ref)
    .fetch_one(&pool)
    .await
    .expect("Document should exist in DB");

    assert_eq!(row.0.as_deref(), Some("my-invoice.pdf"));
    assert_eq!(row.1.as_deref(), Some("2026-05-03T10:00:00Z"));
    assert_eq!(row.2.as_deref(), Some("submitter-42"));
    assert_eq!(row.3, "API");
    assert!(
        matches!(status.as_str(), "RECEIVED" | "PROCESSING_VISION"),
        "unexpected document status after ingest: {status}"
    );

    // Cleanup
    let _ = sqlx::query("DELETE FROM documents WHERE short_ref = $1")
        .bind(short_ref)
        .execute(&pool)
        .await;
}
