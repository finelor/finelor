//! Export Agent for Finelor
//!
//! Queries ready documents, generates SIE4 export bundles, and marks documents as exported.

use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite, Transaction as SqlxTransaction};
use std::collections::HashMap;
use tracing::{info, warn};
use uuid::Uuid;

use crate::agents::{Agent, AgentContext};
use crate::db::DbPool;
use crate::error::{AppError, AppResult};
use crate::export::{
    Sie4Config, Transaction as Sie4Transaction, Verification, generate_sie4_export,
};

/// Input for the ExportAgent
#[derive(Debug, Clone, Serialize)]
pub struct ExportInput {
    pub user_id: i64,
    #[serde(alias = "workspace_id")]
    pub workspace_id: Option<Uuid>,
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
    pub document_types: Option<Vec<String>>,
    pub confidence_min: Option<f64>,
    pub short_refs: Option<Vec<String>>,
}

/// Output from the ExportAgent
#[derive(Debug, Clone)]
pub struct ExportOutput {
    pub batch_id: i64,
    pub document_count: usize,
    pub zip_path: String,
    pub sie4_path: String,
    pub manifest_path: String,
    pub skipped_documents: usize,
}

/// Document ready for export
#[derive(Debug, Clone, sqlx::FromRow)]
struct ExportDocument {
    id: i64,
    document_type: Option<String>,
    // TODO: used by future export filename generation
    #[allow(dead_code)]
    filename: Option<String>,
}

/// Complete document data with extracted fields and accounting decisions
#[derive(Debug, Clone)]
struct DocumentData {
    document: ExportDocument,
    fields: HashMap<String, Option<String>>,
    accounting: Option<AccountingData>,
    invoice_status: Option<String>,
}

#[derive(Debug, Clone)]
struct AccountingData {
    assigned_account_code: Option<String>,
    account_name: Option<String>,
    vat_rate: Option<f64>,
    vat_amount: Option<f64>,
    net_amount: Option<f64>,
    // TODO: used by future gross amount export
    #[allow(dead_code)]
    gross_amount: Option<f64>,
    reasoning_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExportBlockReason {
    MissingTransactionDate,
    InvalidTransactionDate,
    MissingSupplierName,
    MissingTotalAmount,
    InvalidTotalAmount,
    MissingAccountingDecision,
    MissingAssignedAccount,
    PlaceholderAccount,
    IncompleteAccountingReasoning,
    InvoicePendingAnalysis,
}

impl ExportBlockReason {
    pub fn as_code(&self) -> &'static str {
        match self {
            ExportBlockReason::MissingTransactionDate => "missing_transaction_date",
            ExportBlockReason::InvalidTransactionDate => "invalid_transaction_date",
            ExportBlockReason::MissingSupplierName => "missing_supplier_name",
            ExportBlockReason::MissingTotalAmount => "missing_total_amount",
            ExportBlockReason::InvalidTotalAmount => "invalid_total_amount",
            ExportBlockReason::MissingAccountingDecision => "missing_accounting_decision",
            ExportBlockReason::MissingAssignedAccount => "missing_assigned_account",
            ExportBlockReason::PlaceholderAccount => "placeholder_account",
            ExportBlockReason::IncompleteAccountingReasoning => "incomplete_accounting_reasoning",
            ExportBlockReason::InvoicePendingAnalysis => "invoice_pending_analysis",
        }
    }

