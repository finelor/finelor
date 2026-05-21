//! In-memory integration tests for recovery and pipeline boundaries.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use finelor::agents::{
    AccountantAgent, AccountantInput, Agent, AgentContext, DocumentStatus, FieldLoadMode,
    IntakeAgent, IntakeInput, ValidatorAgent, ValidatorInput, VisionAgent, VisionInput,
};
use finelor::ingestion::{DocumentArtifactInput, IngestionInput, ingest_document};
use finelor::orchestration::recover_incomplete_jobs;
use finelor::queue::{JobType, QueueProducer};
use finelor::web::events::AppEventBus;
use serde_json::json;
use sqlx::{Row, SqlitePool};

fn context(pool: SqlitePool) -> AgentContext {
    AgentContext::new(pool, common::test_config(), AppEventBus::new(16))
}

async fn create_document(pool: &SqlitePool, status: &str) -> i64 {
    let hash = format!("pipeline-test-{}", uuid::Uuid::new_v4());
    sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO documents (filename, status, file_hash, original_path, mime_type)
        VALUES ('pipeline-test.png', $1, $2, '', 'image/png')
        RETURNING id
        "#,
    )
    .bind(status)
    .bind(hash)
    .fetch_one(pool)
    .await
    .expect("insert document")
}

async fn insert_field(pool: &SqlitePool, document_id: i64, field_type: &str, value: &str) {
    sqlx::query(
        r#"
        INSERT INTO extracted_fields (document_id, field_type, raw_value, parsed_value, parsed_type, confidence, source)
        VALUES ($1, $2, $3, $3, 'text', 0.9, 'test')
        "#,
    )
    .bind(document_id)
    .bind(field_type)
    .bind(value)
    .execute(pool)
    .await
    .expect("insert extracted field");
}

async fn seed_validation_context(pool: &SqlitePool, document_id: i64) {
    insert_field(pool, document_id, "supplier_name", "Acme AB").await;
    insert_field(pool, document_id, "supplier_org_nr", "5560160680").await;
    insert_field(pool, document_id, "transaction_date", "2026-05-01").await;
    insert_field(pool, document_id, "invoice_number", "INV-1").await;
    insert_field(pool, document_id, "total_amount", "125.00").await;
    insert_field(pool, document_id, "vat_amount", "25.00").await;
    insert_field(pool, document_id, "vat_rate", "25").await;
    insert_field(pool, document_id, "currency", "SEK").await;

    sqlx::query(
        r#"
        INSERT INTO accounting_decisions (
            document_id, reasoning_text, assigned_account_code, account_name,
            vat_rate, vat_amount, net_amount, gross_amount, ai_confidence, model_used
        )
        VALUES ($1, 'balanced test entry', '6070', 'Representation', 25.0, 25.0, 100.0, 125.0, 0.9, 'test')
        "#,
    )
    .bind(document_id)
    .execute(pool)
    .await
    .expect("insert accounting decision");
}

fn fake_vision_response() -> serde_json::Value {
    json!({
        "document_type": "receipt",
        "confidence": 0.91,
        "extracted_text": "Acme AB receipt",
        "supplier": { "name": "Acme AB", "org_nr": "5560160680" },
        "transaction": { "date": "2026-05-01", "invoice_number": "INV-1", "ocr_number": "123455" },
        "amounts": { "total_incl_vat": "125.00", "vat_amount": "25.00", "vat_rate": "25", "currency": "SEK" },
        "line_items": [{ "description": "Lunch", "amount": "125.00", "quantity": 1 }],
        "quality_issues": [],
        "missing_info": []
    })
}

fn fake_accounting_response() -> serde_json::Value {
    json!({
        "entry_date": "2026-05-01",
        "description": "Acme AB receipt",
        "entries": [
            { "account_code": "6070", "description": "Representation", "debet": 100.0, "kredit": 0.0 },
            { "account_code": "2641", "description": "Input VAT", "debet": 25.0, "kredit": 0.0 },
            { "account_code": "1930", "description": "Bank", "debet": 0.0, "kredit": 125.0 }
        ],
        "verified": true,
        "warnings": []
    })
}

fn write_test_png() -> PathBuf {
    let path = std::env::temp_dir().join(format!("finelor-test-{}.png", uuid::Uuid::new_v4()));
    let image = image::RgbImage::from_pixel(2, 2, image::Rgb([255, 255, 255]));
    image.save(&path).expect("write test png");
    path
}

