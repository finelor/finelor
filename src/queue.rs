use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::agents::FieldLoadMode;
use crate::error::AppResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobType {
    Vision,
    Accountant,
    Validator,
    Review,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub job_type: JobType,
    pub document_id: i64,
    pub priority: i32,
    pub retries: u32,
    #[serde(default)]
    pub field_load_mode: FieldLoadMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl Job {
    pub fn new(job_type: JobType, document_id: i64, priority: i32) -> Self {
        Self::new_with_mode(job_type, document_id, priority, FieldLoadMode::OriginalOnly)
    }

    pub fn new_with_mode(
        job_type: JobType,
        document_id: i64,
        priority: i32,
        field_load_mode: FieldLoadMode,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            job_type,
            document_id,
            priority,
            retries: 0,
            field_load_mode,
            error: None,
            created_at: chrono::Utc::now(),
        }
    }
}

#[async_trait]
pub trait JobQueue: Send + Sync {
    async fn enqueue(&self, job: &Job) -> AppResult<()>;
    async fn poll(&self, consumer_name: &str, batch_size: usize) -> AppResult<Vec<(String, Job)>>;
    async fn acknowledge(&self, entry_id: &str) -> AppResult<()>;
    async fn fail_and_retry(
        &self,
        entry_id: &str,
        job: &Job,
        error: String,
        max_retries: u32,
    ) -> AppResult<()>;
    fn notify(&self) -> Arc<Notify>;
    fn poll_interval(&self) -> Duration;
}

#[derive(Clone)]
pub struct QueueProducer {
    queue: Arc<dyn JobQueue>,
}

#[derive(Clone)]
pub struct QueueConsumer {
    queue: Arc<dyn JobQueue>,
    consumer_name: String,
}

impl QueueProducer {
    pub fn new(queue: Arc<dyn JobQueue>) -> Self {
        Self { queue }
    }

    pub async fn enqueue(&self, job: &Job) -> AppResult<()> {
        self.queue.enqueue(job).await
    }

    pub fn queue(&self) -> Arc<dyn JobQueue> {
        self.queue.clone()
    }
}

impl QueueConsumer {
    pub fn new(queue: Arc<dyn JobQueue>, consumer_name: impl Into<String>) -> Self {
        Self {
            queue,
            consumer_name: consumer_name.into(),
        }
    }

    pub async fn init(&self) -> AppResult<()> {
        Ok(())
    }

    pub async fn poll(&self, batch_size: usize) -> AppResult<Vec<(String, Job)>> {
        let jobs = self.queue.poll(&self.consumer_name, batch_size).await?;
        if !jobs.is_empty() {
            return Ok(jobs);
        }

        let notify = self.queue.notify();
        tokio::select! {
            _ = notify.notified() => {}
            _ = tokio::time::sleep(self.queue.poll_interval()) => {}
        }

        self.queue.poll(&self.consumer_name, batch_size).await
    }

    pub async fn acknowledge(&self, entry_id: &str) -> AppResult<()> {
        self.queue.acknowledge(entry_id).await
    }

    pub async fn fail_and_retry(
        &self,
        entry_id: &str,
        job: &Job,
        error: String,
        max_retries: u32,
    ) -> AppResult<()> {
        self.queue
            .fail_and_retry(entry_id, job, error, max_retries)
            .await
    }

    pub fn retry_backoff(retry_number: u32) -> Duration {
        retry_backoff(retry_number)
    }
}

#[derive(Clone)]
pub struct InMemoryJobQueue {
    state: Arc<Mutex<QueueState>>,
    notify: Arc<Notify>,
    idle_poll_interval: Duration,
}

#[derive(Default)]
struct QueueState {
    ready: Vec<Job>,
    in_flight: HashMap<String, Job>,
    scheduled: Vec<ScheduledJob>,
}

#[derive(Clone)]
struct ScheduledJob {
    due_at: Instant,
    job: Job,
}

impl InMemoryJobQueue {
    pub fn new() -> Self {
        Self::with_poll_interval(Duration::from_secs(10))
    }

    pub fn with_poll_interval(idle_poll_interval: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(QueueState::default())),
            notify: Arc::new(Notify::new()),
            idle_poll_interval,
        }
    }

    fn promote_due_jobs(state: &mut QueueState) {
        let now = Instant::now();
        let mut remaining = Vec::with_capacity(state.scheduled.len());
        for scheduled in state.scheduled.drain(..) {
            if scheduled.due_at <= now {
                state.ready.push(scheduled.job);
            } else {
                remaining.push(scheduled);
            }
        }
        state.scheduled = remaining;
    }

    fn sort_ready_jobs(state: &mut QueueState) {
        state.ready.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.created_at.cmp(&right.created_at))
        });
    }
}

