use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

use crate::agents::{Agent, AgentContext, FieldLoadMode, db_helpers};
use crate::document_state;
use crate::error::{AppError, AppResult};
use crate::inference::{
    ChatJsonRequest, ChatMessage, InferenceProvider, extract_json_from_response,
};

/// Input for the AccountantAgent
#[derive(Debug, Clone)]
pub struct AccountantInput {
    pub document_id: i64,
}

/// Output from the AccountantAgent
#[derive(Debug)]
pub struct AccountantOutput {
    pub document_id: i64,
    pub invoice_id: Option<i64>,
    pub account_assignments: Vec<AccountAssignment>,
    pub status: String,
    pub warnings: Vec<String>,
}

/// Account assignment made by the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountAssignment {
    pub account_code: String,
    pub account_name: String,
    pub description: String,
    pub amount: f64,
    pub vat_code: Option<String>,
    pub confidence: f64,
    pub reasoning: String,
}

/// Accounting entry (verifikation) output
/// All fields are optional with defaults to handle imperfect LLM responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountingEntry {
    #[serde(default = "default_date")]
    pub entry_date: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub entries: Vec<KontoEntry>,
    /* Total debet = total kredit */
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}

fn default_date() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KontoEntry {
    #[serde(default)]
    pub account_code: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub debet: f64,
    #[serde(default)]
    pub kredit: f64,
}

/// Struct to hold invoice data
#[derive(sqlx::FromRow)]
struct InvoiceRow {
    id: Option<i64>,
}

/// AccountantAgent assigns bas accounts and validates accounting entries
pub struct AccountantAgent {
    context: AgentContext,
    inference_provider: Arc<dyn InferenceProvider>,
    system_prompt_template: String,
    prompt_path: String,
    model: String,
    field_load_mode: FieldLoadMode,
}

/// Data fetched for LLM context
#[derive(Debug, Default)]
struct DocumentContext {
    supplier_name: Option<String>,
    supplier_org_nr: Option<String>,
    transaction_date: Option<String>,
    invoice_number: Option<String>,
    total_amount: Option<String>,
    vat_amount: Option<String>,
    vat_rate: Option<String>,
    currency: Option<String>,
    extracted_text: Option<String>,
    // TODO: used by future line-item extraction feature
    #[allow(dead_code)]
    line_items: Option<Vec<Value>>,
}

impl AccountantAgent {
    pub fn new(
        context: AgentContext,
        inference_provider: Arc<dyn InferenceProvider>,
        field_load_mode: FieldLoadMode,
    ) -> Self {
        let model = context.config.ollama.models.accountant.clone();
        let prompt_path = context.config.ollama.accountant_prompt_path.clone();
        let system_prompt_template = std::fs::read_to_string(&prompt_path).unwrap_or_else(|err| {
            warn!(
                prompt_path = %prompt_path,
                error = %err,
                "Could not read configured accountant prompt, using deterministic fallback"
            );
            "You are an expert Swedish accountant. Return only a raw JSON object with keys entry_date, description, entries, verified, and warnings. Do not use markdown fences or extra text.".to_string()
        });

        Self {
            context,
            inference_provider,
            system_prompt_template,
            prompt_path,
            model,
            field_load_mode,
        }
    }

    /// Gather all extracted fields for context
    async fn gather_context(&self, document_id: i64) -> AppResult<DocumentContext> {
        let mut context = DocumentContext::default();

        let fields =
            db_helpers::load_document_fields(&self.context.pool, document_id, self.field_load_mode)
                .await?;

        for (field_type, value) in fields {
            let value_text = value.clone().unwrap_or_default();
            match field_type.as_str() {
                "supplier_name" => context.supplier_name = value,
                "supplier_org_nr" => context.supplier_org_nr = value,
                "transaction_date" => context.transaction_date = value,
                "invoice_number" => context.invoice_number = value,
                "total_amount" => context.total_amount = Some(value_text),
                "vat_amount" => context.vat_amount = Some(value_text),
                "vat_rate" => context.vat_rate = Some(value_text),
                "currency" => context.currency = value,
                _ => {}
            }
        }

        Ok(context)
    }

