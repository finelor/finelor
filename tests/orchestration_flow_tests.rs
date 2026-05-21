//! Integration tests for queue-to-agent orchestration flow with fake inference.

mod common;

use std::sync::Arc;

use finelor::agents::{AgentContext, DocumentStatus};
use finelor::orchestration::JobProcessor;
use finelor::queue::{Job, JobType, QueueConsumer, QueueProducer};
use finelor::web::events::AppEventBus;
use serde_json::json;
use sqlx::{Row, SqlitePool};

fn context(pool: SqlitePool) -> AgentContext {
    AgentContext::new(pool, common::test_config(), AppEventBus::new(16))
}

async fn create_document(pool: &SqlitePool, status: &str, original_path: Option<&str>) -> i64 {
    let hash = format!("orchestration-flow-{}", uuid::Uuid::new_v4());
    sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO documents (filename, status, file_hash, original_path, mime_type)
        VALUES ('flow.png', $1, $2, $3, 'image/png')
        RETURNING id
        "#,
    )
    .bind(status)
    .bind(hash)
    .bind(original_path.unwrap_or(""))
    .fetch_one(pool)
    .await
    .expect("insert document")
}

fn write_test_png() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("finelor-flow-{}.png", uuid::Uuid::new_v4()));
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
) -> Job {
    let consumer = QueueConsumer::new(queue.clone(), "flow-worker");
    let mut jobs = consumer.poll(1).await.expect("poll job");
    assert_eq!(jobs.len(), 1, "expected exactly one queued job");
    let (entry_id, job) = jobs.pop().unwrap();
    let processor = JobProcessor::new(context(pool.clone()), producer.clone());
    processor
        .process_job_with_provider(&job, producer, fake)
        .await
        .expect("process job");
    consumer.acknowledge(&entry_id).await.expect("ack job");
    job
}

#[tokio::test]
async fn queued_pipeline_runs_vision_accountant_validator_and_review_boundaries() {
    let pool = common::in_memory_pool().await;
    let queue = common::test_queue();
    let producer = QueueProducer::new(queue.clone());
    let fake = Arc::new(common::FakeInferenceProvider::default());
    fake.push_image_json(vision_response());
    fake.push_chat_json(accounting_response());

    let image_path = write_test_png();
    let document_id = create_document(
        &pool,
        DocumentStatus::ProcessingVision.as_str(),
        Some(&image_path.to_string_lossy()),
    )
    .await;
    producer
        .enqueue(&Job::new(JobType::Vision, document_id, 0))
        .await
        .expect("enqueue vision");

    let vision_job = run_next_job(&pool, &queue, &producer, fake.clone()).await;
    assert!(matches!(vision_job.job_type, JobType::Vision));
    let status: String = sqlx::query_scalar("SELECT status FROM documents WHERE id = $1")
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .expect("vision status");
    assert_eq!(status, DocumentStatus::VisionComplete.as_str());

    let accountant_job = run_next_job(&pool, &queue, &producer, fake.clone()).await;
    assert!(matches!(accountant_job.job_type, JobType::Accountant));
    let invoice_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM invoices WHERE document_id = $1")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .expect("invoice count");
    assert_eq!(invoice_count, 1);

    let validator_job = run_next_job(&pool, &queue, &producer, fake.clone()).await;
    assert!(matches!(validator_job.job_type, JobType::Validator));
    let validation_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM validation_results WHERE document_id = $1")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .expect("validation count");
    assert_eq!(validation_count, 1);

    let review_job = run_next_job(&pool, &queue, &producer, fake.clone()).await;
    assert!(matches!(review_job.job_type, JobType::Review));
    let final_status: String = sqlx::query_scalar("SELECT status FROM documents WHERE id = $1")
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .expect("final status");
    assert!(matches!(
        final_status.as_str(),
        "EXPORT_READY" | "PENDING_HUMAN_REVIEW"
    ));
}

#[tokio::test]
async fn failed_queued_job_can_be_retried_and_then_processed() {
    let pool = common::in_memory_pool().await;
    let queue = common::test_queue();
    let producer = QueueProducer::new(queue.clone());
    let fake = Arc::new(common::FakeInferenceProvider::default());
    fake.push_image_error("model temporarily unavailable");
    fake.push_image_json(vision_response());

    let image_path = write_test_png();
    let document_id = create_document(
        &pool,
        DocumentStatus::ProcessingVision.as_str(),
        Some(&image_path.to_string_lossy()),
    )
    .await;
    producer
        .enqueue(&Job::new(JobType::Vision, document_id, 0))
        .await
        .expect("enqueue vision");

    let consumer = QueueConsumer::new(queue.clone(), "retry-worker");
    let mut jobs = consumer.poll(1).await.expect("poll first job");
    let (entry_id, job) = jobs.pop().expect("first job");
    let processor = JobProcessor::new(context(pool.clone()), producer.clone());
    let err = processor
        .process_job_with_provider(&job, &producer, fake.clone())
        .await
        .expect_err("first provider response should fail");
    consumer
        .fail_and_retry(&entry_id, &job, err.to_string(), 1)
        .await
        .expect("retry job");

    tokio::time::sleep(QueueConsumer::retry_backoff(1)).await;
    let mut retry_jobs = consumer.poll(1).await.expect("poll retry job");
    let (retry_entry_id, retry_job) = retry_jobs.pop().expect("retry job");
    assert_eq!(retry_job.retries, 1);
    assert!(
        retry_job
            .error
            .as_deref()
            .is_some_and(|e| e.contains("model temporarily unavailable"))
    );

    processor
        .process_job_with_provider(&retry_job, &producer, fake.clone())
        .await
        .expect("retry should process");
    consumer
        .acknowledge(&retry_entry_id)
        .await
        .expect("ack retry");

    let row = sqlx::query("SELECT status FROM documents WHERE id = $1")
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .expect("document status");
    let status: String = row.get("status");
    assert_eq!(status, DocumentStatus::VisionComplete.as_str());
}
