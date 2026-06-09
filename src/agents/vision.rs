use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::prelude::*;
use image::{self, GenericImageView, ImageFormat};
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::Row;
use tracing::{info, warn};

use crate::agents::{Agent, AgentContext, DocumentStatus, db_helpers};
use crate::error::{AppError, AppResult};
use crate::inference::{ImageJsonRequest, InferenceProvider, extract_json_from_response};
use crate::queue::QueueProducer;

/// Input for the VisionAgent
#[derive(Debug, Clone)]
pub struct VisionInput {
    pub document_id: i64,
    pub file_path: PathBuf,
}

/// Output from the VisionAgent
#[derive(Debug)]
pub struct VisionOutput {
    pub document_id: i64,
    pub extracted_fields: ExtractedFields,
    pub raw_response: String,
}

/// Structured data extracted from the image
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ExtractedFields {
    #[serde(default = "default_document_type")]
    pub document_type: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub extracted_text: String,
    #[serde(default)]
    pub supplier: SupplierInfo,
    #[serde(default)]
    pub transaction: TransactionInfo,
    #[serde(default)]
    pub amounts: AmountsInfo,
    #[serde(default)]
    pub line_items: Vec<LineItem>,
    #[serde(default)]
    pub quality_issues: Vec<String>,
    #[serde(default)]
    pub missing_info: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct SupplierInfo {
    pub name: Option<String>,
    pub org_nr: Option<String>,
    pub address: Option<String>,
    pub vat_number: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct TransactionInfo {
    pub date: Option<String>,
    pub invoice_number: Option<String>,
    pub ocr_number: Option<String>,
    pub payment_method: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct AmountsInfo {
    pub total_incl_vat: Option<String>,
    pub vat_amount: Option<String>,
    pub vat_rate: Option<String>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineItem {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub amount: String,
    #[serde(default, deserialize_with = "deserialize_quantity")]
    pub quantity: i32,
}

fn default_document_type() -> String {
    "unknown".to_string()
}

fn deserialize_quantity<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(number) => {
            if let Some(quantity) = number.as_i64() {
                return Ok(quantity.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
            }

            if let Some(quantity) = number.as_f64()
                && quantity.is_finite()
            {
                return Ok(quantity.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32);
            }

            Ok(0)
        }
        serde_json::Value::String(value) => {
            let value = value.trim().replace(',', ".");
            if value.is_empty() {
                return Ok(0);
            }

            value
                .parse::<f64>()
                .map(|quantity| quantity.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
                .map_err(serde::de::Error::custom)
        }
        serde_json::Value::Null => Ok(0),
        other => Err(serde::de::Error::custom(format!(
            "expected numeric quantity, got {other}"
        ))),
    }
}

/// VisionAgent processes images and extracts structured text using Ollama
pub struct VisionAgent {
    context: AgentContext,
    inference_provider: Arc<dyn InferenceProvider>,
    prompt: String,
    model: String,
}

impl VisionAgent {
    pub fn new(
        context: AgentContext,
        inference_provider: Arc<dyn InferenceProvider>,
        _queue_producer: QueueProducer,
    ) -> Self {
        let model = context.config.ollama.models.vision.clone();
        let prompt_path = context.config.ollama.vision_prompt_path.clone();
        let prompt = std::fs::read_to_string(&prompt_path).unwrap_or_else(|err| {
            warn!(
                prompt_path = %prompt_path,
                error = %err,
                "Could not read configured vision prompt, using bundled fallback"
            );
            include_str!("../../assets/prompts/sweden/vision_agent.md").to_string()
        });

        Self {
            context,
            inference_provider,
            prompt,
            model,
        }
    }

    /// Read and process the image file
    async fn load_and_resize_image(&self, file_path: &Path) -> AppResult<Vec<u8>> {
        // Use spawn_blocking for file operations
        let path = file_path.to_path_buf();
        let image_data = tokio::task::spawn_blocking(move || {
            let img = image::open(&path)
                .map_err(|e| AppError::ImageProcessing(format!("Failed to open image: {}", e)))?;

            // Resize if too large (max 1024px longest side)
            let (width, height) = img.dimensions();
            let max_dim = 1024u32;

            let resized = if width > max_dim || height > max_dim {
                let scale = if width > height {
                    max_dim as f32 / width as f32
                } else {
                    max_dim as f32 / height as f32
                };
                let new_width = (width as f32 * scale) as u32;
                let new_height = (height as f32 * scale) as u32;
                img.resize(new_width, new_height, image::imageops::FilterType::Lanczos3)
            } else {
                img
            };

            // Convert to JPEG bytes
            let mut buffer = Vec::new();
            let mut cursor = std::io::Cursor::new(&mut buffer);
            resized
                .write_to(&mut cursor, ImageFormat::Jpeg)
                .map_err(|e| AppError::ImageProcessing(format!("Failed to encode image: {}", e)))?;

            Ok::<_, AppError>(buffer)
        })
        .await
        .map_err(|e| AppError::ImageProcessing(format!("Task failed: {}", e)))??;

        Ok(image_data)
    }

    /// Convert PDF to image if needed (for PDF files)
    /// Note: For MVP, we assume images. PDF support can be added later.
    // TODO: used by future PDF support
    #[allow(dead_code)]
    fn convert_if_pdf(&self, file_path: &Path, mime_type: &str) -> AppResult<PathBuf> {
        if mime_type == "application/pdf" {
            // For MVP, return error - PDF conversion not implemented
            // In production, use poppler or similar to convert PDF to image
            Err(AppError::ImageProcessing(
                "PDF conversion not implemented in MVP".to_string(),
            ))
        } else {
            Ok(file_path.to_path_buf())
        }
    }

    /// Get file path from database
    async fn get_file_path(&self, document_id: i64) -> AppResult<PathBuf> {
        let row = sqlx::query("SELECT original_path, mime_type FROM documents WHERE id = $1")
            .bind(document_id)
            .fetch_one(&self.context.pool)
            .await?;

        let path_str: String = row.try_get("original_path")?;
        let _mime_type: String = row.try_get("mime_type")?;

        Ok(PathBuf::from(path_str))
    }

    /// Store extracted fields in database
    async fn store_extracted_fields(
        &self,
        document_id: i64,
        fields: &ExtractedFields,
    ) -> AppResult<()> {
        // Store document-wide fields
        let confidence = (fields.confidence * 100.0) as i32;

        // Store supplier name
        if let Some(supplier_name) = &fields.supplier.name {
            sqlx::query(
                r#"
                INSERT INTO extracted_fields 
                (document_id, field_type, raw_value, parsed_value, confidence, source)
                VALUES ($1, 'supplier_name', $2, $2, $3, 'vision')
                "#,
            )
            .bind(document_id)
            .bind(supplier_name)
            .bind(confidence as f64 / 100.0)
            .execute(&self.context.pool)
            .await?;
        }

        // Store supplier org nr
        if let Some(org_nr) = &fields.supplier.org_nr {
            sqlx::query(
                r#"
                INSERT INTO extracted_fields 
                (document_id, field_type, raw_value, parsed_value, parsed_type, confidence, source)
                VALUES ($1, 'supplier_org_nr', $2, $2, 'org_number', $3, 'vision')
                "#,
            )
            .bind(document_id)
            .bind(org_nr)
            .bind(confidence as f64 / 100.0)
            .execute(&self.context.pool)
            .await?;
        }

        // Store transaction date
        if let Some(date) = &fields.transaction.date {
            sqlx::query(
                r#"
                INSERT INTO extracted_fields 
                (document_id, field_type, raw_value, parsed_value, parsed_type, confidence, source)
                VALUES ($1, 'transaction_date', $2, $2, 'date', $3, 'vision')
                "#,
            )
            .bind(document_id)
            .bind(date)
            .bind(confidence as f64 / 100.0)
            .execute(&self.context.pool)
            .await?;
        }

        // Store invoice number
        if let Some(inv_no) = &fields.transaction.invoice_number {
            sqlx::query(
                r#"
                INSERT INTO extracted_fields 
                (document_id, field_type, raw_value, parsed_value, confidence, source)
                VALUES ($1, 'invoice_number', $2, $2, $3, 'vision')
                "#,
            )
            .bind(document_id)
            .bind(inv_no)
            .bind(confidence as f64 / 100.0)
            .execute(&self.context.pool)
            .await?;
        }

        // Store OCR number
        if let Some(ocr) = &fields.transaction.ocr_number {
            sqlx::query(
                r#"
                INSERT INTO extracted_fields 
                (document_id, field_type, raw_value, parsed_value, confidence, source)
                VALUES ($1, 'ocr_number', $2, $2, $3, 'vision')
                "#,
            )
            .bind(document_id)
            .bind(ocr)
            .bind(confidence as f64 / 100.0)
            .execute(&self.context.pool)
            .await?;
        }

        // Store total amount
        if let Some(total) = &fields.amounts.total_incl_vat {
            sqlx::query(
                r#"
                INSERT INTO extracted_fields 
                (document_id, field_type, raw_value, parsed_value, parsed_type, confidence, source)
                VALUES ($1, 'total_amount', $2, $2, 'money', $3, 'vision')
                "#,
            )
            .bind(document_id)
            .bind(total)
            .bind(confidence as f64 / 100.0)
            .execute(&self.context.pool)
            .await?;
        }

        // Store VAT amount
        if let Some(vat) = &fields.amounts.vat_amount {
            sqlx::query(
                r#"
                INSERT INTO extracted_fields 
                (document_id, field_type, raw_value, parsed_value, parsed_type, confidence, source)
                VALUES ($1, 'vat_amount', $2, $2, 'money', $3, 'vision')
                "#,
            )
            .bind(document_id)
            .bind(vat)
            .bind(confidence as f64 / 100.0)
            .execute(&self.context.pool)
            .await?;
        }

        // Store VAT rate
        if let Some(vat_rate) = &fields.amounts.vat_rate {
            sqlx::query(
                r#"
                INSERT INTO extracted_fields 
                (document_id, field_type, raw_value, parsed_value, parsed_type, confidence, source)
                VALUES ($1, 'vat_rate', $2, $2, 'percentage', $3, 'vision')
                "#,
            )
            .bind(document_id)
            .bind(vat_rate)
            .bind(confidence as f64 / 100.0)
            .execute(&self.context.pool)
            .await?;
        }

        // Store currency
        if let Some(currency) = &fields.amounts.currency {
            sqlx::query(
                r#"
                INSERT INTO extracted_fields 
                (document_id, field_type, raw_value, parsed_value, confidence, source)
                VALUES ($1, 'currency', $2, $2, $3, 'vision')
                "#,
            )
            .bind(document_id)
            .bind(currency)
            .bind(confidence as f64 / 100.0)
            .execute(&self.context.pool)
            .await?;
        }

        info!(document_id = %document_id, "Stored extracted fields");
        Ok(())
    }
}

#[async_trait::async_trait]
impl Agent for VisionAgent {
    type Input = VisionInput;
    type Output = VisionOutput;
    type Error = AppError;

    fn name(&self) -> &'static str {
        "VisionAgent"
    }

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        info!(
            document_id = %input.document_id,
            file_path = %input.file_path.display(),
            "Starting vision extraction"
        );

        // Update status and timestamps
        self.context
            .update_document_status(input.document_id, DocumentStatus::ProcessingVision)
            .await?;

        db_helpers::update_vision_timestamps(&self.context.pool, input.document_id, true, false)
            .await?;

        // Get file path from DB if not provided
        let file_path = if input.file_path.as_os_str().is_empty() {
            self.get_file_path(input.document_id).await?
        } else {
            input.file_path
        };

        // Check file exists
        if !file_path.exists() {
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("File not found: {}", file_path.display()),
            )));
        }

        // Load and resize image
        let image_data = self.load_and_resize_image(&file_path).await?;
        let base64_image = BASE64_STANDARD.encode(&image_data);
        info!(
            document_id = %input.document_id,
            image_size_kb = image_data.len() / 1024,
            "Image loaded and resized"
        );

        // Call Ollama API
        info!(
            document_id = %input.document_id,
            model = %self.model,
            "Calling Ollama vision model"
        );

        let response = self
            .inference_provider
            .generate_image_json(ImageJsonRequest {
                model: self.model.clone(),
                prompt: self.prompt.clone(),
                base64_image,
            })
            .await?;

        info!(
            document_id = %input.document_id,
            response_size = response.response.len(),
            eval_count = response.eval_count,
            "Ollama response received"
        );

        // Parse JSON response
        let json = extract_json_from_response(&response.response)?;

        // Deserialize into ExtractedFields
        let extracted_fields: ExtractedFields = serde_json::from_value(json).map_err(|e| {
            warn!(error = %e, "Failed to parse extracted fields, using partial data");
            AppError::Serialization(format!("Parse error: {}", e))
        })?;

        info!(
            document_id = %input.document_id,
            document_type = %extracted_fields.document_type,
            confidence = extracted_fields.confidence,
            supplier_name = ?extracted_fields.supplier.name,
            total = ?extracted_fields.amounts.total_incl_vat,
            "Extraction complete"
        );

        // Store extracted fields
        self.store_extracted_fields(input.document_id, &extracted_fields)
            .await?;

        // Update status
        self.context
            .update_document_status(input.document_id, DocumentStatus::VisionComplete)
            .await?;

        db_helpers::update_vision_timestamps(&self.context.pool, input.document_id, false, true)
            .await?;

        self.context
            .record_document_event(
                input.document_id,
                "VISION_COMPLETED",
                serde_json::json!({
                    "confidence": extracted_fields.confidence,
                    "document_type": extracted_fields.document_type,
                }),
            )
            .await?;

        Ok(VisionOutput {
            document_id: input.document_id,
            extracted_fields,
            raw_response: response.response,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracted_fields_accept_decimal_line_item_quantity() {
        let fields: ExtractedFields = serde_json::from_value(serde_json::json!({
            "document_type": "receipt",
            "confidence": 0.63,
            "extracted_text": "Receipt text",
            "supplier": { "name": "Test Supplier" },
            "transaction": {},
            "amounts": { "total_incl_vat": "123.00", "currency": "SEK" },
            "line_items": [
                { "description": "Coffee", "amount": "123.00", "quantity": 0.63 }
            ],
            "quality_issues": [],
            "missing_info": []
        }))
        .expect("vision output should tolerate decimal quantity");

        assert_eq!(fields.confidence, 0.63);
        assert_eq!(fields.line_items[0].quantity, 1);
    }

    #[test]
    fn extracted_fields_default_missing_optional_sections() {
        let fields: ExtractedFields = serde_json::from_value(serde_json::json!({
            "confidence": 0.5,
            "line_items": [
                { "quantity": "2,0" },
                { "quantity": null }
            ]
        }))
        .expect("vision output should tolerate sparse model JSON");

        assert_eq!(fields.document_type, "unknown");
        assert_eq!(fields.line_items[0].quantity, 2);
        assert_eq!(fields.line_items[1].quantity, 0);
        assert!(fields.supplier.name.is_none());
    }

    #[test]
    fn line_item_quantity_accepts_integer_float_string_and_null() {
        let fields: ExtractedFields = serde_json::from_value(serde_json::json!({
            "line_items": [
                { "description": "A", "quantity": 3 },
                { "description": "B", "quantity": 2.49 },
                { "description": "C", "quantity": "2,50" },
                { "description": "D", "quantity": null },
                { "description": "E" }
            ]
        }))
        .expect("valid quantity variants");

        let quantities = fields
            .line_items
            .iter()
            .map(|item| item.quantity)
            .collect::<Vec<_>>();
        assert_eq!(quantities, vec![3, 2, 3, 0, 0]);
    }

    #[test]
    fn line_item_quantity_rejects_non_numeric_strings_and_objects() {
        let string_err = serde_json::from_value::<ExtractedFields>(serde_json::json!({
            "line_items": [{ "quantity": "many" }]
        }))
        .expect_err("non-numeric string should fail");
        assert!(string_err.to_string().contains("invalid float literal"));

        let object_err = serde_json::from_value::<ExtractedFields>(serde_json::json!({
            "line_items": [{ "quantity": { "value": 1 } }]
        }))
        .expect_err("object quantity should fail");
        assert!(object_err.to_string().contains("expected numeric quantity"));
    }
}