    fn build_system_prompt(&self) -> String {
        format!(
            "{}\n\nCRITICAL: Output must be a raw JSON object starting with {{ and ending with }}. Do not use markdown fences. Do not prefix or suffix any text.",
            self.system_prompt_template.trim_end()
        )
    }

    fn build_user_prompt(&self, context: &DocumentContext) -> String {
        format!(
            "Create the accounting entry for this document.\n\nDocument data:\n- Supplier: {}\n- Supplier org nr: {}\n- Transaction date: {}\n- Invoice number: {}\n- Total amount: {} {}\n- VAT amount: {}\n- VAT rate: {}\n- Extracted text: {}\n\nReturn only the JSON object. No markdown code fences. No leading or trailing text.",
            context.supplier_name.as_deref().unwrap_or("Unknown"),
            context.supplier_org_nr.as_deref().unwrap_or("Unknown"),
            context.transaction_date.as_deref().unwrap_or("Unknown"),
            context.invoice_number.as_deref().unwrap_or("Unknown"),
            context.total_amount.as_deref().unwrap_or("Unknown"),
            context.currency.as_deref().unwrap_or("SEK"),
            context.vat_amount.as_deref().unwrap_or("0"),
            context.vat_rate.as_deref().unwrap_or("Unknown"),
            context.extracted_text.as_deref().unwrap_or("")
        )
    }

    /// Insert or update invoice record
    async fn create_or_update_invoice(
        &self,
        document_id: i64,
        context: &DocumentContext,
    ) -> AppResult<i64> {
        // Check if invoice exists
        let existing: Option<InvoiceRow> =
            sqlx::query_as("SELECT id FROM invoices WHERE document_id = $1")
                .bind(document_id)
                .fetch_optional(&self.context.pool)
                .await?;

        let supplier_name = context
            .supplier_name
            .clone()
            .unwrap_or_else(|| "Unknown".to_string());
        let invoice_number = context
            .invoice_number
            .clone()
            .unwrap_or_else(|| "Unknown".to_string());
        let invoice_date_str = context
            .transaction_date
            .clone()
            .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
        let invoice_date = chrono::NaiveDate::parse_from_str(&invoice_date_str, "%Y-%m-%d")
            .unwrap_or_else(|_| chrono::Local::now().naive_local().date());
        let total_amount: f64 = context
            .total_amount
            .as_ref()
            .and_then(|value| value.replace(',', ".").parse().ok())
            .unwrap_or(0.0);
        let vat_amount: f64 = context
            .vat_amount
            .as_ref()
            .and_then(|value| value.replace(',', ".").parse().ok())
            .unwrap_or(0.0);

        if let Some(row) = existing {
            let invoice_id = row.id.ok_or_else(|| {
                AppError::Agent("existing invoice row did not include an id".to_string())
            })?;
            sqlx::query(
                r#"
                UPDATE invoices 
                SET supplier_name = $2,
                    invoice_number = $3,
                    invoice_date = $4,
                    total_amount = $5,
                    vat_amount = $6,
                    status = 'PROCESSING_ANALYSIS',
                    updated_at = CURRENT_TIMESTAMP
                WHERE id = $1
                "#,
            )
            .bind(invoice_id)
            .bind(supplier_name)
            .bind(invoice_number)
            .bind(invoice_date)
            .bind(total_amount)
            .bind(vat_amount)
            .execute(&self.context.pool)
            .await?;
            return Ok(invoice_id);
        }

        let invoice_id = sqlx::query_scalar(
            r#"
            INSERT INTO invoices 
            (document_id, supplier_name, invoice_number, invoice_date, 
             total_amount, vat_amount, currency, status, bas_year)
            VALUES ($1, $2, $3, $4, $5, $6, 'SEK', 'PENDING_ANALYSIS', 2026)
            RETURNING id
            "#,
        )
        .bind(document_id)
        .bind(supplier_name)
        .bind(invoice_number)
        .bind(invoice_date)
        .bind(total_amount)
        .bind(vat_amount)
        .fetch_one(&self.context.pool)
        .await?;

        Ok(invoice_id)
    }

