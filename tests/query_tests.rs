//! Tests for single-workspace accounting query service behavior.

mod common;

use sqlx::SqlitePool;
use uuid::Uuid;

async fn create_document(pool: &SqlitePool, status: &str) -> (i64, String) {
    let document_token = Uuid::new_v4();
    let file_hash = format!("query_test_hash_{}", document_token.simple());

    let row: (i64, String) = sqlx::query_as(
        r#"
        INSERT INTO documents (filename, status, file_hash)
        VALUES ('query-test.pdf', $1, $2)
        RETURNING id, short_ref
        "#,
    )
    .bind(status)
    .bind(file_hash)
    .fetch_one(pool)
    .await
    .expect("create document");

    row
}

async fn cleanup_documents(pool: &SqlitePool, document_ids: &[i64]) {
    for document_id in document_ids {
        let _ = sqlx::query("DELETE FROM document_artifacts WHERE document_id = $1")
            .bind(document_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
            .bind(document_id)
            .execute(pool)
            .await;
    }
}

#[tokio::test]
async fn company_profile_returns_workspace_metadata() {
    let pool = common::in_memory_pool().await;

    let profile = finelor::query::workspace_profile(&pool)
        .await
        .expect("company profile")
        .expect("workspace profile exists");
    assert_eq!(profile.id, 1);
}

#[tokio::test]
async fn workspace_profile_reads_persisted_company_profile_values() {
    let pool = common::in_memory_pool().await;
    sqlx::query(
        r#"
        UPDATE company_profile
        SET display_name = $1, jurisdiction = $2, updated_at = CURRENT_TIMESTAMP
        WHERE singleton = TRUE
        "#,
    )
    .bind("Profile Query Test AB")
    .bind("SE")
    .execute(&pool)
    .await
    .expect("update company_profile");

    let profile = finelor::query::workspace_profile(&pool)
        .await
        .expect("workspace profile query")
        .expect("workspace profile exists");
    assert_eq!(profile.display_name, "Profile Query Test AB");
    assert_eq!(profile.jurisdiction.as_deref(), Some("SE"));
}

#[tokio::test]
async fn query_counts_and_lists_use_single_workspace_scope() {
    let pool = common::in_memory_pool().await;

    let (ready_id, ready_ref) = create_document(&pool, "EXPORT_READY").await;
    let (pending_id, pending_ref) = create_document(&pool, "PENDING_HUMAN_REVIEW").await;

    let counts = finelor::query::document_status_counts(&pool)
        .await
        .expect("counts");
    assert!(counts.ready_count >= 1);
    assert!(counts.pending_count >= 1);

    let ready = finelor::query::list_documents_by_status(&pool, "EXPORT_READY", 50)
        .await
        .expect("ready list");
    assert!(ready.iter().any(|d| d.short_ref == ready_ref));

    let attention = finelor::query::list_documents_requiring_attention(&pool, 50)
        .await
        .expect("attention list");
    assert!(attention.iter().any(|d| d.short_ref == pending_ref));

    cleanup_documents(&pool, &[ready_id, pending_id]).await;
}

#[tokio::test]
async fn source_media_artifact_lookup_returns_latest_for_workspace_document() {
    let pool = common::in_memory_pool().await;
    let (document_id, _) = create_document(&pool, "PENDING_HUMAN_REVIEW").await;

    sqlx::query(
        r#"
        INSERT INTO document_artifacts (
            document_id,
            channel_type,
            channel_identifier,
            profile_identifier,
            external_artifact_id,
            original_filename,
            mime_type,
            file_hash,
            metadata
        )
        VALUES ($1, 'TELEGRAM', '1593935753', '12345', 'tg_1593935753_99', 'receipt.jpg', 'image/jpeg', $2, $3)
        "#,
    )
    .bind(document_id)
    .bind(format!("query_media_hash_{}", Uuid::new_v4().simple()))
    .bind(serde_json::json!({
        "file_id": "telegram-file-id",
        "message_id": 99,
    }))
    .execute(&pool)
    .await
    .expect("insert document artifact");

    let artifact = finelor::query::latest_source_media_artifact(&pool, document_id)
        .await
        .expect("artifact lookup")
        .expect("artifact exists");

    assert_eq!(artifact.channel_type, "TELEGRAM");
    assert_eq!(
        artifact.external_file_id.as_deref(),
        Some("telegram-file-id")
    );

    cleanup_documents(&pool, &[document_id]).await;
}

#[tokio::test]
async fn read_only_tools_return_workspace_documents() {
    let pool = common::in_memory_pool().await;

    let (pending_id, pending_ref) = create_document(&pool, "PENDING_HUMAN_REVIEW").await;
    let (ready_id, ready_ref) = create_document(&pool, "EXPORT_READY").await;

    let pending = finelor::messaging::tools::execute_read_only_tool(
        &pool,
        None,
        &finelor::messaging::tools::ReadOnlyToolCall {
            name: "list_pending_reviews".to_string(),
            args: serde_json::json!({ "limit": 50 }),
        },
    )
    .await;
    assert_eq!(pending["ok"], true);
    assert!(
        pending["result"]["total_count"]
            .as_i64()
            .unwrap_or_default()
            >= 1
    );

    let get_doc = finelor::messaging::tools::execute_read_only_tool(
        &pool,
        None,
        &finelor::messaging::tools::ReadOnlyToolCall {
            name: "get_document".to_string(),
            args: serde_json::json!({ "document_short_ref": pending_ref }),
        },
    )
    .await;
    assert_eq!(get_doc["ok"], true);
    assert_eq!(get_doc["result"]["found"], true);

    let ready = finelor::messaging::tools::execute_read_only_tool(
        &pool,
        None,
        &finelor::messaging::tools::ReadOnlyToolCall {
            name: "list_export_ready".to_string(),
            args: serde_json::json!({ "limit": 50 }),
        },
    )
    .await;
    assert_eq!(ready["ok"], true);
    assert!(ready["result"]["items"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|row| row["document_short_ref"] == ready_ref)
    }));

    cleanup_documents(&pool, &[pending_id, ready_id]).await;
}