#[tokio::test]
async fn recovery_queues_expected_jobs_from_incomplete_documents() {
    let pool = common::in_memory_pool().await;
    let queue = common::test_queue();
    let producer = QueueProducer::new(queue.clone());

    for (status, priority) in [
        ("RECEIVED", "NORMAL"),
        ("PROCESSING_VISION", "HIGH"),
        ("VISION_COMPLETE", "LOW"),
        ("PROCESSING_ACCOUNTANT", "HIGH"),
        ("ACCOUNTANT_REVIEWED", "NORMAL"),
        ("PROCESSING_VALIDATOR", "NORMAL"),
        ("VALIDATED", "NORMAL"),
        ("FAILED", "HIGH"),
        ("EXPORT_READY", "HIGH"),
    ] {
        let id = create_document(&pool, status).await;
        sqlx::query("UPDATE documents SET priority = $1 WHERE id = $2")
            .bind(priority)
            .bind(id)
            .execute(&pool)
            .await
            .expect("update priority");
    }

    let recovered = recover_incomplete_jobs(&pool, &producer)
        .await
        .expect("recover jobs");
    assert_eq!(recovered, 7);

    let jobs = common::drain_jobs(&queue, 20).await;
    let job_types = jobs
        .iter()
        .map(|(_, job)| &job.job_type)
        .collect::<Vec<_>>();
    assert_eq!(
        job_types
            .iter()
            .filter(|job_type| matches!(job_type, JobType::Vision))
            .count(),
        2
    );
    assert_eq!(
        job_types
            .iter()
            .filter(|job_type| matches!(job_type, JobType::Accountant))
            .count(),
        2
    );
    assert_eq!(
        job_types
            .iter()
            .filter(|job_type| matches!(job_type, JobType::Validator))
            .count(),
        2
    );
    assert_eq!(
        job_types
            .iter()
            .filter(|job_type| matches!(job_type, JobType::Review))
            .count(),
        1
    );
    assert!(jobs.iter().any(|(_, job)| job.priority == 5));
    assert!(jobs.iter().any(|(_, job)| job.priority == -5));
}

#[tokio::test]
async fn intake_updates_status_records_event_and_enqueues_vision_once() {
    let pool = common::in_memory_pool().await;
    let queue = common::test_queue();
    let producer = QueueProducer::new(queue.clone());
    let document_id = create_document(&pool, "RECEIVED").await;

    let agent = IntakeAgent::new(context(pool.clone()), producer.clone());
    let output = agent
        .process(IntakeInput {
            document_id,
            file_path: PathBuf::from("test.png"),
            mime_type: "image/png".to_string(),
            filename: "test.png".to_string(),
        })
        .await
        .expect("intake process");

    assert!(output.queued_for_vision);
    let status: String = sqlx::query_scalar("SELECT status FROM documents WHERE id = $1")
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .expect("status");
    assert_eq!(status, DocumentStatus::ProcessingVision.as_str());

    let jobs = common::drain_jobs(&queue, 10).await;
    assert_eq!(jobs.len(), 1);
    assert!(matches!(jobs[0].1.job_type, JobType::Vision));

    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM document_events WHERE document_id = $1 AND event_type = 'INTAKE_QUEUED'",
    )
    .bind(document_id)
    .fetch_one(&pool)
    .await
    .expect("event count");
    assert_eq!(event_count, 1);

    let second = agent
        .process(IntakeInput {
            document_id,
            file_path: PathBuf::from("test.png"),
            mime_type: "image/png".to_string(),
            filename: "test.png".to_string(),
        })
        .await
        .expect("duplicate intake process");
    assert!(!second.queued_for_vision);
    assert!(common::drain_jobs(&queue, 10).await.is_empty());
}

