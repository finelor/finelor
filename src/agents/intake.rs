use std::path::PathBuf;

use tracing::{error, info, warn};

use crate::agents::{Agent, AgentContext, DocumentStatus};
use crate::error::{AppError, AppResult};
use crate::queue::{Job, JobType, QueueProducer};

/// Input for the IntakeAgent
#[derive(Debug)]
pub struct IntakeInput {
    pub document_id: i64,
    pub file_path: PathBuf,
    pub mime_type: String,
    pub filename: String,
}

/// Output from the IntakeAgent
#[derive(Debug)]
pub struct IntakeOutput {
    pub document_id: i64,
    pub document_type: String,
    pub queued_for_vision: bool,
}

/// IntakeAgent processes new documents and classifies them
pub struct IntakeAgent {
    context: AgentContext,
    queue_producer: QueueProducer,
}

/// Classification result
#[derive(Debug)]
struct DocumentClassification {
    document_type: String,
    priority: i32,
}

impl IntakeAgent {
    pub fn new(context: AgentContext, queue_producer: QueueProducer) -> Self {
        Self {
            context,
            queue_producer,
        }
    }

    /// Classify the document based on file type and content
    fn classify(&self, _input: &IntakeInput) -> DocumentClassification {
        // For MVP, classify based on mime type
        // In production, this could analyze content
        DocumentClassification {
            document_type: "INVOICE".to_string(),
            priority: 0,
        }
    }

    /// Check if the document has already been processed (duplicate detection)
    async fn check_duplicate(&self, document_id: i64) -> AppResult<bool> {
        let record: Option<String> = sqlx::query_scalar(
            r#"
            SELECT status FROM documents WHERE id = $1
            "#,
        )
        .bind(document_id)
        .fetch_optional(&self.context.pool)
        .await?;

        // If document exists and is past RECEIVED state, it's being processed
        if let Some(status) = record {
            return Ok(status != "RECEIVED");
        }

        Ok(false)
    }
}

#[async_trait::async_trait]
impl Agent for IntakeAgent {
    type Input = IntakeInput;
    type Output = IntakeOutput;
    type Error = AppError;

    fn name(&self) -> &'static str {
        "IntakeAgent"
    }

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        info!(
            document_id = %input.document_id,
            filename = %input.filename,
            mime_type = %input.mime_type,
            "Starting document intake"
        );

        // Check for duplicates
        match self.check_duplicate(input.document_id).await {
            Ok(true) => {
                warn!(document_id = %input.document_id, "Document already being processed");
                return Ok(IntakeOutput {
                    document_id: input.document_id,
                    document_type: "INVOICE".to_string(),
                    queued_for_vision: false,
                });
            }
            Ok(false) => {}
            Err(e) => {
                error!(error = %e, "Failed to check duplicate status");
                return Err(e);
            }
        }

        // Classify the document
        let classification = self.classify(&input);
        info!(
            document_id = %input.document_id,
            document_type = %classification.document_type,
            priority = classification.priority,
            "Document classified"
        );

        // Update document type in database
        sqlx::query(
            r#"
            UPDATE documents 
            SET document_type = $1, priority = 'NORMAL', updated_at = CURRENT_TIMESTAMP
            WHERE id = $2
            "#,
        )
        .bind(&classification.document_type)
        .bind(input.document_id)
        .execute(&self.context.pool)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to update document type");
            AppError::Database(e)
        })?;

        // Update status to PROCESSING_VISION
        self.context
            .update_document_status(input.document_id, DocumentStatus::ProcessingVision)
            .await?;

        // Queue job for VisionAgent
        let job = Job::new(JobType::Vision, input.document_id, classification.priority);

        self.queue_producer.enqueue(&job).await.map_err(|e| {
            error!(error = %e, "Failed to queue vision job");
            AppError::Queue(format!("Failed to enqueue: {}", e))
        })?;

        info!(
            document_id = %input.document_id,
            job_id = %job.id,
            "Document intake complete, queued for vision processing"
        );
        self.context
            .record_document_event(
                input.document_id,
                "INTAKE_QUEUED",
                serde_json::json!({
                    "job_id": job.id,
                    "document_type": classification.document_type,
                }),
            )
            .await?;

        Ok(IntakeOutput {
            document_id: input.document_id,
            document_type: classification.document_type,
            queued_for_vision: true,
        })
    }
}
