//! File-backed SQLite smoke tests for filesystem/WAL/locking coverage.

mod common;

use std::sync::Arc;

use common::TestDocumentStatePreset as Preset;
use finelor::agents::AgentContext;
use finelor::orchestration::JobProcessor;
use finelor::queue::{Job, JobType, QueueConsumer, QueueProducer};
use finelor::web::events::AppEventBus;
use serde_json::json;
use sqlx::SqlitePool;

fn context(pool: SqlitePool) -> AgentContext {
    AgentContext::new(pool, common::test_config(), AppEventBus::new(16))
}

async fn create_document(pool: &SqlitePool, preset: Preset, original_path: Option<&str>) -> i64 {
    common::create_document_with_state_preset(pool, preset, original_path, "image/png")
        .await
        .0
}

fn write_test_png() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("finelor-filedb-{}.png", uuid::Uuid::new_v4()));
    let image = image::RgbImage::from_pixel(2, 2, image::Rgb([255, 255, 255]));
    image.save(&path).expect("write test png");
    path
}

fn vision_response() -> serde_json::Value {
    json!({
        "document_type": "receipt",
        "confidence": 0.92,
        "extracted_text": "Acme AB receipt",
        "supplier": { "name": "Acme AB", "org_nr": "5560160680" },
        "transaction": { "date": "2026-05-01", "invoice_number": "INV-1" },
        "amounts": { "total_incl_vat": "125.00", "vat_amount": "25.00", "vat_rate": "25", "currency": "SEK" },
        "line_items": [{ "description": "Lunch", "amount": "125.00", "quantity": 1 }],
        "quality_issues": [],
        "missing_info": []
    })
}

fn accounting_response() -> serde_json::Value {
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

async fn run_next_job(
    pool: &SqlitePool,
    queue: &Arc<finelor::queue::InMemoryJobQueue>,
    producer: &QueueProducer,
    fake: Arc<common::FakeInferenceProvider>,
) {
    let consumer = QueueConsumer::new(queue.clone(), "filedb-worker");
    let mut jobs = consumer.poll(1).await.expect("poll job");
    assert_eq!(jobs.len(), 1, "expected exactly one queued job");
    let (entry_id, job) = jobs.pop().expect("queued job");
    let processor = JobProcessor::new(context(pool.clone()), producer.clone());
    processor
        .process_job_with_provider(&job, producer, fake)
        .await
        .expect("process job");
    consumer.acknowledge(&entry_id).await.expect("ack job");
}

#[tokio::test]
async fn filedb_migration_bootstrap_creates_core_tables() {
    let db = common::FileBackedDb::new().await;
    let pool = db.pool();
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'documents'",
    )
    .fetch_one(&pool)
    .await
    .expect("documents table check");
    assert_eq!(exists, 1);
    db.cleanup().await;
}

#[tokio::test]
async fn filedb_pipeline_smoke_runs_to_validation_boundary() {
    let db = common::FileBackedDb::new().await;
    let pool = db.pool();
    let queue = common::test_queue();
    let producer = QueueProducer::new(queue.clone());
    let fake = Arc::new(common::FakeInferenceProvider::default());
    fake.push_image_json(vision_response());
    fake.push_chat_json(accounting_response());

    let image_path = write_test_png();
    let document_id = create_document(
        &pool,
        Preset::IntakeProcessing,
        Some(&image_path.to_string_lossy()),
    )
    .await;
    producer
        .enqueue(&Job::new(JobType::Vision, document_id, 0))
        .await
        .expect("enqueue vision");

    run_next_job(&pool, &queue, &producer, fake.clone()).await;

    let pending_after_vision = QueueConsumer::new(queue.clone(), "filedb-peek")
        .poll(1)
        .await
        .expect("poll after vision");
    assert!(
        pending_after_vision.is_empty(),
        "vision completion should not automatically enqueue accountant"
    );

    finelor::document_state::request_accounting(&pool, document_id)
        .await
        .expect("mark accounting requested");
    producer
        .enqueue(&Job::new(JobType::Accountant, document_id, 0))
        .await
        .expect("enqueue accountant");

    run_next_job(&pool, &queue, &producer, fake.clone()).await;
    run_next_job(&pool, &queue, &producer, fake.clone()).await;

    let validation_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM validation_results WHERE document_id = $1")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .expect("validation count");
    assert_eq!(validation_count, 1);

    db.cleanup().await;
}

#[tokio::test]
async fn filedb_persists_data_across_pool_reopen() {
    let db = common::FileBackedDb::new().await;
    let first_pool = db.pool();
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO users (email, password_hash, role)
        VALUES ('filedb@example.com', 'hash', 'admin')
        RETURNING id
        "#,
    )
    .fetch_one(&first_pool)
    .await
    .expect("insert user");
    drop(first_pool);

    let reopened = db.reopen_pool().await;
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&reopened)
        .await
        .expect("read persisted user");
    assert_eq!(exists, 1);
    reopened.close().await;
    db.cleanup().await;
}