impl Default for InMemoryJobQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl JobQueue for InMemoryJobQueue {
    async fn enqueue(&self, job: &Job) -> AppResult<()> {
        {
            let mut state = self.state.lock().expect("queue mutex poisoned");
            state.ready.push(job.clone());
        }

        self.notify.notify_waiters();
        self.notify.notify_one();
        info!(
            job_id = %job.id,
            job_type = ?job.job_type,
            document_id = %job.document_id,
            "Job enqueued successfully"
        );
        Ok(())
    }

    async fn poll(&self, consumer_name: &str, batch_size: usize) -> AppResult<Vec<(String, Job)>> {
        let mut jobs = Vec::new();
        let mut state = self.state.lock().expect("queue mutex poisoned");
        Self::promote_due_jobs(&mut state);
        Self::sort_ready_jobs(&mut state);

        let take = batch_size.min(state.ready.len());
        let claimed: Vec<Job> = state.ready.drain(..take).collect();
        for job in claimed {
            debug!(job_id = %job.id, consumer = consumer_name, "Claimed job from in-memory queue");
            state.in_flight.insert(job.id.clone(), job.clone());
            jobs.push((job.id.clone(), job));
        }

        Ok(jobs)
    }

    async fn acknowledge(&self, entry_id: &str) -> AppResult<()> {
        let mut state = self.state.lock().expect("queue mutex poisoned");
        state.in_flight.remove(entry_id);
        debug!(entry_id = %entry_id, "Job acknowledged");
        Ok(())
    }

    async fn fail_and_retry(
        &self,
        entry_id: &str,
        job: &Job,
        error: String,
        max_retries: u32,
    ) -> AppResult<()> {
        let mut state = self.state.lock().expect("queue mutex poisoned");
        let failed_job = state
            .in_flight
            .remove(entry_id)
            .unwrap_or_else(|| job.clone());

        if failed_job.retries < max_retries {
            let next_retries = failed_job.retries + 1;
            let backoff = retry_backoff(next_retries);
            let mut retry_job = failed_job.clone();
            retry_job.id = Uuid::new_v4().to_string();
            retry_job.retries = next_retries;
            retry_job.error = Some(error);
            retry_job.created_at = chrono::Utc::now();

            state.scheduled.push(ScheduledJob {
                due_at: Instant::now() + backoff,
                job: retry_job.clone(),
            });
            drop(state);

            self.notify.notify_waiters();
            self.notify.notify_one();
            warn!(
                job_id = %retry_job.id,
                retries = retry_job.retries,
                backoff_ms = backoff.as_millis() as u64,
                document_id = %retry_job.document_id,
                "Requeued failed job"
            );
        } else {
            error!(
                job_id = %failed_job.id,
                document_id = %failed_job.document_id,
                "Job failed permanently after {} retries",
                max_retries
            );
        }

        Ok(())
    }

    fn notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    fn poll_interval(&self) -> Duration {
        let state = self.state.lock().expect("queue mutex poisoned");
        let Some(next_due) = state
            .scheduled
            .iter()
            .map(|scheduled| scheduled.due_at)
            .min()
        else {
            return self.idle_poll_interval;
        };
        next_due
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO)
            .min(self.idle_poll_interval)
    }
}

pub fn create_in_memory_queue() -> InMemoryJobQueue {
    InMemoryJobQueue::new()
}