#[tokio::test]
async fn duplicate_ingestion_returns_existing_document_without_requeueing() {
    let pool = common::in_memory_pool().await;
    let queue = common::test_queue();
    let producer = QueueProducer::new(queue.clone());
    let config = common::test_config();

    let input = |external_artifact_id: &str| IngestionInput {
        file_bytes: b"same invoice bytes".to_vec(),
        filename: format!("{external_artifact_id}.pdf"),
        mime_type: "application/pdf".to_string(),
        document_type: "INVOICE".to_string(),
        artifact: DocumentArtifactInput {
            channel_type: "API".to_string(),
            channel_identifier: "test-webhook".to_string(),
            profile_identifier: Some("submitter-1".to_string()),
            external_artifact_id: Some(external_artifact_id.to_string()),
            source_timestamp: Some("2026-05-19T10:00:00Z".to_string()),
            original_filename: Some(format!("{external_artifact_id}.pdf")),
            metadata: json!({ "test": true }),
        },
    };

    let first = ingest_document(&pool, &config, &producer, None, input("first"))
        .await
        .expect("first ingestion");
    let first_jobs = common::drain_jobs(&queue, 10).await;
    assert_eq!(first_jobs.len(), 1);

    let second = ingest_document(&pool, &config, &producer, None, input("second"))
        .await
        .expect("duplicate ingestion");
    assert_eq!(second.id, first.id);
    assert_eq!(second.short_ref, first.short_ref);
    assert!(common::drain_jobs(&queue, 10).await.is_empty());

    let artifact_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM document_artifacts WHERE document_id = $1")
            .bind(first.id)
            .fetch_one(&pool)
            .await
            .expect("artifact count");
    assert_eq!(artifact_count, 2);

    let duplicate_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM document_events WHERE document_id = $1 AND event_type = 'DUPLICATE_DETECTED'",
    )
    .bind(first.id)
    .fetch_one(&pool)
    .await
    .expect("duplicate event count");
    assert_eq!(duplicate_events, 1);
}

#[tokio::test]
async fn validator_writes_results_confidence_status_and_events() {
    let pool = common::in_memory_pool().await;
    let document_id = create_document(&pool, "ACCOUNTANT_REVIEWED").await;
    seed_validation_context(&pool, document_id).await;

    let agent = ValidatorAgent::new(context(pool.clone()), FieldLoadMode::OriginalOnly);
    agent
        .process(ValidatorInput { document_id })
        .await
        .expect("validator process");

    let status: String = sqlx::query_scalar("SELECT status FROM documents WHERE id = $1")
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .expect("status");
    assert_eq!(status, DocumentStatus::Validated.as_str());

    let validation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM validation_results WHERE document_id = $1 AND overall_status IS NOT NULL",
    )
    .bind(document_id)
    .fetch_one(&pool)
    .await
    .expect("validation count");
    assert_eq!(validation_count, 1);

    let confidence: Option<f64> =
        sqlx::query_scalar("SELECT confidence_score FROM review_decisions WHERE document_id = $1")
            .bind(document_id)
            .fetch_optional(&pool)
            .await
            .expect("confidence");
    assert!(confidence.unwrap_or_default() > 0.0);

    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM document_events WHERE document_id = $1 AND event_type = 'VALIDATION_COMPLETED'",
    )
    .bind(document_id)
    .fetch_one(&pool)
    .await
    .expect("event count");
    assert_eq!(event_count, 1);
}

#[tokio::test]
async fn vision_agent_uses_fake_inference_and_enqueues_accountant() {
    let pool = common::in_memory_pool().await;
    let queue = common::test_queue();
    let producer = QueueProducer::new(queue.clone());
    let document_id = create_document(&pool, "PROCESSING_VISION").await;
    let image_path = write_test_png();
    sqlx::query("UPDATE documents SET original_path = $1 WHERE id = $2")
        .bind(image_path.to_string_lossy().to_string())
        .bind(document_id)
        .execute(&pool)
        .await
        .expect("update path");

    let fake = common::FakeInferenceProvider::default();
    fake.push_image_json(fake_vision_response());
    let agent = VisionAgent::new(context(pool.clone()), Arc::new(fake), producer);
    agent
        .process(VisionInput {
            document_id,
            file_path: image_path,
        })
        .await
        .expect("vision process");

    let status: String = sqlx::query_scalar("SELECT status FROM documents WHERE id = $1")
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .expect("status");
    assert_eq!(status, DocumentStatus::VisionComplete.as_str());

    let supplier: Option<String> = sqlx::query_scalar(
        "SELECT parsed_value FROM extracted_fields WHERE document_id = $1 AND field_type = 'supplier_name' LIMIT 1",
    )
    .bind(document_id)
    .fetch_optional(&pool)
    .await
    .expect("supplier field");
    assert_eq!(supplier.as_deref(), Some("Acme AB"));

    let jobs = common::drain_jobs(&queue, 10).await;
    assert_eq!(jobs.len(), 1);
    assert!(matches!(jobs[0].1.job_type, JobType::Accountant));
}

