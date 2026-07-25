//! Orchestration module for coordinating agent processing

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sqlx::Row;
use tracing::{error, info, warn};

use crate::agents::{
    AccountantAgent, AccountantInput, Agent, AgentContext, IntakeAgent, ReviewAgent, ReviewInput,
    ValidatorAgent, ValidatorInput, VisionAgent, VisionInput,
};
use crate::document_state;
use crate::error::AppResult;
use crate::inference::{InferenceProvider, OllamaProvider};
use crate::queue::{Job, JobType, QueueConsumer, QueueProducer};

const JOB_BATCH_SIZE: usize = 8;
/// Orchestrator coordinates the processing pipeline
pub struct Orchestrator {
    context: AgentContext,
    queue_producer: QueueProducer,
}

impl Orchestrator {
    pub fn new(context: AgentContext, queue_producer: QueueProducer) -> Self {
        Self {
            context,
            queue_producer,
        }
    }

    /// Get an IntakeAgent instance
    pub fn get_intake_agent(&self) -> IntakeAgent {
        IntakeAgent::new(self.context.clone(), self.queue_producer.clone())
    }
}

/// Worker that processes jobs from the queue
pub struct JobProcessor {
    context: AgentContext,
    queue_producer: QueueProducer,
}

impl JobProcessor {
    pub fn new(context: AgentContext, queue_producer: QueueProducer) -> Self {
        Self {
            context,
            queue_producer,
        }
    }