    pub fn user_message(&self) -> &'static str {
        match self {
            ExportBlockReason::MissingTransactionDate => "missing transaction date",
            ExportBlockReason::InvalidTransactionDate => "invalid transaction date",
            ExportBlockReason::MissingSupplierName => "missing supplier name",
            ExportBlockReason::MissingTotalAmount => "missing total amount",
            ExportBlockReason::InvalidTotalAmount => "invalid total amount",
            ExportBlockReason::MissingAccountingDecision => "missing accounting decision",
            ExportBlockReason::MissingAssignedAccount => "missing assigned account",
            ExportBlockReason::PlaceholderAccount => "placeholder accounting account",
            ExportBlockReason::IncompleteAccountingReasoning => "incomplete accounting data",
            ExportBlockReason::InvoicePendingAnalysis => "invoice is still pending analysis",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportReadiness {
    pub ready: bool,
    pub reasons: Vec<ExportBlockReason>,
}

#[derive(Debug, Clone)]
pub enum ExportProcessError {
    EmptyPool,
    BlockedDocuments {
        blocked_count: usize,
        reasons: Vec<ExportBlockReason>,
    },
}

impl ExportProcessError {
    pub fn to_app_error(&self) -> AppError {
        match self {
            ExportProcessError::EmptyPool => {
                AppError::Agent("No documents ready for export".to_string())
            }
            ExportProcessError::BlockedDocuments {
                blocked_count,
                reasons,
            } => AppError::Agent(format!(
                "No exportable documents found (blocked_documents={}, reasons={})",
                blocked_count,
                summarize_block_reasons(reasons)
            )),
        }
    }
}

/// Export bundle manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportManifest {
    pub version: String,
    pub batch_id: String,
    pub generated_at: String,
    pub user_id: i64,
    pub filters: ExportFilters,
    pub documents: Vec<ManifestDocument>,
    pub statistics: ExportStatistics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportFilters {
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub document_types: Option<Vec<String>>,
    pub confidence_min: Option<f64>,
    pub short_refs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestDocument {
    pub document_id: String,
    pub document_type: Option<String>,
    pub verification_number: i32,
    pub transaction_date: Option<String>,
    pub supplier_name: Option<String>,
    pub total_amount: Option<f64>,
    pub account_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportStatistics {
    pub total_documents: usize,
    pub total_verifications: i32,
    pub total_amount: f64,
    pub vat_amount: f64,
}

/// ExportAgent generates SIE4 exports and ZIP bundles
pub struct ExportAgent {
    context: AgentContext,
}

fn decimal_to_f64(value: Option<&f64>) -> Option<f64> {
    value.copied()
}

impl ExportAgent {
    pub fn new(context: AgentContext) -> Self {
        Self { context }
    }

    pub async fn evaluate_document_readiness(
        pool: &DbPool,
        document_id: i64,
    ) -> AppResult<ExportReadiness> {
        let fields: Vec<(String, Option<String>)> = sqlx::query_as(
            r#"
            SELECT field_type, parsed_value
            FROM extracted_fields
            WHERE document_id = $1
            "#,
        )
        .bind(document_id)
        .fetch_all(pool)
        .await?;
        let field_map: HashMap<String, Option<String>> = fields.into_iter().collect();

        let accounting: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            r#"
            SELECT assigned_account_code, account_name, reasoning_text
            FROM accounting_decisions
            WHERE document_id = $1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(document_id)
        .fetch_optional(pool)
        .await?;

        let invoice_status: Option<String> = sqlx::query_scalar(
            r#"
            SELECT status
            FROM invoices
            WHERE document_id = $1
            LIMIT 1
            "#,
        )
        .bind(document_id)
        .fetch_optional(pool)
        .await?;

        Ok(Self::evaluate_readiness(
            &field_map,
            accounting.as_ref(),
            invoice_status.as_deref(),
        ))
    }

    pub(crate) fn evaluate_readiness(
        fields: &HashMap<String, Option<String>>,
        accounting: Option<&(Option<String>, Option<String>, Option<String>)>,
        invoice_status: Option<&str>,
    ) -> ExportReadiness {
        let mut reasons = Vec::new();

        match fields.get("transaction_date").and_then(|v| v.as_ref()) {
            Some(date) if date.len() == 10 => {}
            Some(_) => reasons.push(ExportBlockReason::InvalidTransactionDate),
            None => reasons.push(ExportBlockReason::MissingTransactionDate),
        }

        if fields
            .get("supplier_name")
            .and_then(|v| v.as_ref())
            .is_none()
        {
            reasons.push(ExportBlockReason::MissingSupplierName);
        }

        match fields.get("total_amount").and_then(|v| v.as_ref()) {
            Some(amount) if amount.parse::<f64>().is_ok() => {}
            Some(_) => reasons.push(ExportBlockReason::InvalidTotalAmount),
            None => reasons.push(ExportBlockReason::MissingTotalAmount),
        }

        match accounting {
            None => reasons.push(ExportBlockReason::MissingAccountingDecision),
            Some((assigned_account_code, account_name, reasoning_text)) => {
                match assigned_account_code.as_deref() {
                    None | Some("") => reasons.push(ExportBlockReason::MissingAssignedAccount),
                    Some("1930") => reasons.push(ExportBlockReason::PlaceholderAccount),
                    Some(_) => {}
                }

                if account_name.as_deref() == Some("Placeholder for missing data") {
                    reasons.push(ExportBlockReason::PlaceholderAccount);
                }

                if let Some(reasoning) = reasoning_text.as_deref()
                    && (reasoning.contains("No extracted document data was provided")
                        || reasoning
                            .contains("Assignment total (0.00) doesn't match invoice total"))
                {
                    reasons.push(ExportBlockReason::IncompleteAccountingReasoning);
                }
            }
        }

        if invoice_status == Some("PENDING_ANALYSIS") {
            reasons.push(ExportBlockReason::InvoicePendingAnalysis);
        }

        dedup_reasons(&mut reasons);

        ExportReadiness {
            ready: reasons.is_empty(),
            reasons,
        }
    }

    /// Query documents ready for export
    async fn query_ready_documents(&self, input: &ExportInput) -> AppResult<Vec<ExportDocument>> {
        let mut builder: QueryBuilder<'_, Sqlite> = QueryBuilder::new(
            "SELECT d.id, d.document_type, d.filename \
             FROM documents d \
             JOIN document_accounting_state das ON das.document_id = d.id \
             WHERE das.status = ",
        );
        builder.push_bind("READY_FOR_EXPORT");

        if let Some(date_from) = input.date_from {
            builder
                .push(" AND date(received_at) >= date(")
                .push_bind(date_from)
                .push(")");
        }
        if let Some(date_to) = input.date_to {
            builder
                .push(" AND date(received_at) <= date(")
                .push_bind(date_to)
                .push(")");
        }
        if let Some(confidence_min) = input.confidence_min {
            builder.push(
                " AND EXISTS (SELECT 1 FROM review_decisions rd WHERE rd.document_id = d.id AND rd.confidence_score >= ",
            );
            builder.push_bind(confidence_min).push(")");
        }
        if let Some(document_types) = input.document_types.as_ref()
            && !document_types.is_empty()
        {
            builder.push(" AND d.document_type IN (");
            let mut separated = builder.separated(", ");
            for value in document_types {
                separated.push_bind(value);
            }
            separated.push_unseparated(")");
        }
        if let Some(short_refs) = input.short_refs.as_ref()
            && !short_refs.is_empty()
        {
            builder.push(" AND short_ref IN (");
            let mut separated = builder.separated(", ");
            for value in short_refs {
                separated.push_bind(value);
            }
            separated.push_unseparated(")");
        }
        builder.push(" ORDER BY received_at ASC LIMIT 1000");

        let rows: Vec<ExportDocument> = builder
            .build_query_as()
            .fetch_all(&self.context.pool)
            .await?;

        Ok(rows)
    }

    /// Load complete document data
    async fn load_document_data(&self, doc: &ExportDocument) -> AppResult<DocumentData> {
        // Load extracted fields
        let fields: Vec<(String, Option<String>)> = sqlx::query_as(
            r#"
            SELECT field_type, parsed_value
            FROM extracted_fields
            WHERE document_id = $1
            "#,
        )
        .bind(doc.id)
        .fetch_all(&self.context.pool)
        .await?;

        let field_map: HashMap<String, Option<String>> = fields.into_iter().collect();

        // Load accounting decision
        let accounting: Option<AccountingData> = sqlx::query_as::<
            _,
            (
                Option<String>,
                Option<String>,
                Option<f64>,
                Option<f64>,
                Option<f64>,
                Option<f64>,
                Option<String>,
            ),
        >(
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
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(doc.id)
        .fetch_optional(&self.context.pool)
        .await?
        .map(|(ac, an, vr, va, na, ga, rt)| AccountingData {
            assigned_account_code: ac,
            account_name: an,
            vat_rate: vr,
            vat_amount: va,
            net_amount: na,
            gross_amount: ga,
            reasoning_text: rt,
        });

        let invoice_status: Option<String> = sqlx::query_scalar(
            r#"
            SELECT status
            FROM invoices
            WHERE document_id = $1
            LIMIT 1
            "#,
        )
        .bind(doc.id)
        .fetch_optional(&self.context.pool)
        .await?;

        Ok(DocumentData {
            document: doc.clone(),
            fields: field_map,
            accounting,
            invoice_status,
        })
    }

    /// Generate verification number from document index
    fn generate_verification_number(&self, index: usize) -> i32 {
        (index + 1) as i32
    }

    /// Convert document data to SIE4 verification
    fn document_to_verification(
        &self,
        doc_data: &DocumentData,
        ver_number: i32,
    ) -> Result<Verification, Vec<ExportBlockReason>> {
        let fields = &doc_data.fields;
        let accounting_tuple = doc_data.accounting.as_ref().map(|accounting| {
            (
                accounting.assigned_account_code.clone(),
                accounting.account_name.clone(),
                accounting.reasoning_text.clone(),
            )
        });
        let readiness = Self::evaluate_readiness(
            fields,
            accounting_tuple.as_ref(),
            doc_data.invoice_status.as_deref(),
        );
        if !readiness.ready {
            return Err(readiness.reasons);
        }

        // Extract required fields
        let transaction_date = fields
            .get("transaction_date")
            .and_then(|v| v.as_ref())
            .ok_or_else(|| vec![ExportBlockReason::MissingTransactionDate])?;
        let supplier_name = fields
            .get("supplier_name")
            .and_then(|v| v.as_ref())
            .ok_or_else(|| vec![ExportBlockReason::MissingSupplierName])?;
        let total_amount = fields
            .get("total_amount")
            .and_then(|v| v.as_ref())
            .ok_or_else(|| vec![ExportBlockReason::MissingTotalAmount])?;

        // Parse date (YYYY-MM-DD to YYYYMMDD)
        let date_parsed = if transaction_date.len() == 10 {
            transaction_date.replace("-", "")
        } else {
            return Err(vec![ExportBlockReason::InvalidTransactionDate]);
        };

        // Store a reference for verification
        let ver_date = date_parsed.clone();

        // Parse amount (handle decimals)
        let total: f64 = total_amount
            .parse()
            .map_err(|_| vec![ExportBlockReason::InvalidTotalAmount])?;

        // Get accounting info
        let account_code = doc_data
            .accounting
            .as_ref()
            .and_then(|a| a.assigned_account_code.clone())
            .ok_or_else(|| vec![ExportBlockReason::MissingAssignedAccount])?;

        let motkonto = "1930".to_string(); // Default contra account (checking account)

        // Parse VAT info
        let vat_amount = doc_data
            .accounting
            .as_ref()
            .and_then(|a| decimal_to_f64(a.vat_amount.as_ref()));
        let vat_rate = doc_data
            .accounting
            .as_ref()
            .and_then(|a| decimal_to_f64(a.vat_rate.as_ref()));
        let net_amount = doc_data
            .accounting
            .as_ref()
            .and_then(|a| decimal_to_f64(a.net_amount.as_ref()));

        // Create verification text (max 30 chars)
        let ver_text = if supplier_name.len() > 25 {
            format!("{}...", &supplier_name[..25])
        } else {
            supplier_name.clone()
        };

        let today = Utc::now().format("%Y%m%d").to_string();
        let signature = "Finelor";

        // Build transactions
        let mut transactions: Vec<Sie4Transaction> = Vec::new();

        // Main expense/debit transaction
        let vat_account = Self::get_vat_account(vat_rate);

        if let Some(vat) = vat_amount {
            // Net amount to main account
            if let Some(net) = net_amount {
                transactions.push(Sie4Transaction {
                    account: account_code.clone(),
                    amount: net,
                    trans_date: date_parsed.clone(),
                    ver_date: date_parsed.clone(),
                    object1: None,
                    object2: None,
                    quantity: None,
                    signature: None,
                });
            }

            // VAT to VAT account
            transactions.push(Sie4Transaction {
                account: vat_account,
                amount: vat,
                trans_date: date_parsed.clone(),
                ver_date: date_parsed.clone(),
                object1: None,
                object2: None,
                quantity: None,
                signature: None,
            });

            // Contra entry for total
            transactions.push(Sie4Transaction {
                account: motkonto.clone(),
                amount: -total,
                trans_date: date_parsed.clone(),
                ver_date: date_parsed.clone(),
                object1: None,
                object2: None,
                quantity: None,
                signature: None,
            });
        } else {
            // Simple two-transaction entry (no VAT breakdown)
            transactions.push(Sie4Transaction {
                account: account_code,
                amount: total,
                trans_date: date_parsed.clone(),
                ver_date: date_parsed.clone(),
                object1: None,
                object2: None,
                quantity: None,
                signature: None,
            });

            transactions.push(Sie4Transaction {
                account: motkonto,
                amount: -total,
                trans_date: date_parsed.clone(),
                ver_date: ver_date.clone(),
                object1: None,
                object2: None,
                quantity: None,
                signature: None,
            });
        }

        Ok(Verification {
            series: "F".to_string(),
            number: ver_number,
            date: ver_date,
            text: ver_text,
            reg_date: today,
            signature: signature.to_string(),
            project: None,
            transactions,
        })
    }

    /// Get VAT account based on rate
    fn get_vat_account(vat_rate: Option<f64>) -> String {
        match vat_rate {
            Some(0.25) => "2640".to_string(), // Incoming VAT 25%
            Some(0.12) => "2641".to_string(), // Incoming VAT 12%
            Some(0.06) => "2642".to_string(), // Incoming VAT 6%
            _ => "2640".to_string(),          // Default to 25%
        }
    }

    /// Generate export manifest
    fn generate_manifest(
        &self,
        input: &ExportInput,
        documents: &[DocumentData],
        batch_id: i64,
    ) -> ExportManifest {
        let manifest_docs: Vec<ManifestDocument> = documents
            .iter()
            .enumerate()
            .map(|(i, d)| ManifestDocument {
                document_id: d.document.id.to_string(),
                document_type: d.document.document_type.clone(),
                verification_number: (i + 1) as i32,
                transaction_date: d.fields.get("transaction_date").cloned().flatten(),
                supplier_name: d.fields.get("supplier_name").cloned().flatten(),
                total_amount: d
                    .fields
                    .get("total_amount")
                    .and_then(|v| v.as_ref().and_then(|s| s.parse().ok())),
                account_code: d
                    .accounting
                    .as_ref()
                    .and_then(|a| a.assigned_account_code.clone()),
            })
            .collect();

        let total_amount: f64 = manifest_docs.iter().filter_map(|d| d.total_amount).sum();

        let vat_amount: f64 = documents
            .iter()
            .filter_map(|d| {
                d.accounting
                    .as_ref()
                    .and_then(|a| decimal_to_f64(a.vat_amount.as_ref()))
            })
            .sum();

        ExportManifest {
            version: "1.0.0".to_string(),
            batch_id: batch_id.to_string(),
            generated_at: Utc::now().to_rfc3339(),
            user_id: input.user_id,
            filters: ExportFilters {
                date_from: input.date_from.map(|d| d.to_string()),
                date_to: input.date_to.map(|d| d.to_string()),
                document_types: input.document_types.clone(),
                confidence_min: input.confidence_min,
                short_refs: input.short_refs.clone(),
            },
            documents: manifest_docs,
            statistics: ExportStatistics {
                total_documents: documents.len(),
                total_verifications: documents.len() as i32,
                total_amount,
                vat_amount,
            },
        }
    }

    /// Mark documents as exported
    async fn mark_exported(
        &self,
        tx: &mut SqlxTransaction<'_, Sqlite>,
        document_ids: &[i64],
        batch_id: i64,
    ) -> AppResult<()> {
        for doc_id in document_ids {
            crate::document_state::mark_exported_in_tx(tx, *doc_id, batch_id).await?;
            sqlx::query(
                r#"
                INSERT INTO document_events (document_id, event_type, payload)
                VALUES ($1, 'EXPORTED', $2)
                "#,
            )
            .bind(doc_id)
            .bind(serde_json::json!({ "batch_id": batch_id }))
            .execute(&mut **tx)
            .await?;
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl Agent for ExportAgent {
    type Input = ExportInput;
    type Output = ExportOutput;
    type Error = AppError;

    fn name(&self) -> &'static str {
        "ExportAgent"
    }

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        info!(user_id = %input.user_id, workspace_id = ?input.workspace_id, "Starting export");

        // Query ready documents
        let documents = self.query_ready_documents(&input).await?;

        if documents.is_empty() {
            warn!("No documents ready for export");
            return Err(ExportProcessError::EmptyPool.to_app_error());
        }

        info!(count = documents.len(), "Found documents ready for export");

        // Load complete data for each document
        let mut doc_data_list = Vec::new();
        for doc in &documents {
            match self.load_document_data(doc).await {
                Ok(data) => doc_data_list.push(data),
                Err(e) => {
                    warn!(document_id = %doc.id, error = %e, "Failed to load document data");
                }
            }
        }

        if doc_data_list.is_empty() {
            return Err(ExportProcessError::BlockedDocuments {
                blocked_count: documents.len(),
                reasons: vec![ExportBlockReason::MissingAccountingDecision],
            }
            .to_app_error());
        }

        let filter_criteria = serde_json::to_string(&input)?;
        let batch_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO export_batches (user_id, document_count, filter_criteria, expires_at)
            VALUES ($1, 0, $2, datetime('now', '+30 days'))
            RETURNING id
            "#,
        )
        .bind(input.user_id)
        .bind(&filter_criteria)
        .fetch_one(&self.context.pool)
        .await?;

        // Create SIE4 config
        let config = Sie4Config {
            program_name: "Finelor".to_string(),
            program_version: env!("CARGO_PKG_VERSION").to_string(),
            encoding: "PC8".to_string(),
            company_org_nr: self.context.config.export.company_org_nr.clone(),
            company_name: self.context.config.export.company_name.clone(),
            fiscal_year_start: self.context.config.export.fiscal_year_start.clone(),
            fiscal_year_end: self.context.config.export.fiscal_year_end.clone(),
            currency: "SEK".to_string(),
        };

        // Convert documents to verifications
        let mut verifications = Vec::new();
        let mut exportable_docs = Vec::new();
        let mut blocked_documents: Vec<(i64, Vec<ExportBlockReason>)> = Vec::new();
        for (i, doc_data) in doc_data_list.iter().enumerate() {
            let ver_number = self.generate_verification_number(i);
            match self.document_to_verification(doc_data, ver_number) {
                Ok(ver) => {
                    verifications.push(ver);
                    exportable_docs.push(doc_data.clone());
                }
                Err(reasons) => {
                    warn!(
                        document_id = %doc_data.document.id,
                        reasons = %summarize_block_reasons(&reasons),
                        "Document blocked from export"
                    );
                    self.context
                        .record_document_event(
                            doc_data.document.id,
                            "EXPORT_BLOCKED",
                            serde_json::json!({
                                "reasons": reasons.iter().map(ExportBlockReason::as_code).collect::<Vec<_>>()
                            }),
                        )
                    .await?;
                    blocked_documents.push((doc_data.document.id, reasons));
                }
            }
        }

        if verifications.is_empty() {
            let mut reasons = Vec::new();
            for (_, blocked_reasons) in &blocked_documents {
                reasons.extend(blocked_reasons.iter().cloned());
            }
            dedup_reasons(&mut reasons);
            return Err(ExportProcessError::BlockedDocuments {
                blocked_count: blocked_documents.len(),
                reasons,
            }
            .to_app_error());
        }

        // Generate SIE4 content
        let sie4_content = generate_sie4_export(&config, &verifications)?;

        // Generate manifest
        let manifest = self.generate_manifest(&input, &exportable_docs, batch_id);
        let manifest_json = serde_json::to_string_pretty(&manifest)?;

        // Create export directory
        let export_dir = std::path::PathBuf::from(&self.context.config.export.output_path)
            .join(format!("batch_{}", batch_id));
        tokio::fs::create_dir_all(&export_dir).await?;

        // Write files
        let sie4_path = export_dir.join("export.si");
        let manifest_path = export_dir.join("manifest.json");
        let zip_path = export_dir.join("export.zip");

        // Write SIE4 file (PC8 encoded)
        let sie4_bytes = crate::export::encode_sie4(&sie4_content)?;
        tokio::fs::write(&sie4_path, &sie4_bytes).await?;

        // Write manifest
        tokio::fs::write(&manifest_path, &manifest_json).await?;

        // Create ZIP bundle
        let zip_file = std::fs::File::create(&zip_path)?;
        let mut zip = zip::ZipWriter::new(zip_file);

        use std::io::Write;

        let options = zip::write::FileOptions::<()>::default()
            .compression_method(zip::CompressionMethod::Deflated);

        zip.start_file("export.si", options)?;
        zip.write_all(&sie4_bytes)?;

        zip.start_file("manifest.json", options)?;
        zip.write_all(manifest_json.as_bytes())?;

        zip.finish()?;

        let doc_ids: Vec<i64> = exportable_docs.iter().map(|d| d.document.id).collect();
        let mut tx = self.context.pool.begin().await?;

        for doc_id in &doc_ids {
            crate::document_state::start_exporting_in_tx(&mut tx, *doc_id, Some(batch_id)).await?;
        }

        // Store export batch paths after files are generated.
        sqlx::query(
            r#"
            UPDATE export_batches
            SET document_count = $1,
                sie4_path = $2,
                manifest_path = $3,
                zip_path = $4,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $5
            "#,
        )
        .bind(verifications.len() as i32)
        .bind(sie4_path.to_str())
        .bind(manifest_path.to_str())
        .bind(zip_path.to_str())
        .bind(batch_id)
        .execute(&mut *tx)
        .await?;

        // Mark documents as exported
        self.mark_exported(&mut tx, &doc_ids, batch_id).await?;
        tx.commit().await?;

        for doc_id in &doc_ids {
            info!(document_id = %doc_id, batch_id = %batch_id, "Document marked as exported");
            self.context
                .record_document_event(
                    *doc_id,
                    "EXPORT_BATCH_CREATED",
                    serde_json::json!({ "batch_id": batch_id }),
                )
                .await?;
        }

        info!(
            batch_id = %batch_id,
            document_count = verifications.len(),
            skipped_documents = blocked_documents.len(),
            "Export complete"
        );

        Ok(ExportOutput {
            batch_id,
            document_count: verifications.len(),
            zip_path: zip_path.to_string_lossy().to_string(),
            sie4_path: sie4_path.to_string_lossy().to_string(),
            manifest_path: manifest_path.to_string_lossy().to_string(),
            skipped_documents: blocked_documents.len(),
        })
    }
}

fn dedup_reasons(reasons: &mut Vec<ExportBlockReason>) {
    let mut seen = std::collections::HashSet::new();
    reasons.retain(|reason| seen.insert(reason.as_code()));
}

pub fn summarize_block_reasons(reasons: &[ExportBlockReason]) -> String {
    let mut labels = Vec::new();
    for reason in reasons {
        let message = reason.user_message();
        if !labels.contains(&message) {
            labels.push(message);
        }
    }
    labels.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fields(
        transaction_date: Option<&str>,
        supplier_name: Option<&str>,
        total_amount: Option<&str>,
    ) -> HashMap<String, Option<String>> {
        let mut fields = HashMap::new();
        fields.insert(
            "transaction_date".to_string(),
            transaction_date.map(|s| s.to_string()),
        );
        fields.insert(
            "supplier_name".to_string(),
            supplier_name.map(|s| s.to_string()),
        );
        fields.insert(
            "total_amount".to_string(),
            total_amount.map(|s| s.to_string()),
        );
        fields
    }

    fn make_accounting(
        assigned_account_code: Option<&str>,
        account_name: Option<&str>,
        reasoning_text: Option<&str>,
    ) -> (Option<String>, Option<String>, Option<String>) {
        (
            assigned_account_code.map(|s| s.to_string()),
            account_name.map(|s| s.to_string()),
            reasoning_text.map(|s| s.to_string()),
        )
    }

    #[test]
    fn readiness_all_fields_valid() {
        let fields = make_fields(Some("2024-01-15"), Some("Acme Corp"), Some("1234.56"));
        let accounting = make_accounting(Some("6100"), Some("Office supplies"), Some("Valid"));
        let result = ExportAgent::evaluate_readiness(&fields, Some(&accounting), None);
        assert!(result.ready);
        assert!(result.reasons.is_empty());
    }

    #[test]
    fn readiness_missing_transaction_date() {
        let fields = make_fields(None, Some("Acme Corp"), Some("1234.56"));
        let accounting = make_accounting(Some("6100"), Some("Office supplies"), Some("Valid"));
        let result = ExportAgent::evaluate_readiness(&fields, Some(&accounting), None);
        assert!(!result.ready);
        assert_eq!(
            result.reasons,
            vec![ExportBlockReason::MissingTransactionDate]
        );
    }

    #[test]
    fn readiness_invalid_transaction_date() {
        let fields = make_fields(Some("2024-01-1"), Some("Acme Corp"), Some("1234.56"));
        let accounting = make_accounting(Some("6100"), Some("Office supplies"), Some("Valid"));
        let result = ExportAgent::evaluate_readiness(&fields, Some(&accounting), None);
        assert!(!result.ready);
        assert_eq!(
            result.reasons,
            vec![ExportBlockReason::InvalidTransactionDate]
        );
    }

    #[test]
    fn readiness_missing_supplier_name() {
        let fields = make_fields(Some("2024-01-15"), None, Some("1234.56"));
        let accounting = make_accounting(Some("6100"), Some("Office supplies"), Some("Valid"));
        let result = ExportAgent::evaluate_readiness(&fields, Some(&accounting), None);
        assert!(!result.ready);
        assert_eq!(result.reasons, vec![ExportBlockReason::MissingSupplierName]);
    }

    #[test]
    fn readiness_missing_total_amount() {
        let fields = make_fields(Some("2024-01-15"), Some("Acme Corp"), None);
        let accounting = make_accounting(Some("6100"), Some("Office supplies"), Some("Valid"));
        let result = ExportAgent::evaluate_readiness(&fields, Some(&accounting), None);
        assert!(!result.ready);
        assert_eq!(result.reasons, vec![ExportBlockReason::MissingTotalAmount]);
    }

    #[test]
    fn readiness_invalid_total_amount() {
        let fields = make_fields(Some("2024-01-15"), Some("Acme Corp"), Some("not-a-number"));
        let accounting = make_accounting(Some("6100"), Some("Office supplies"), Some("Valid"));
        let result = ExportAgent::evaluate_readiness(&fields, Some(&accounting), None);
        assert!(!result.ready);
        assert_eq!(result.reasons, vec![ExportBlockReason::InvalidTotalAmount]);
    }

    #[test]
    fn readiness_missing_accounting_decision() {
        let fields = make_fields(Some("2024-01-15"), Some("Acme Corp"), Some("1234.56"));
        let result = ExportAgent::evaluate_readiness(&fields, None, None);
        assert!(!result.ready);
        assert_eq!(
            result.reasons,
            vec![ExportBlockReason::MissingAccountingDecision]
        );
    }

    #[test]
    fn readiness_missing_assigned_account_none() {
        let fields = make_fields(Some("2024-01-15"), Some("Acme Corp"), Some("1234.56"));
        let accounting = make_accounting(None, Some("Office supplies"), Some("Valid"));
        let result = ExportAgent::evaluate_readiness(&fields, Some(&accounting), None);
        assert!(!result.ready);
        assert_eq!(
            result.reasons,
            vec![ExportBlockReason::MissingAssignedAccount]
        );
    }

    #[test]
    fn readiness_missing_assigned_account_empty() {
        let fields = make_fields(Some("2024-01-15"), Some("Acme Corp"), Some("1234.56"));
        let accounting = make_accounting(Some(""), Some("Office supplies"), Some("Valid"));
        let result = ExportAgent::evaluate_readiness(&fields, Some(&accounting), None);
        assert!(!result.ready);
        assert_eq!(
            result.reasons,
            vec![ExportBlockReason::MissingAssignedAccount]
        );
    }

    #[test]
    fn readiness_placeholder_account_code() {
        let fields = make_fields(Some("2024-01-15"), Some("Acme Corp"), Some("1234.56"));
        let accounting = make_accounting(Some("1930"), Some("Office supplies"), Some("Valid"));
        let result = ExportAgent::evaluate_readiness(&fields, Some(&accounting), None);
        assert!(!result.ready);
        assert_eq!(result.reasons, vec![ExportBlockReason::PlaceholderAccount]);
    }

    #[test]
    fn readiness_placeholder_account_name() {
        let fields = make_fields(Some("2024-01-15"), Some("Acme Corp"), Some("1234.56"));
        let accounting = make_accounting(
            Some("6100"),
            Some("Placeholder for missing data"),
            Some("Valid"),
        );
        let result = ExportAgent::evaluate_readiness(&fields, Some(&accounting), None);
        assert!(!result.ready);
        assert_eq!(result.reasons, vec![ExportBlockReason::PlaceholderAccount]);
    }

    #[test]
    fn readiness_incomplete_reasoning_no_data() {
        let fields = make_fields(Some("2024-01-15"), Some("Acme Corp"), Some("1234.56"));
        let accounting = make_accounting(
            Some("6100"),
            Some("Office supplies"),
            Some("No extracted document data was provided"),
        );
        let result = ExportAgent::evaluate_readiness(&fields, Some(&accounting), None);
        assert!(!result.ready);
        assert_eq!(
            result.reasons,
            vec![ExportBlockReason::IncompleteAccountingReasoning]
        );
    }

    #[test]
    fn readiness_incomplete_reasoning_mismatch() {
        let fields = make_fields(Some("2024-01-15"), Some("Acme Corp"), Some("1234.56"));
        let accounting = make_accounting(
            Some("6100"),
            Some("Office supplies"),
            Some("Assignment total (0.00) doesn't match invoice total"),
        );
        let result = ExportAgent::evaluate_readiness(&fields, Some(&accounting), None);
        assert!(!result.ready);
        assert_eq!(
            result.reasons,
            vec![ExportBlockReason::IncompleteAccountingReasoning]
        );
    }

    #[test]
    fn readiness_invoice_pending_analysis() {
        let fields = make_fields(Some("2024-01-15"), Some("Acme Corp"), Some("1234.56"));
        let accounting = make_accounting(Some("6100"), Some("Office supplies"), Some("Valid"));
        let result =
            ExportAgent::evaluate_readiness(&fields, Some(&accounting), Some("PENDING_ANALYSIS"));
        assert!(!result.ready);
        assert_eq!(
            result.reasons,
            vec![ExportBlockReason::InvoicePendingAnalysis]
        );
    }

    #[test]
    fn readiness_multiple_block_reasons() {
        let fields = make_fields(None, None, Some("not-a-number"));
        let result = ExportAgent::evaluate_readiness(&fields, None, Some("PENDING_ANALYSIS"));
        assert!(!result.ready);
        let expected = vec![
            ExportBlockReason::MissingTransactionDate,
            ExportBlockReason::MissingSupplierName,
            ExportBlockReason::InvalidTotalAmount,
            ExportBlockReason::MissingAccountingDecision,
            ExportBlockReason::InvoicePendingAnalysis,
        ];
        assert_eq!(result.reasons, expected);
    }

    #[test]
    fn dedup_reasons_removes_duplicates() {
        let mut reasons = vec![
            ExportBlockReason::MissingTransactionDate,
            ExportBlockReason::MissingTransactionDate,
            ExportBlockReason::PlaceholderAccount,
        ];
        dedup_reasons(&mut reasons);
        assert_eq!(reasons.len(), 2);
        assert_eq!(reasons[0], ExportBlockReason::MissingTransactionDate);
        assert_eq!(reasons[1], ExportBlockReason::PlaceholderAccount);
    }

    #[test]
    fn block_reason_as_code_all_variants() {
        assert_eq!(
            ExportBlockReason::MissingTransactionDate.as_code(),
            "missing_transaction_date"
        );
        assert_eq!(
            ExportBlockReason::InvalidTransactionDate.as_code(),
            "invalid_transaction_date"
        );
        assert_eq!(
            ExportBlockReason::MissingSupplierName.as_code(),
            "missing_supplier_name"
        );
        assert_eq!(
            ExportBlockReason::MissingTotalAmount.as_code(),
            "missing_total_amount"
        );
        assert_eq!(
            ExportBlockReason::InvalidTotalAmount.as_code(),
            "invalid_total_amount"
        );
        assert_eq!(
            ExportBlockReason::MissingAccountingDecision.as_code(),
            "missing_accounting_decision"
        );
        assert_eq!(
            ExportBlockReason::MissingAssignedAccount.as_code(),
            "missing_assigned_account"
        );
        assert_eq!(
            ExportBlockReason::PlaceholderAccount.as_code(),
            "placeholder_account"
        );
        assert_eq!(
            ExportBlockReason::IncompleteAccountingReasoning.as_code(),
            "incomplete_accounting_reasoning"
        );
        assert_eq!(
            ExportBlockReason::InvoicePendingAnalysis.as_code(),
            "invoice_pending_analysis"
        );
    }

    #[test]
    fn block_reason_user_message_all_variants() {
        assert_eq!(
            ExportBlockReason::MissingTransactionDate.user_message(),
            "missing transaction date"
        );
        assert_eq!(
            ExportBlockReason::InvalidTransactionDate.user_message(),
            "invalid transaction date"
        );
        assert_eq!(
            ExportBlockReason::MissingSupplierName.user_message(),
            "missing supplier name"
        );
        assert_eq!(
            ExportBlockReason::MissingTotalAmount.user_message(),
            "missing total amount"
        );
        assert_eq!(
            ExportBlockReason::InvalidTotalAmount.user_message(),
            "invalid total amount"
        );
        assert_eq!(
            ExportBlockReason::MissingAccountingDecision.user_message(),
            "missing accounting decision"
        );
        assert_eq!(
            ExportBlockReason::MissingAssignedAccount.user_message(),
            "missing assigned account"
        );
        assert_eq!(
            ExportBlockReason::PlaceholderAccount.user_message(),
            "placeholder accounting account"
        );
        assert_eq!(
            ExportBlockReason::IncompleteAccountingReasoning.user_message(),
            "incomplete accounting data"
        );
        assert_eq!(
            ExportBlockReason::InvoicePendingAnalysis.user_message(),
            "invoice is still pending analysis"
        );
    }

    #[test]
    fn summarize_empty() {
        assert_eq!(summarize_block_reasons(&[]), "");
    }

    #[test]
    fn summarize_single() {
        let reasons = vec![ExportBlockReason::MissingTransactionDate];
        assert_eq!(
            summarize_block_reasons(&reasons),
            "missing transaction date"
        );
    }

    #[test]
    fn summarize_multiple() {
        let reasons = vec![
            ExportBlockReason::MissingTransactionDate,
            ExportBlockReason::MissingSupplierName,
        ];
        assert_eq!(
            summarize_block_reasons(&reasons),
            "missing transaction date, missing supplier name"
        );
    }

    #[test]
    fn summarize_deduplicates_by_message() {
        let reasons = vec![
            ExportBlockReason::MissingTransactionDate,
            ExportBlockReason::MissingTransactionDate,
        ];
        assert_eq!(
            summarize_block_reasons(&reasons),
            "missing transaction date"
        );
    }

    #[test]
    fn export_agent_has_no_channel_chat_scope_dependency() {
        let source = include_str!("export.rs");
        let forbidden_a = ["channel_metadata->>'", "chat", "_", "id", "'"].concat();
        let forbidden_b = ["chat", "_", "id"].concat();
        let forbidden_c = ["document", "_", "artifacts"].concat();

        assert!(!source.contains(&forbidden_a));
        assert!(!source.contains(&forbidden_b));
        assert!(!source.contains(&forbidden_c));
    }
}