#[tokio::test]
async fn accountant_agent_uses_fake_inference_and_writes_accounting_rows() {
    let pool = common::in_memory_pool().await;
    let document_id = create_document(&pool, "VISION_COMPLETE").await;
    insert_field(&pool, document_id, "supplier_name", "Acme AB").await;
    insert_field(&pool, document_id, "transaction_date", "2026-05-01").await;
    insert_field(&pool, document_id, "invoice_number", "INV-1").await;
    insert_field(&pool, document_id, "total_amount", "125.00").await;
    insert_field(&pool, document_id, "vat_amount", "25.00").await;
    insert_field(&pool, document_id, "vat_rate", "25").await;
    insert_field(&pool, document_id, "currency", "SEK").await;

    let fake = common::FakeInferenceProvider::default();
    fake.push_chat_json(fake_accounting_response());
    let agent = AccountantAgent::new(
        context(pool.clone()),
        Arc::new(fake),
        FieldLoadMode::OriginalOnly,
    );
    agent
        .process(AccountantInput { document_id })
        .await
        .expect("accountant process");

    let status: String = sqlx::query_scalar("SELECT status FROM documents WHERE id = $1")
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .expect("status");
    assert_eq!(status, DocumentStatus::AccountantReviewed.as_str());

    let invoice_id: i64 = sqlx::query_scalar("SELECT id FROM invoices WHERE document_id = $1")
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .expect("invoice id");
    let assignment_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM account_assignments WHERE invoice_id = $1")
            .bind(invoice_id)
            .fetch_one(&pool)
            .await
            .expect("assignment count");
    assert_eq!(assignment_count, 3);

    let decision_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM accounting_decisions WHERE document_id = $1")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .expect("decision count");
    assert_eq!(decision_count, 1);
}

#[tokio::test]
async fn inference_error_returns_agent_error_without_external_service() {
    let pool = common::in_memory_pool().await;
    let queue = common::test_queue();
    let document_id = create_document(&pool, "PROCESSING_VISION").await;
    let image_path = write_test_png();

    let fake = common::FakeInferenceProvider::default();
    fake.push_image_error("model unavailable");
    let agent = VisionAgent::new(context(pool), Arc::new(fake), QueueProducer::new(queue));

    let err = agent
        .process(VisionInput {
            document_id,
            file_path: image_path,
        })
        .await
        .expect_err("provider error should propagate");
    assert!(err.to_string().contains("model unavailable"));
}

#[tokio::test]
async fn export_agent_marks_ready_documents_exported() {
    let pool = common::in_memory_pool().await;
    let document_id = create_document(&pool, "EXPORT_READY").await;
    seed_validation_context(&pool, document_id).await;
    sqlx::query("INSERT INTO invoices (document_id, supplier_name, invoice_date, total_amount, vat_amount, status) VALUES ($1, 'Acme AB', '2026-05-01', 125.0, 25.0, 'PENDING_REVIEW')")
        .bind(document_id)
        .execute(&pool)
        .await
        .expect("insert invoice");

    let agent = finelor::agents::ExportAgent::new(context(pool.clone()));
    let user_id: i64 = sqlx::query_scalar("INSERT INTO users (role, email, password_hash, display_name) VALUES ('admin', 'export_test@example.com', 'hash', 'Export Test') RETURNING id")
        .fetch_one(&pool)
        .await
        .expect("insert user");
    agent
        .process(finelor::agents::ExportInput {
            user_id,
            workspace_id: None,
            date_from: None,
            date_to: None,
            document_types: None,
            confidence_min: None,
            short_refs: None,
        })
        .await
        .expect("export process");

    let row = sqlx::query("SELECT status, exported_in_batch FROM documents WHERE id = $1")
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .expect("document export row");
    let status: String = row.get("status");
    let batch_id: Option<i64> = row.try_get("exported_in_batch").ok();
    assert_eq!(status, DocumentStatus::Exported.as_str());
    assert!(batch_id.is_some());
}