    /// Store account assignments
    async fn store_account_assignments(
        &self,
        invoice_id: i64,
        assignments: &[AccountAssignment],
    ) -> AppResult<()> {
        for (idx, assignment) in assignments.iter().enumerate() {
            sqlx::query(
                r#"
                INSERT INTO account_assignments
                (invoice_id, account_code, account_name, description, 
                 amount, vat_code, confidence, reasoning, sort_order)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                "#,
            )
            .bind(invoice_id)
            .bind(&assignment.account_code)
            .bind(&assignment.account_name)
            .bind(&assignment.description)
            .bind(assignment.amount)
            .bind(&assignment.vat_code)
            .bind(assignment.confidence)
            .bind(&assignment.reasoning)
            .bind(idx as i32)
            .execute(&self.context.pool)
            .await?;
        }

        Ok(())
    }

    async fn upsert_accounting_decision(
        &self,
        document_id: i64,
        assignments: &[AccountAssignment],
        total_amount: f64,
        vat_amount: f64,
        warnings: &[String],
        thinking_text: Option<&str>,
    ) -> AppResult<()> {
        let primary_assignment = assignments
            .iter()
            .find(|assignment| assignment.account_code != "1930")
            .or_else(|| assignments.first());

        let assigned_account_code =
            primary_assignment.map(|assignment| assignment.account_code.clone());
        let account_name = primary_assignment.map(|assignment| assignment.account_name.clone());
        let reasoning_text = if warnings.is_empty() {
            assignments
                .iter()
                .map(|assignment| assignment.reasoning.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        } else {
            format!(
                "{} | warnings: {}",
                assignments
                    .iter()
                    .map(|assignment| assignment.reasoning.as_str())
                    .collect::<Vec<_>>()
                    .join(" | "),
                warnings.join("; ")
            )
        };
        let net_amount = if vat_amount > 0.0 {
            Some((total_amount - vat_amount).max(0.0))
        } else {
            None
        };
        let avg_confidence = if assignments.is_empty() {
            0.0
        } else {
            assignments
                .iter()
                .map(|assignment| assignment.confidence)
                .sum::<f64>()
                / assignments.len() as f64
        };

        sqlx::query(
            r#"
            INSERT INTO accounting_decisions
            (document_id, reasoning_text, assigned_account_code, account_name, vat_amount,
             net_amount, gross_amount, ai_confidence, model_used, thinking_text, prompt_path, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(document_id)
        .bind(reasoning_text)
        .bind(assigned_account_code)
        .bind(account_name)
        .bind(if vat_amount > 0.0 {
            Some(vat_amount)
        } else {
            None
        })
        .bind(net_amount)
        .bind(total_amount)
        .bind(avg_confidence)
        .bind(&self.model)
        .bind(thinking_text)
        .bind(&self.prompt_path)
        .execute(&self.context.pool)
        .await?;

        Ok(())
    }

    /// Validate that the posting is internally balanced and matches the invoice total
    fn validate_balance(entries: &[AccountAssignment], total_amount: f64) -> Vec<String> {
        let mut warnings = Vec::new();
        let debit_total: f64 = entries
            .iter()
            .filter(|entry| entry.amount > 0.0 && entry.account_code != "1930")
            .map(|entry| entry.amount)
            .sum();
        let credit_total: f64 = entries
            .iter()
            .filter(|entry| entry.account_code == "1930")
            .map(|entry| entry.amount)
            .sum();

        let balanced = (debit_total - credit_total).abs() <= 0.01;
        if !balanced {
            warnings.push(format!(
                "Debit total ({:.2}) doesn't match credit total ({:.2})",
                debit_total, credit_total
            ));
        }

        let posting_total = if debit_total > 0.0 {
            debit_total
        } else {
            credit_total
        };
        if posting_total > 0.0 && (posting_total - total_amount).abs() > 0.01 {
            warnings.push(format!(
                "Posting total ({:.2}) doesn't match invoice total ({:.2})",
                posting_total, total_amount
            ));
        }

        // Check for missing VAT code on expense accounts
        for entry in entries {
            if entry.account_code.starts_with("4") && entry.vat_code.is_none() {
                warnings.push(format!(
                    "Expense entry {} ({}) has no VAT code assigned",
                    entry.account_code, entry.account_name
                ));
            }
        }

        warnings
    }
}

#[async_trait::async_trait]
impl Agent for AccountantAgent {
    type Input = AccountantInput;
    type Output = AccountantOutput;
    type Error = AppError;

    fn name(&self) -> &'static str {
        "AccountantAgent"
    }

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        info!(
            document_id = %input.document_id,
            "Starting accountant analysis"
        );

        // Gather context
        let context = self.gather_context(input.document_id).await?;
        let invoice_id = self
            .create_or_update_invoice(input.document_id, &context)
            .await?;

        let system_prompt = self.build_system_prompt();
        let user_prompt = self.build_user_prompt(&context);

        // Call LLM
        info!(
            document_id = %input.document_id,
            model = %self.model,
            prompt_path = %self.prompt_path,
            "Calling accountant model"
        );

        let response = self
            .inference_provider
            .chat_json(ChatJsonRequest {
                model: self.model.clone(),
                messages: vec![
                    ChatMessage {
                        role: "system".to_string(),
                        content: system_prompt,
                    },
                    ChatMessage {
                        role: "user".to_string(),
                        content: user_prompt,
                    },
                ],
                think: true,
            })
            .await?;

        info!(
            document_id = %input.document_id,
            response_len = response.content.len(),
            thinking_len = response.thinking.as_ref().map(|text| text.len()).unwrap_or(0),
            "Accountant model response received"
        );

        // Parse JSON with enhanced error handling
        let json = extract_json_from_response(&response.content).map_err(|e| {
            warn!(
                document_id = %input.document_id,
                error = %e,
                raw_response = %response.content.chars().take(2000).collect::<String>(),
                "Failed to extract JSON from model response"
            );
            AppError::Serialization(format!("JSON extraction failed: {}", e))
        })?;

        // Try to parse the entry, with fallback to partial data on failure
        let entry: AccountingEntry = match serde_json::from_value(json.clone()) {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    document_id = %input.document_id,
                    error = %e,
                    json = %serde_json::to_string_pretty(&json).unwrap_or_default().chars().take(2000).collect::<String>(),
                    "Failed to parse accounting entry, attempting fallback extraction"
                );

                // Fallback: try to extract entries array from malformed JSON
                if let Some(entries_array) = json.get("entries").and_then(|v| v.as_array()) {
                    let mut entries = Vec::new();
                    for entry_val in entries_array {
                        if let Ok(konto_entry) =
                            serde_json::from_value::<KontoEntry>(entry_val.clone())
                        {
                            entries.push(konto_entry);
                        }
                    }
                    AccountingEntry {
                        entry_date: json
                            .get("entry_date")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string()),
                        description: json
                            .get("description")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_default(),
                        entries,
                        verified: json
                            .get("verified")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        warnings: json
                            .get("warnings")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    }
                } else {
                    return Err(AppError::Serialization(format!(
                        "Failed to parse accounting entry: {}. Raw JSON: {}",
                        e,
                        json.to_string().chars().take(500).collect::<String>()
                    )));
                }
            }
        };

        // Convert to assignments
        let total_amount: f64 = context
            .total_amount
            .as_ref()
            .and_then(|s| s.replace(',', ".").parse().ok())
            .unwrap_or(0.0);

        let mut assignments = Vec::new();
        let mut credit_found = false;

        for konto_entry in entry.entries {
            if konto_entry.kredit > 0.0 {
                credit_found = true;
            }

            let is_debit = konto_entry.debet > 0.0;
            let amount = if is_debit {
                konto_entry.debet
            } else {
                konto_entry.kredit
            };

            assignments.push(AccountAssignment {
                account_code: konto_entry.account_code.clone(),
                account_name: konto_entry.description.clone(),
                description: konto_entry.description,
                amount,
                vat_code: None,   // Could parse from response
                confidence: 0.85, // LLM should provide this
                reasoning: format!("Matched to BAS 2026 account {}", konto_entry.account_code),
            });
        }

        // Check balance
        if !credit_found {
            warn!(
                document_id = %input.document_id,
                "No credit entry found in LLM output"
            );
        }

        let mut warnings = Self::validate_balance(&assignments, total_amount);
        warnings.extend(entry.warnings.clone());

        // Update invoice status
        let status = if warnings.is_empty() {
            "PENDING_REVIEW"
        } else {
            "PENDING_ANALYSIS"
        };

        sqlx::query(
            r#"
            UPDATE invoices 
            SET status = $1, updated_at = CURRENT_TIMESTAMP
            WHERE id = $2
            "#,
        )
        .bind(status)
        .bind(invoice_id)
        .execute(&self.context.pool)
        .await?;

        // Store assignments
        self.store_account_assignments(invoice_id, &assignments)
            .await?;
        self.upsert_accounting_decision(
            input.document_id,
            &assignments,
            total_amount,
            context
                .vat_amount
                .as_ref()
                .and_then(|value| value.replace(',', ".").parse().ok())
                .unwrap_or(0.0),
            &warnings,
            response.thinking.as_deref(),
        )
        .await?;

        document_state::complete_accountant_review(&self.context.pool, input.document_id).await?;
        self.context
            .record_document_event(
                input.document_id,
                "STATUS_CHANGED",
                serde_json::json!({
                    "accounting_status": "VALIDATING",
                    "run_kind": "VALIDATION"
                }),
            )
            .await?;
        self.context
            .record_document_event(
                input.document_id,
                "ACCOUNTANT_COMPLETED",
                serde_json::json!({
                    "invoice_id": invoice_id,
                    "assignments": assignments.len(),
                    "warnings": warnings,
                    "status": status,
                }),
            )
            .await?;

        info!(
            document_id = %input.document_id,
            invoice_id = %invoice_id,
            assignments = assignments.len(),
            warnings = warnings.len(),
            status = status,
            "Accountant analysis complete"
        );

        Ok(AccountantOutput {
            document_id: input.document_id,
            invoice_id: Some(invoice_id),
            account_assignments: assignments,
            status: status.to_string(),
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{AccountAssignment, AccountingEntry, DocumentContext, KontoEntry};

    fn raw_json_instruction(text: &str) -> String {
        format!(
            "{}\n\nCRITICAL: Output must be a raw JSON object starting with {{ and ending with }}. Do not use markdown fences. Do not prefix or suffix any text.",
            text.trim_end()
        )
    }

    fn build_user_prompt_for_test(context: &DocumentContext) -> String {
        format!(
            "Create the accounting entry for this document.\n\nDocument data:\n- Supplier: {}\n- Supplier org nr: {}\n- Transaction date: {}\n- Invoice number: {}\n- Total amount: {} {}\n- VAT amount: {}\n- VAT rate: {}\n- Extracted text: {}\n\nReturn only the JSON object. No markdown code fences. No leading or trailing text.",
            context.supplier_name.as_deref().unwrap_or("Unknown"),
            context.supplier_org_nr.as_deref().unwrap_or("Unknown"),
            context.transaction_date.as_deref().unwrap_or("Unknown"),
            context.invoice_number.as_deref().unwrap_or("Unknown"),
            context.total_amount.as_deref().unwrap_or("Unknown"),
            context.currency.as_deref().unwrap_or("SEK"),
            context.vat_amount.as_deref().unwrap_or("0"),
            context.vat_rate.as_deref().unwrap_or("Unknown"),
            context.extracted_text.as_deref().unwrap_or("")
        )
    }

    #[test]
    fn system_prompt_appends_no_fences_instruction() {
        let prompt = raw_json_instruction("base prompt");
        assert!(prompt.contains("base prompt"));
        assert!(prompt.contains("Do not use markdown fences"));
    }

    #[test]
    fn user_prompt_contains_document_context() {
        let context = DocumentContext {
            supplier_name: Some("ESPRESSO HOUSE".to_string()),
            supplier_org_nr: Some("556507-7160".to_string()),
            transaction_date: Some("2026-04-16".to_string()),
            invoice_number: None,
            total_amount: Some("104.00".to_string()),
            vat_amount: Some("11.14".to_string()),
            vat_rate: Some("12".to_string()),
            currency: Some("SEK".to_string()),
            extracted_text: None,
            line_items: None,
        };

        let prompt = build_user_prompt_for_test(&context);
        assert!(prompt.contains("ESPRESSO HOUSE"));
        assert!(prompt.contains("104.00 SEK"));
        assert!(prompt.contains("VAT amount: 11.14"));
        assert!(prompt.contains("Invoice number: Unknown"));
    }

    #[test]
    fn validate_balance_does_not_double_count_balanced_entry() {
        let entries = vec![
            AccountAssignment {
                account_code: "7690".to_string(),
                account_name: "Simple meals".to_string(),
                description: "Simple meals".to_string(),
                amount: 92.86,
                vat_code: None,
                confidence: 0.85,
                reasoning: "Matched to BAS 2026 account 7690".to_string(),
            },
            AccountAssignment {
                account_code: "2641".to_string(),
                account_name: "Input VAT".to_string(),
                description: "Input VAT".to_string(),
                amount: 11.14,
                vat_code: None,
                confidence: 0.85,
                reasoning: "Matched to BAS 2026 account 2641".to_string(),
            },
            AccountAssignment {
                account_code: "1930".to_string(),
                account_name: "Checking account".to_string(),
                description: "Checking account".to_string(),
                amount: 104.00,
                vat_code: None,
                confidence: 0.85,
                reasoning: "Matched to BAS 2026 account 1930".to_string(),
            },
        ];

        let warnings = super::AccountantAgent::validate_balance(&entries, 104.00);
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("doesn't match invoice total"))
        );
    }

    #[test]
    fn accounting_entry_defaults_missing_optional_fields() {
        let entry: AccountingEntry = serde_json::from_value(serde_json::json!({
            "entries": [
                {
                    "account_code": "1930",
                    "kredit": 104.0
                }
            ]
        }))
        .expect("missing optional model fields should default");

        assert!(!entry.entry_date.is_empty());
        assert_eq!(entry.description, "");
        assert!(!entry.verified);
        assert!(entry.warnings.is_empty());
        assert_eq!(entry.entries[0].account_code, "1930");
        assert_eq!(entry.entries[0].description, "");
        assert_eq!(entry.entries[0].debet, 0.0);
        assert_eq!(entry.entries[0].kredit, 104.0);
    }

    #[test]
    fn konto_entry_defaults_missing_amounts_to_zero() {
        let entry: KontoEntry = serde_json::from_value(serde_json::json!({
            "account_code": "7690",
            "description": "Other personnel costs"
        }))
        .expect("missing amounts should default");

        assert_eq!(entry.account_code, "7690");
        assert_eq!(entry.debet, 0.0);
        assert_eq!(entry.kredit, 0.0);
    }

    #[test]
    fn validate_balance_reports_imbalanced_and_total_mismatch() {
        let entries = vec![
            AccountAssignment {
                account_code: "7690".to_string(),
                account_name: "Simple meals".to_string(),
                description: "Simple meals".to_string(),
                amount: 90.0,
                vat_code: Some("2641".to_string()),
                confidence: 0.85,
                reasoning: "Matched".to_string(),
            },
            AccountAssignment {
                account_code: "1930".to_string(),
                account_name: "Checking account".to_string(),
                description: "Checking account".to_string(),
                amount: 100.0,
                vat_code: None,
                confidence: 0.85,
                reasoning: "Matched".to_string(),
            },
        ];

        let warnings = super::AccountantAgent::validate_balance(&entries, 104.0);

        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("doesn't match credit total"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("doesn't match invoice total"))
        );
    }

    #[test]
    fn validate_balance_reports_missing_vat_code_on_expense_accounts() {
        let entries = vec![AccountAssignment {
            account_code: "4010".to_string(),
            account_name: "Purchases".to_string(),
            description: "Purchases".to_string(),
            amount: 100.0,
            vat_code: None,
            confidence: 0.85,
            reasoning: "Matched".to_string(),
        }];

        let warnings = super::AccountantAgent::validate_balance(&entries, 100.0);

        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("has no VAT code assigned"))
        );
    }
}