fn retry_backoff(retry_number: u32) -> Duration {
    let capped_retry = retry_number.min(6);
    let seconds = 2u64.pow(capped_retry);
    Duration::from_secs(seconds.min(60))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_queue() -> InMemoryJobQueue {
        InMemoryJobQueue::with_poll_interval(Duration::from_millis(10))
    }

    #[test]
    fn retry_backoff_expected_values() {
        assert_eq!(QueueConsumer::retry_backoff(0), Duration::from_secs(1));
        assert_eq!(QueueConsumer::retry_backoff(1), Duration::from_secs(2));
        assert_eq!(QueueConsumer::retry_backoff(2), Duration::from_secs(4));
        assert_eq!(QueueConsumer::retry_backoff(3), Duration::from_secs(8));
        assert_eq!(QueueConsumer::retry_backoff(4), Duration::from_secs(16));
        assert_eq!(QueueConsumer::retry_backoff(5), Duration::from_secs(32));
        assert_eq!(QueueConsumer::retry_backoff(6), Duration::from_secs(60));
        assert_eq!(QueueConsumer::retry_backoff(7), Duration::from_secs(60));
        assert_eq!(QueueConsumer::retry_backoff(100), Duration::from_secs(60));
    }

    #[test]
    fn job_serde_roundtrip() {
        let original = Job {
            id: "job-id-1".to_string(),
            job_type: JobType::Accountant,
            document_id: 42,
            priority: 7,
            retries: 3,
            field_load_mode: crate::agents::FieldLoadMode::EffectiveWithCorrections,
            error: Some("timeout".to_string()),
            created_at: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&original).expect("serialize job");
        let decoded: Job = serde_json::from_str(&json).expect("deserialize job");

        assert_eq!(decoded.id, original.id);
        assert_eq!(
            format!("{:?}", decoded.job_type),
            format!("{:?}", original.job_type)
        );
        assert_eq!(decoded.document_id, original.document_id);
        assert_eq!(decoded.priority, original.priority);
        assert_eq!(decoded.retries, original.retries);
        assert_eq!(decoded.error, original.error);
        assert_eq!(
            format!("{:?}", decoded.field_load_mode),
            format!("{:?}", original.field_load_mode)
        );
        let delta = (decoded.created_at - original.created_at).num_milliseconds();
        assert_eq!(delta, 0);
    }

    #[test]
    fn job_serde_roundtrip_with_defaults() {
        let original = Job {
            id: "job-id-2".to_string(),
            job_type: JobType::Review,
            document_id: 43,
            priority: 0,
            retries: 0,
            field_load_mode: crate::agents::FieldLoadMode::OriginalOnly,
            error: None,
            created_at: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&original).expect("serialize job");
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value.as_object_mut().unwrap().remove("field_load_mode");
        let decoded: Job = serde_json::from_value(value).expect("deserialize job");
        assert_eq!(
            decoded.field_load_mode,
            crate::agents::FieldLoadMode::OriginalOnly
        );
    }

    #[tokio::test]
    async fn in_memory_queue_claims_highest_priority_first() {
        let queue = test_queue();
        let low = Job::new(JobType::Vision, 44, 1);
        let high = Job::new(JobType::Review, 44, 10);
        queue.enqueue(&low).await.unwrap();
        queue.enqueue(&high).await.unwrap();

        let jobs = queue.poll("worker", 1).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].1.id, high.id);
    }

    #[tokio::test]
    async fn in_memory_queue_does_not_double_claim_running_job() {
        let queue = test_queue();
        let job = Job::new(JobType::Vision, 44, 1);
        queue.enqueue(&job).await.unwrap();

        let first = queue.poll("worker-1", 1).await.unwrap();
        let second = queue.poll("worker-2", 1).await.unwrap();
        assert_eq!(first.len(), 1);
        assert!(second.is_empty());
    }

    #[tokio::test]
    async fn in_memory_queue_retries_as_new_delayed_job() {
        let queue = InMemoryJobQueue::with_poll_interval(Duration::from_millis(1));
        let job = Job::new(JobType::Vision, 44, 1);
        queue.enqueue(&job).await.unwrap();
        let claimed = queue.poll("worker", 1).await.unwrap();
        queue
            .fail_and_retry(&claimed[0].0, &claimed[0].1, "timeout".to_string(), 3)
            .await
            .unwrap();

        assert!(queue.poll("worker", 1).await.unwrap().is_empty());
        tokio::time::sleep(Duration::from_secs(2)).await;
        let retry = queue.poll("worker", 1).await.unwrap();
        assert_eq!(retry.len(), 1);
        assert_ne!(retry[0].1.id, job.id);
        assert_eq!(retry[0].1.retries, 1);
        assert_eq!(retry[0].1.error.as_deref(), Some("timeout"));
    }

    #[tokio::test]
    async fn in_memory_queue_permanent_failure_does_not_requeue() {
        let queue = test_queue();
        let job = Job::new(JobType::Vision, 44, 1);
        queue.enqueue(&job).await.unwrap();
        let claimed = queue.poll("worker", 1).await.unwrap();
        queue
            .fail_and_retry(&claimed[0].0, &claimed[0].1, "timeout".to_string(), 0)
            .await
            .unwrap();

        assert!(queue.poll("worker", 1).await.unwrap().is_empty());
    }
}