    /// Start processing jobs from the queue
    pub async fn run(&self) -> AppResult<()> {
        let consumer_name = format!("job-processor-{}", uuid::Uuid::new_v4());
        let queue_consumer = QueueConsumer::new(self.queue_producer.queue(), &consumer_name);
        let inference_provider: Arc<dyn InferenceProvider> =
            Arc::new(OllamaProvider::new(&self.context.config.ollama));
        let max_job_retries = self.context.config.worker.max_job_retries;

        queue_consumer.init().await?;

        info!(
            consumer = %consumer_name,
            "Job processor started"
        );

        let mut poll_failures = 0u32;

        loop {
            match queue_consumer.poll(JOB_BATCH_SIZE).await {
                Ok(jobs) => {
                    poll_failures = 0;

                    for (entry_id, job) in jobs {
                        match self
                            .process_job(&job, &self.queue_producer, inference_provider.clone())
                            .await
                        {
                            Ok(()) => {
                                queue_consumer.acknowledge(&entry_id).await?;
                                info!(
                                    entry_id = %entry_id,
                                    job_id = %job.id,
                                    job_type = ?job.job_type,
                                    document_id = %job.document_id,
                                    "Job processed successfully"
                                );
                            }
                            Err(err) => {
                                error!(
                                    entry_id = %entry_id,
                                    job_id = %job.id,
                                    job_type = ?job.job_type,
                                    document_id = %job.document_id,
                                    retries = job.retries,
                                    error = %err,
                                    "Job processing failed"
                                );

                                if job.retries >= max_job_retries {
                                    let failed_job_type = job.job_type.clone();
                                    let _ = document_state::mark_job_failed(
                                        &self.context.pool,
                                        job.document_id,
                                        failed_job_type.clone(),
                                        Some("job retry limit reached"),
                                    )
                                    .await;
                                    let _ = self
                                        .context
                                        .record_document_event(
                                            job.document_id,
                                            "DOCUMENT_FAILED",
                                            serde_json::json!({
                                            "kind": "SYSTEM",
                                            "stage": format!("{:?}", failed_job_type).to_uppercase(),
                                            "retryable": true,
                                            "retries": job.retries,
                                        }),
                                        )
                                    .await;
                                }

                                queue_consumer
                                    .fail_and_retry(
                                        &entry_id,
                                        &job,
                                        err.to_string(),
                                        max_job_retries,
                                    )
                                    .await?;
                            }
                        }
                    }
                }
                Err(err) => {
                    poll_failures += 1;
                    let backoff = poll_backoff(poll_failures);

                    warn!(
                        failures = poll_failures,
                        backoff_ms = backoff.as_millis() as u64,
                        error = %err,
                        "Queue poll failed, backing off"
                    );

                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    pub async fn process_job_with_provider(
        &self,
        job: &Job,
        queue_producer: &QueueProducer,
        inference_provider: Arc<dyn InferenceProvider>,
    ) -> AppResult<()> {
        match job.job_type {
            JobType::Vision => {
                let agent = VisionAgent::new(
                    self.context.clone(),
                    inference_provider.clone(),
                    queue_producer.clone(),
                );

                agent
                    .process(VisionInput {
                        document_id: job.document_id,
                        file_path: PathBuf::new(),
                    })
                    .await?;
            }
            JobType::Accountant => {
                document_state::start_accounting_with_metadata(
                    &self.context.pool,
                    job.document_id,
                    Some("ollama"),
                    Some(&self.context.config.ollama.models.accountant),
                    Some(&self.context.config.ollama.accountant_prompt_path),
                )
                .await?;
                self.context
                    .record_document_event(
                        job.document_id,
                        "STATUS_CHANGED",
                        serde_json::json!({
                            "accounting_status": "ACCOUNTING",
                            "run_kind": "ACCOUNTING"
                        }),
                    )
                    .await?;

                let agent = AccountantAgent::new(
                    self.context.clone(),
                    inference_provider,
                    job.field_load_mode,
                );
                agent
                    .process(AccountantInput {
                        document_id: job.document_id,
                    })
                    .await?;

                // Accountant-complete notification intentionally skipped
                // per spam-mitigation policy (see docs/tasks/pending/...).

                queue_producer
                    .enqueue(&Job::new_with_mode(
                        JobType::Validator,
                        job.document_id,
                        job.priority,
                        job.field_load_mode,
                    ))
                    .await?;
            }
            JobType::Validator => {
                document_state::start_validation(&self.context.pool, job.document_id).await?;
                let agent = ValidatorAgent::new(self.context.clone(), job.field_load_mode);
                agent
                    .process(ValidatorInput {
                        document_id: job.document_id,
                    })
                    .await?;

                let validation_errors: Option<serde_json::Value> = sqlx::query_scalar(
                    "SELECT validation_errors FROM validation_results WHERE document_id = $1 ORDER BY checked_at DESC LIMIT 1",
                )
                .bind(job.document_id)
                .fetch_optional(&self.context.pool)
                .await?;

                if let Some(errors) = validation_errors
                    && let Some(arr) = errors.as_array()
                    && !arr.is_empty()
                {
                    info!(
                        document_id = %job.document_id,
                        "Validation found issues that need review"
                    );
                }

                queue_producer
                    .enqueue(&Job::new(JobType::Review, job.document_id, job.priority))
                    .await?;
            }
            JobType::Review => {
                let agent = ReviewAgent::new(self.context.clone(), queue_producer.clone());

                agent
                    .process(ReviewInput {
                        document_id: job.document_id,
                        force_human_review: false,
                    })
                    .await?;
            }
        }

        Ok(())
    }

    async fn process_job(
        &self,
        job: &Job,
        queue_producer: &QueueProducer,
        inference_provider: Arc<dyn InferenceProvider>,
    ) -> AppResult<()> {
        self.process_job_with_provider(job, queue_producer, inference_provider)
            .await
    }
}

pub async fn recover_incomplete_jobs(
    pool: &crate::db::DbPool,
    queue_producer: &QueueProducer,
) -> AppResult<usize> {
    let rows = sqlx::query(
        r#"
        SELECT
            d.id,
            d.priority,
            dis.status AS intake_status,
            das.status AS accounting_status,
            das.completed_at AS accounting_completed_at,
            (
                SELECT dar.run_kind
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_kind,
            (
                SELECT dar.run_status
                FROM document_accounting_runs dar
                WHERE dar.document_id = d.id
                ORDER BY dar.created_at DESC, dar.id DESC
                LIMIT 1
            ) AS latest_accounting_run_status
        FROM documents d
        JOIN document_intake_state dis ON dis.document_id = d.id
        JOIN document_accounting_state das ON das.document_id = d.id
        WHERE dis.status IN ('RECEIVED', 'PROCESSING')
           OR das.status IN ('REQUESTED', 'ACCOUNTING', 'VALIDATING', 'EXPORTING')
        ORDER BY d.received_at ASC, d.id ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut recovered = 0usize;
    for row in rows {
        let document_id: i64 = row.try_get("id")?;
        let priority: Option<String> = row.try_get("priority")?;
        let intake_status: String = row.try_get("intake_status")?;
        let accounting_status: String = row.try_get("accounting_status")?;
        let accounting_completed_at: Option<String> = row.try_get("accounting_completed_at")?;
        let latest_accounting_run_kind: Option<String> =
            row.try_get("latest_accounting_run_kind")?;
        let latest_accounting_run_status: Option<String> =
            row.try_get("latest_accounting_run_status")?;

        let Some(job_type) = recoverable_job_type(
            intake_status.as_str(),
            accounting_status.as_str(),
            accounting_completed_at.as_deref(),
            latest_accounting_run_kind.as_deref(),
            latest_accounting_run_status.as_deref(),
        ) else {
            continue;
        };

        let priority = document_priority(priority.as_deref());
        let job = Job::new(job_type, document_id, priority);
        queue_producer.enqueue(&job).await?;
        recovered += 1;

        info!(
            document_id = %document_id,
            job_id = %job.id,
            job_type = ?job.job_type,
            "Recovered incomplete document pipeline job"
        );
    }

    if recovered > 0 {
        info!(
            count = recovered,
            "Recovered incomplete document pipeline jobs"
        );
    }

    Ok(recovered)
}

fn recoverable_job_type(
    intake_status: &str,
    accounting_status: &str,
    accounting_completed_at: Option<&str>,
    latest_accounting_run_kind: Option<&str>,
    latest_accounting_run_status: Option<&str>,
) -> Option<JobType> {
    if matches!(intake_status, "RECEIVED" | "PROCESSING") {
        return Some(JobType::Vision);
    }

    if accounting_status == "REQUESTED" {
        return Some(JobType::Accountant);
    }

    if accounting_status == "ACCOUNTING" && accounting_completed_at.is_none() {
        return Some(JobType::Accountant);
    }

    if accounting_status == "VALIDATING"
        && latest_accounting_run_kind == Some("VALIDATION")
        && latest_accounting_run_status != Some("COMPLETED")
    {
        return Some(JobType::Validator);
    }

    if accounting_status == "VALIDATING" && accounting_completed_at.is_some() {
        return Some(JobType::Review);
    }

    None
}

fn document_priority(priority: Option<&str>) -> i32 {
    match priority.unwrap_or("NORMAL") {
        "URGENT" => 10,
        "HIGH" => 5,
        "LOW" => -5,
        _ => 0,
    }
}

fn poll_backoff(failures: u32) -> Duration {
    let capped = failures.min(5);
    Duration::from_secs(2u64.pow(capped).min(30))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_backoff_expected_values() {
        // failures 0 -> capped=0, 2^0=1, min(1,30)=1 -> 1s
        assert_eq!(poll_backoff(0), Duration::from_secs(1));
        // 1..=4 map to exact powers of two within the 30s cap
        assert_eq!(poll_backoff(1), Duration::from_secs(2));
        assert_eq!(poll_backoff(2), Duration::from_secs(4));
        assert_eq!(poll_backoff(3), Duration::from_secs(8));
        assert_eq!(poll_backoff(4), Duration::from_secs(16));
        // 5 is capped at 5 -> 2^5=32, min(32,30)=30
        assert_eq!(poll_backoff(5), Duration::from_secs(30));
        // >=5 are capped at 5 -> always 30
        assert_eq!(poll_backoff(6), Duration::from_secs(30));
        assert_eq!(poll_backoff(7), Duration::from_secs(30));
        assert_eq!(poll_backoff(8), Duration::from_secs(30));
        assert_eq!(poll_backoff(9), Duration::from_secs(30));
        assert_eq!(poll_backoff(10), Duration::from_secs(30));
        assert_eq!(poll_backoff(100), Duration::from_secs(30));
    }

    #[test]
    fn recoverable_statuses_map_to_resume_jobs() {
        assert!(matches!(
            recoverable_job_type("PROCESSING", "NOT_REQUESTED", None, None, None),
            Some(JobType::Vision)
        ));
        assert!(matches!(
            recoverable_job_type("RECEIVED", "NOT_REQUESTED", None, None, None),
            Some(JobType::Vision)
        ));
        assert!(recoverable_job_type("INGESTED", "NOT_REQUESTED", None, None, None).is_none());
        assert!(matches!(
            recoverable_job_type("INGESTED", "REQUESTED", None, None, None),
            Some(JobType::Accountant)
        ));
        assert!(matches!(
            recoverable_job_type(
                "INGESTED",
                "ACCOUNTING",
                None,
                Some("ACCOUNTING"),
                Some("RUNNING")
            ),
            Some(JobType::Accountant)
        ));
        assert!(matches!(
            recoverable_job_type(
                "INGESTED",
                "VALIDATING",
                Some("2026-01-01T00:00:00Z"),
                Some("VALIDATION"),
                Some("RUNNING")
            ),
            Some(JobType::Validator)
        ));
        assert!(matches!(
            recoverable_job_type(
                "INGESTED",
                "VALIDATING",
                Some("2026-01-01T00:00:00Z"),
                Some("VALIDATION"),
                Some("COMPLETED")
            ),
            Some(JobType::Review)
        ));
        assert!(recoverable_job_type("FAILED", "FAILED", None, None, None).is_none());
        assert!(
            recoverable_job_type(
                "INGESTED",
                "READY_FOR_EXPORT",
                Some("2026-01-01T00:00:00Z"),
                Some("EXPORT"),
                Some("REQUESTED")
            )
            .is_none()
        );
    }

    #[test]
    fn document_priority_maps_persisted_labels() {
        assert_eq!(document_priority(Some("URGENT")), 10);
        assert_eq!(document_priority(Some("HIGH")), 5);
        assert_eq!(document_priority(Some("LOW")), -5);
        assert_eq!(document_priority(Some("NORMAL")), 0);
        assert_eq!(document_priority(None), 0);
    }

    #[test]
    fn orchestration_has_no_channel_delivery_dependency() {
        let source = include_str!("orchestration.rs");
        let forbidden_a = ["Telegram", "Notifier"].concat();
        let forbidden_b = ["send", "_", "notification"].concat();
        let forbidden_c = ["send", "_", "review", "_", "message"].concat();
        let forbidden_d = ["document", "_", "artifacts"].concat();
        let forbidden_e = ["document", "_", "interactions"].concat();
        let forbidden_f = ["human", "_", "review", "_", "sessions"].concat();
        let forbidden_g = ["interaction", "_", "key"].concat();

        assert!(!source.contains(&forbidden_a));
        assert!(!source.contains(&forbidden_b));
        assert!(!source.contains(&forbidden_c));
        assert!(!source.contains(&forbidden_d));
        assert!(!source.contains(&forbidden_e));
        assert!(!source.contains(&forbidden_f));
        assert!(!source.contains(&forbidden_g));
    }
}
