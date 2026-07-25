//! Database Integration Tests for Finelor Agents
//!
//! Tests for database operations including:
//! - validation_results INSERT with ON CONFLICT
//! - review_decisions INSERT with ON CONFLICT
//! - Foreign key constraints
//! - Transaction rollback on errors
//!
//! NOTE: These tests use an isolated in-memory SQLite database per test.

mod common;

use common::TestDocumentStatePreset as Preset;
use serde_json::json;
use sqlx::SqlitePool;

/// Helper to create a test document
async fn create_test_document(pool: &SqlitePool) -> i64 {
    common::create_document_with_state_preset(pool, Preset::IntakeReceived, None, "application/pdf")
        .await
        .0
}

/// Test validation_results INSERT behavior
/// This documents the current behavior - multiple inserts are allowed (no unique constraint)
#[tokio::test]
async fn test_validation_results_insert_behavior() {
    let pool = common::in_memory_pool().await;

    // Create document within the test
    let document_id = create_test_document(&pool).await;

    // First insert should succeed
    let result = sqlx::query(
        r#"
        INSERT INTO validation_results 
        (document_id, checks_passed, checks_failed, validation_errors, overall_status, checked_at)
        VALUES ($1, 3, 0, $2, 'VALID', CURRENT_TIMESTAMP)
        "#,
    )
    .bind(document_id)
    .bind(json!([{"field": "test", "message": "test error"}]))
    .execute(&pool)
    .await;

    assert!(result.is_ok(), "First insert should succeed");

    // Second insert - current schema allows multiple results per document
    let result = sqlx::query(
        r#"
        INSERT INTO validation_results 
        (document_id, checks_passed, checks_failed, validation_errors, overall_status, checked_at)
        VALUES ($1, 5, 1, $2, 'INVALID', CURRENT_TIMESTAMP)
        "#,
    )
    .bind(document_id)
    .bind(json!([{"field": "test2", "message": "another error"}]))
    .execute(&pool)
    .await;

    assert!(
        result.is_ok(),
        "Multiple validation results per document are allowed"
    );

    // Verify we have 2 records
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM validation_results WHERE document_id = $1")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(count, 2, "Should have 2 validation results");

    // Cleanup
    let _ = sqlx::query("DELETE FROM validation_results WHERE document_id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
}

/// Test review_decisions UPSERT behavior
#[tokio::test]
async fn test_review_decisions_insert_behavior() {
    let pool = common::in_memory_pool().await;
    let document_id = create_test_document(&pool).await;

    // First insert
    let result = sqlx::query(
        r#"
        INSERT INTO review_decisions 
        (document_id, confidence_score, decision_type, human_review_required, review_reason)
        VALUES ($1, 0.85, 'AUTO_APPROVED', FALSE, 'High confidence')
        "#,
    )
    .bind(document_id)
    .execute(&pool)
    .await;

    assert!(result.is_ok(), "First insert should succeed");

    // Second write updates the one decision row for this document.
    let result = sqlx::query(
        r#"
        INSERT INTO review_decisions 
        (document_id, confidence_score, decision_type, human_review_required, review_reason)
        VALUES ($1, 0.45, 'PENDING_HUMAN_REVIEW', TRUE, 'Low confidence')
        ON CONFLICT (document_id) DO UPDATE SET
            confidence_score = EXCLUDED.confidence_score,
            decision_type = EXCLUDED.decision_type,
            human_review_required = EXCLUDED.human_review_required,
            review_reason = EXCLUDED.review_reason
        "#,
    )
    .bind(document_id)
    .execute(&pool)
    .await;

    assert!(
        result.is_ok(),
        "Review decision upsert should update the existing document row"
    );

    // Get latest decision
    let latest: (String, String) = sqlx::query_as(
        r#"
        SELECT decision_type, CAST(confidence_score AS TEXT) 
        FROM review_decisions 
        WHERE document_id = $1 
        ORDER BY reviewed_at DESC 
        LIMIT 1
        "#,
    )
    .bind(document_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(latest.0, "PENDING_HUMAN_REVIEW");
    assert_eq!(latest.1, "0.45", "Should retrieve latest confidence score");

    // Cleanup
    let _ = sqlx::query("DELETE FROM review_decisions WHERE document_id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
}

/// Test foreign key constraint violations
#[tokio::test]
async fn test_foreign_key_constraint_violation() {
    let pool = common::in_memory_pool().await;
    let fake_document_id = -999_999_i64; // Non-existent document

    // Try to insert validation_result with non-existent document_id
    let result = sqlx::query(
        r#"
        INSERT INTO validation_results 
        (document_id, checks_passed, checks_failed, validation_errors, overall_status, checked_at)
        VALUES ($1, 3, 0, $2, 'VALID', CURRENT_TIMESTAMP)
        "#,
    )
    .bind(fake_document_id)
    .bind(json!([{"field": "test", "message": "test"}]))
    .execute(&pool)
    .await;

    assert!(result.is_err(), "Should fail with FK constraint violation");
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    let err_str_lower = err_str.to_lowercase();
    assert!(
        err_str_lower.contains("foreign key")
            || err_str_lower.contains("violates foreign-key")
            || err_str_lower.contains("is not present"),
        "Error should indicate FK violation: {}",
        err_str
    );
}

/// Regression: intervention candidate query must not depend on documents.workspace_id.
#[tokio::test]
async fn test_intervention_candidate_lookup_without_document_workspace_column() {
    let pool = common::in_memory_pool().await;

    let document_id = create_test_document(&pool).await;
    let event_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO document_events (document_id, event_type, payload)
        VALUES ($1, 'VISION_COMPLETED', '{}')
        RETURNING id
        "#,
    )
    .bind(document_id)
    .fetch_one(&pool)
    .await
    .expect("insert document event");

    let candidate = finelor::messaging::interventions::document_intervention_candidate_by_event_id(
        &pool, event_id,
    )
    .await
    .expect("intervention candidate query should succeed")
    .expect("candidate row should exist");

    assert_eq!(candidate.event_id, event_id);
    assert_eq!(candidate.document_id, document_id);

    let short_ref: String = sqlx::query_scalar("SELECT short_ref FROM documents WHERE id = $1")
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .expect("short_ref lookup");
    assert_eq!(candidate.short_ref, short_ref);

    let _ = sqlx::query("DELETE FROM document_events WHERE id = $1")
        .bind(event_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
}

/// Test transaction rollback on error
#[tokio::test]
async fn test_transaction_rollback() {
    let pool = common::in_memory_pool().await;
    let document_id = create_test_document(&pool).await;

    // Start transaction
    let mut tx = pool.begin().await.expect("Failed to start transaction");

    // Insert within transaction
    let result = sqlx::query(
        r#"
        INSERT INTO validation_results 
        (document_id, checks_passed, checks_failed, validation_errors, overall_status, checked_at)
        VALUES ($1, 3, 0, $2, 'VALID', CURRENT_TIMESTAMP)
        "#,
    )
    .bind(document_id)
    .bind(json!([{"field": "tx_test", "message": "test"}]))
    .execute(&mut *tx)
    .await;

    assert!(result.is_ok(), "Insert within transaction should succeed");

    // Verify the record exists within transaction
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM validation_results WHERE document_id = $1")
            .bind(document_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();

    assert_eq!(count, 1, "Record should exist in transaction");

    // Rollback transaction
    tx.rollback().await.expect("Failed to rollback");

    // Verify record doesn't exist after rollback
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM validation_results WHERE document_id = $1")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(count, 0, "Record should not exist after rollback");

    // Cleanup
    let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
}

/// Test DECIMAL(3,2) for confidence_score storage
#[tokio::test]
async fn test_confidence_score_decimal_storage() {
    let pool = common::in_memory_pool().await;

    // Test storing confidence scores
    let test_scores = vec![
        ("0.95", "AUTO_APPROVED"),
        ("0.75", "PENDING_HUMAN_REVIEW"),
        ("0.50", "PENDING_HUMAN_REVIEW"),
    ];

    for (score, decision) in test_scores {
        let document_id = create_test_document(&pool).await;
        let result = sqlx::query(
            r#"
            INSERT INTO review_decisions 
            (document_id, confidence_score, decision_type, human_review_required)
            VALUES ($1, $2::DECIMAL(3,2), $3, FALSE)
            "#,
        )
        .bind(document_id)
        .bind(score)
        .bind(decision)
        .execute(&pool)
        .await;

        assert!(result.is_ok(), "Should store confidence score {}", score);

        let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
            .bind(document_id)
            .execute(&pool)
            .await;
    }
}

/// Test JSONB field storage and retrieval
#[tokio::test]
async fn test_jsonb_field_operations() {
    let pool = common::in_memory_pool().await;
    let document_id = create_test_document(&pool).await;

    // Insert with complex JSONB
    let errors = json!([
        {"field": "org_nr", "severity": "ERROR", "message": "Invalid checksum"},
        {"field": "vat_amount", "severity": "WARNING", "message": "Does not match calculation"}
    ]);

    let result = sqlx::query(
        r#"
        INSERT INTO validation_results 
        (document_id, checks_passed, checks_failed, validation_errors, overall_status, checked_at)
        VALUES ($1, 2, 2, $2, 'NEEDS_REVIEW', CURRENT_TIMESTAMP)
        "#,
    )
    .bind(document_id)
    .bind(&errors)
    .execute(&pool)
    .await;

    assert!(result.is_ok(), "Should store JSONB validation_errors");

    // Retrieve and verify JSONB
    let retrieved: serde_json::Value = sqlx::query_scalar(
        "SELECT validation_errors FROM validation_results WHERE document_id = $1",
    )
    .bind(document_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(retrieved, errors, "Retrieved JSONB should match inserted");

    // Cleanup
    let _ = sqlx::query("DELETE FROM validation_results WHERE document_id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
}

/// Test extracted_fields INSERT with FK constraint
#[tokio::test]
async fn test_extracted_fields_insert() {
    let pool = common::in_memory_pool().await;
    let document_id = create_test_document(&pool).await;

    // Insert extracted field
    let result = sqlx::query(
        r#"
        INSERT INTO extracted_fields 
        (document_id, field_type, raw_value, parsed_value, confidence, source)
        VALUES ($1, 'supplier_name', 'Acme Corp AB', 'Acme Corporation', 0.92, 'VisionAgent')
        "#,
    )
    .bind(document_id)
    .execute(&pool)
    .await;

    assert!(result.is_ok(), "Should insert extracted field");

    // Verify
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM extracted_fields WHERE document_id = $1")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(count, 1);

    // Cleanup
    let _ = sqlx::query("DELETE FROM extracted_fields WHERE document_id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
}

/// Regression: extracted_fields.confidence must decode as SQLite numeric
/// for deterministic validator context loading.
#[tokio::test]
async fn test_extracted_fields_confidence_decodes_as_f64() {
    let pool = common::in_memory_pool().await;
    let document_id = create_test_document(&pool).await;

    sqlx::query(
        r#"
        INSERT INTO extracted_fields
        (document_id, field_type, raw_value, parsed_value, confidence, source)
        VALUES ($1, 'supplier_name', 'Test Supplier', 'Test Supplier', 0.90, 'vision')
        "#,
    )
    .bind(document_id)
    .execute(&pool)
    .await
    .expect("insert extracted field");

    let confidence: Option<f64> = sqlx::query_scalar(
        r#"
        SELECT confidence
        FROM extracted_fields
        WHERE document_id = $1
          AND field_type = 'supplier_name'
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(document_id)
    .fetch_optional(&pool)
    .await
    .expect("decode confidence as f64");

    assert_eq!(confidence.expect("confidence value exists"), 0.9);

    let _ = sqlx::query("DELETE FROM extracted_fields WHERE document_id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
}

/// Test NULL value handling in database
#[tokio::test]
async fn test_null_value_handling() {
    let pool = common::in_memory_pool().await;
    let document_id = create_test_document(&pool).await;

    // Insert with NULL confidence_score
    let result = sqlx::query(
        r#"
        INSERT INTO review_decisions 
        (document_id, confidence_score, decision_type, human_review_required)
        VALUES ($1, NULL, 'ERROR', FALSE)
        "#,
    )
    .bind(document_id)
    .execute(&pool)
    .await;

    assert!(result.is_ok(), "Should allow NULL confidence_score");

    // Retrieve NULL value
    let score: Option<String> = sqlx::query_scalar(
        "SELECT CAST(confidence_score AS TEXT) FROM review_decisions WHERE document_id = $1",
    )
    .bind(document_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(score.is_none(), "NULL confidence_score should be None");

    // Cleanup
    let _ = sqlx::query("DELETE FROM review_decisions WHERE document_id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
}

/// Test cleanup: CASCADE DELETE behavior
#[tokio::test]
async fn test_cascade_delete_documents() {
    let pool = common::in_memory_pool().await;
    let document_id = create_test_document(&pool).await;

    // Insert related records
    sqlx::query(
        r#"
        INSERT INTO validation_results 
        (document_id, checks_passed, checks_failed, validation_errors, overall_status, checked_at)
        VALUES ($1, 3, 0, '[]', 'VALID', CURRENT_TIMESTAMP)
        "#,
    )
    .bind(document_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO review_decisions 
        (document_id, confidence_score, decision_type, human_review_required)
        VALUES ($1, 0.85, 'AUTO_APPROVED', FALSE)
        "#,
    )
    .bind(document_id)
    .execute(&pool)
    .await
    .unwrap();

    // Verify records exist
    let val_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM validation_results WHERE document_id = $1")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(val_count, 1);

    // Delete document (should CASCADE)
    sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(document_id)
        .execute(&pool)
        .await
        .unwrap();

    // Verify related records deleted
    let val_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM validation_results WHERE document_id = $1")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        val_count, 0,
        "Validation results should be deleted with CASCADE"
    );
}

/// Regression test: NULL corrections should decode safely via fetch_optional(...).flatten()
#[tokio::test]
async fn test_review_corrections_nullable_decode_pattern() {
    let pool = common::in_memory_pool().await;
    let document_id = create_test_document(&pool).await;

    sqlx::query(
        r#"
        INSERT INTO review_decisions
        (document_id, confidence_score, decision_type, human_review_required, corrections)
        VALUES ($1, 0.58, 'PENDING_HUMAN_REVIEW', TRUE, NULL)
        "#,
    )
    .bind(document_id)
    .execute(&pool)
    .await
    .expect("Should insert review decision with NULL corrections");

    let decoded: Option<serde_json::Value> = sqlx::query_scalar::<_, Option<serde_json::Value>>(
        "SELECT corrections FROM review_decisions WHERE document_id = $1",
    )
    .bind(document_id)
    .fetch_optional(&pool)
    .await
    .expect("Nullable corrections query should succeed")
    .flatten();

    assert!(
        decoded.is_none(),
        "NULL corrections should decode to None instead of erroring"
    );

    let _ = sqlx::query("DELETE FROM review_decisions WHERE document_id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
}

/// Regression test: accounting_decisions numeric fields should decode as SQLite numeric values.
#[allow(clippy::type_complexity)]
#[tokio::test]
async fn test_accounting_decisions_numeric_decode_as_f64() {
    let pool = common::in_memory_pool().await;
    let document_id = create_test_document(&pool).await;

    sqlx::query(
        r#"
        INSERT INTO accounting_decisions
        (document_id, assigned_account_code, account_name, vat_rate, vat_amount, net_amount, gross_amount, reasoning_text)
        VALUES ($1, '6110', 'Office supplies', 0.25, 25.00, 100.00, 125.00, 'Valid accounting decision')
        "#,
    )
    .bind(document_id)
    .execute(&pool)
    .await
    .expect("Should insert accounting decision");

    let decoded: (
        Option<String>,
        Option<String>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<String>,
    ) = sqlx::query_as(
        r#"
        SELECT
            assigned_account_code,
            account_name,
            vat_rate,
            vat_amount,
            net_amount,
            gross_amount,
            reasoning_text
        FROM accounting_decisions
        WHERE document_id = $1
        LIMIT 1
        "#,
    )
    .bind(document_id)
    .fetch_one(&pool)
    .await
    .expect("numeric fields should decode via f64");

    assert_eq!(decoded.0.as_deref(), Some("6110"));
    assert_eq!(decoded.1.as_deref(), Some("Office supplies"));
    assert_eq!(decoded.2, Some(0.25));
    assert_eq!(decoded.3, Some(25.0));
    assert_eq!(decoded.4, Some(100.0));
    assert_eq!(decoded.5, Some(125.0));
    assert_eq!(decoded.6.as_deref(), Some("Valid accounting decision"));

    let _ = sqlx::query("DELETE FROM accounting_decisions WHERE document_id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
}

/// Regression test: retry cleanup must delete account_assignments through invoices.invoice_id.
#[tokio::test]
async fn test_retry_cleanup_deletes_account_assignments_via_invoice_id() {
    let pool = common::in_memory_pool().await;
    let document_id = create_test_document(&pool).await;
    let invoice_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO invoices (document_id, supplier_name, total_amount, status)
        VALUES ($1, 'Retry Test Supplier', 35.00, 'PENDING_ANALYSIS')
        RETURNING id
        "#,
    )
    .bind(document_id)
    .fetch_one(&pool)
    .await
    .expect("Should insert invoice");

    sqlx::query(
        r#"
        INSERT INTO account_assignments (invoice_id, account_code, account_name, amount)
        VALUES ($1, '6110', 'Office supplies', 35.00)
        "#,
    )
    .bind(invoice_id)
    .execute(&pool)
    .await
    .expect("Should insert account assignment");

    let before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM account_assignments WHERE invoice_id = $1")
            .bind(invoice_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, 1, "Assignment should exist before cleanup");

    sqlx::query(
        r#"
        DELETE FROM account_assignments
        WHERE invoice_id IN (
            SELECT id
            FROM invoices
            WHERE document_id = $1
        )
        "#,
    )
    .bind(document_id)
    .execute(&pool)
    .await
    .expect("Retry cleanup delete should succeed");

    let after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM account_assignments WHERE invoice_id = $1")
            .bind(invoice_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after, 0,
        "Retry cleanup should delete assignment through invoice_id"
    );

    let _ = sqlx::query("DELETE FROM invoices WHERE id = $1")
        .bind(invoice_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(document_id)
        .execute(&pool)
        .await;
}
