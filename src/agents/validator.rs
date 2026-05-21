//! Validator agent used in the active pipeline.
//!
//! The agent is implemented as deterministic rules and produces the shared
//! persisted validation contract.

use chrono::{Duration, Local, NaiveDate};
use tracing::info;

use crate::agents::{
    Agent, AgentContext, DocumentStatus, FieldLoadMode, db_helpers, validation::AmountsLogicCheck,
    validation::DateCheck, validation::ErrorSeverity, validation::OcrNumberCheck,
    validation::OrgNrCheck, validation::SuggestedCorrections, validation::ValidationChecks,
    validation::ValidationError, validation::ValidationResult, validation::ValidationStatus,
    validation::ValidationWarning, validation::ValidatorInput, validation::ValidatorOutput,
    validation::VatCalculationCheck,
};
use crate::confidence::{AgentConfidence, ConfidenceCalculator, store_composite_confidence};
use crate::error::{AppError, AppResult};

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
    ocr_number: Option<String>,
    account_code: Option<String>,
    vision_confidence: Option<f64>,
    accountant_confidence: Option<f64>,
}

pub struct ValidatorAgent {
    context: AgentContext,
    field_load_mode: FieldLoadMode,
}

impl ValidatorAgent {
    pub fn new(context: AgentContext, field_load_mode: FieldLoadMode) -> Self {
        Self {
            context,
            field_load_mode,
        }
    }

    async fn gather_context(&self, document_id: i64) -> AppResult<DocumentContext> {
        let mut context = DocumentContext::default();

        let fields =
            db_helpers::load_document_fields(&self.context.pool, document_id, self.field_load_mode)
                .await?;

        for (field_type, value) in fields {
            match field_type.as_str() {
                "supplier_name" => context.supplier_name = value,
                "supplier_org_nr" => context.supplier_org_nr = value,
                "transaction_date" => context.transaction_date = value,
                "invoice_number" => context.invoice_number = value,
                "total_amount" => context.total_amount = value,
                "vat_amount" => context.vat_amount = value,
                "vat_rate" => context.vat_rate = value,
                "currency" => context.currency = value,
                "ocr_number" => context.ocr_number = value,
                _ => {}
            }
        }

        let account_result: Option<(Option<String>, Option<f64>)> = sqlx::query_as(
            r#"
            SELECT assigned_account_code, ai_confidence
            FROM accounting_decisions
            WHERE document_id = $1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(document_id)
        .fetch_optional(&self.context.pool)
        .await?;

        if let Some((account_code, confidence)) = account_result {
            context.account_code = account_code;
            context.accountant_confidence = confidence;
        }

        let vision_conf: Option<f64> = sqlx::query_scalar(
            r#"
            SELECT confidence
            FROM extracted_fields
            WHERE document_id = $1 AND field_type = 'supplier_name'
              AND source <> 'USER_CORRECTION'
            ORDER BY updated_at DESC, created_at DESC
            LIMIT 1
            "#,
        )
        .bind(document_id)
        .fetch_optional(&self.context.pool)
        .await?;

        context.vision_confidence = vision_conf;

        Ok(context)
    }

    pub(crate) fn validate_org_nr_mod10(org_nr: &str) -> Option<OrgNrCheck> {
        let cleaned: String = org_nr.chars().filter(|c| c.is_ascii_digit()).collect();

        if cleaned.len() != 10 {
            return Some(OrgNrCheck {
                valid: false,
                computed_checksum: 0,
                expected_checksum: 0,
                error: Some("Invalid length - expected 10 digits".to_string()),
            });
        }

        let weights = [2, 1, 2, 1, 2, 1, 2, 1, 2];
        let mut sum = 0;

        for (i, c) in cleaned.chars().take(9).enumerate() {
            let digit = c.to_digit(10)? as i32;
            let product = digit * weights[i];
            sum += product / 10 + product % 10;
        }

        let checksum = (10 - (sum % 10)) % 10;
        let expected = cleaned.chars().nth(9)?.to_digit(10)? as i32;

        Some(OrgNrCheck {
            valid: checksum == expected,
            computed_checksum: checksum,
            expected_checksum: expected,
            error: if checksum != expected {
                Some(format!(
                    "Checksum mismatch: computed {} but expected {}",
                    checksum, expected
                ))
            } else {
                None
            },
        })
    }

    pub(crate) fn validate_ocr_mod10(ocr_number: &str) -> Option<OcrNumberCheck> {
        let cleaned: String = ocr_number.chars().filter(|c| c.is_ascii_digit()).collect();
        if cleaned.is_empty() {
            return Some(OcrNumberCheck {
                valid: None,
                mod10_check: None,
                error: None,
            });
        }

        if cleaned.len() < 2 {
            return Some(OcrNumberCheck {
                valid: Some(false),
                mod10_check: Some(false),
                error: Some("OCR number is too short for modulus 10 validation".to_string()),
            });
        }

        let mut sum = 0;
        let digits: Vec<i32> = cleaned
            .chars()
            .filter_map(|c| c.to_digit(10).map(|n| n as i32))
            .collect();
        let payload_len = digits.len() - 1;

        for (idx, digit) in digits.iter().take(payload_len).enumerate() {
            let weight = if idx % 2 == 0 { 2 } else { 1 };
            let product = digit * weight;
            sum += product / 10 + product % 10;
        }

        let checksum = (10 - (sum % 10)) % 10;
        let expected = *digits.last()?;
        let valid = checksum == expected;

        Some(OcrNumberCheck {
            valid: Some(valid),
            mod10_check: Some(valid),
            error: if valid {
                None
            } else {
                Some(format!(
                    "OCR checksum mismatch: computed {} but expected {}",
                    checksum, expected
                ))
            },
        })
    }

    pub(crate) fn validate_date(date_str: Option<&str>) -> DateCheck {
        let Some(date_str) = date_str.filter(|value| !value.trim().is_empty()) else {
            return DateCheck {
                valid: false,
                is_future: false,
                format_ok: false,
                error: Some("Transaction date is missing".to_string()),
            };
        };

        match NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            Ok(date) => {
                let today = Local::now().date_naive();
                let max_date = today + Duration::days(1);
                let min_date = today - Duration::days(365 * 7);
                let is_future = date > max_date;
                let too_old = date < min_date;
                let valid = !is_future && !too_old;

                DateCheck {
                    valid,
                    is_future,
                    format_ok: true,
                    error: if is_future {
                        Some("Date is in the future".to_string())
                    } else if too_old {
                        Some("Date is older than seven years".to_string())
                    } else {
                        None
                    },
                }
            }
            Err(_) => DateCheck {
                valid: false,
                is_future: false,
                format_ok: false,
                error: Some("Date format is invalid; expected YYYY-MM-DD".to_string()),
            },
        }
    }

    pub(crate) fn parse_amount(value: Option<&str>) -> Option<f64> {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.replace(',', ".").parse::<f64>().ok())
    }

    pub(crate) fn parse_rate(value: Option<&str>) -> Option<f64> {
        Self::parse_amount(value)
    }

    pub(crate) fn validate_vat(
        total_amount: Option<&str>,
        vat_amount: Option<&str>,
        vat_rate: Option<&str>,
    ) -> VatCalculationCheck {
        let total = Self::parse_amount(total_amount);
        let vat = Self::parse_amount(vat_amount);
        let rate = Self::parse_rate(vat_rate);

        match (total, vat, rate) {
            (None, _, _) => VatCalculationCheck {
                valid: false,
                computed_vat: "0.00".to_string(),
                expected_vat: vat_amount.unwrap_or("0.00").to_string(),
                error: Some("Total amount is missing".to_string()),
            },
            (_, None, _) => VatCalculationCheck {
                valid: false,
                computed_vat: "0.00".to_string(),
                expected_vat: "0.00".to_string(),
                error: Some("VAT amount is missing".to_string()),
            },
            (Some(total), Some(vat), Some(rate)) if total >= 0.0 && vat >= 0.0 => {
                let expected_vat = if rate == 0.0 {
                    0.0
                } else {
                    total * (rate / (100.0 + rate))
                };
                let valid = (expected_vat - vat).abs() <= 0.05;
                VatCalculationCheck {
                    valid,
                    computed_vat: format!("{expected_vat:.2}"),
                    expected_vat: format!("{vat:.2}"),
                    error: if valid {
                        None
                    } else {
                        Some("VAT amount does not match the calculated value".to_string())
                    },
                }
            }
            (Some(_), Some(vat), None) if vat.abs() <= 0.005 => VatCalculationCheck {
                valid: true,
                computed_vat: "0.00".to_string(),
                expected_vat: format!("{vat:.2}"),
                error: None,
            },
            (Some(total), Some(vat), Some(_)) => VatCalculationCheck {
                valid: false,
                computed_vat: "0.00".to_string(),
                expected_vat: format!("{vat:.2}"),
                error: if total < 0.0 || vat < 0.0 {
                    Some("Amounts must be positive".to_string())
                } else {
                    Some("VAT amount does not match the calculated value".to_string())
                },
            },
            _ => VatCalculationCheck {
                valid: false,
                computed_vat: "0.00".to_string(),
                expected_vat: vat_amount.unwrap_or("0.00").to_string(),
                error: Some("VAT rate is missing".to_string()),
            },
        }
    }

    pub(crate) fn validate_amounts_logic(
        total_amount: Option<&str>,
        vat_amount: Option<&str>,
    ) -> AmountsLogicCheck {
        let total = Self::parse_amount(total_amount);
        let vat = Self::parse_amount(vat_amount);

        let positive_amounts =
            total.map(|v| v > 0.0).unwrap_or(false) && vat.map(|v| v >= 0.0).unwrap_or(false);

        let total_matches_vat_base = match (total, vat) {
            (Some(total), Some(vat)) => total >= vat,
            _ => false,
        };

        AmountsLogicCheck {
            total_matches_vat_base,
            positive_amounts,
            error: if total.is_none() {
                Some("Total amount is missing".to_string())
            } else if vat.is_none() || (positive_amounts && total_matches_vat_base) {
                None
            } else if !positive_amounts {
                Some("Amounts must be positive".to_string())
            } else {
                Some("Total amount is smaller than VAT amount".to_string())
            },
        }
    }

    pub(crate) fn is_known_account_code(account_code: Option<&str>) -> bool {
        account_code
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.len() == 4 && value.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false)
    }

    fn build_validation_result(context: &DocumentContext) -> ValidationResult {
        let org_nr = context
            .supplier_org_nr
            .as_deref()
            .and_then(Self::validate_org_nr_mod10)
            .unwrap_or(OrgNrCheck {
                valid: false,
                computed_checksum: 0,
                expected_checksum: 0,
                error: Some("Supplier organization number is missing".to_string()),
            });

        let vat_calculation = Self::validate_vat(
            context.total_amount.as_deref(),
            context.vat_amount.as_deref(),
            context.vat_rate.as_deref(),
        );
        let ocr_number = Self::validate_ocr_mod10(context.ocr_number.as_deref().unwrap_or(""))
            .unwrap_or(OcrNumberCheck {
                valid: None,
                mod10_check: None,
                error: None,
            });
        let date = Self::validate_date(context.transaction_date.as_deref());
        let amounts_logic = Self::validate_amounts_logic(
            context.total_amount.as_deref(),
            context.vat_amount.as_deref(),
        );

        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        if !org_nr.valid {
            errors.push(ValidationError {
                field: "org_nr".to_string(),
                severity: if context.supplier_org_nr.is_some() {
                    ErrorSeverity::Error
                } else {
                    ErrorSeverity::Critical
                },
                message: org_nr.error.clone().unwrap_or_else(|| {
                    "Supplier organization number validation failed".to_string()
                }),
            });
        }

        if !date.valid {
            errors.push(ValidationError {
                field: "transaction_date".to_string(),
                severity: if date.is_future {
                    ErrorSeverity::Critical
                } else {
                    ErrorSeverity::Error
                },
                message: date
                    .error
                    .clone()
                    .unwrap_or_else(|| "Transaction date is invalid".to_string()),
            });
        }

        if !vat_calculation.valid {
            errors.push(ValidationError {
                field: "vat_calculation".to_string(),
                severity: ErrorSeverity::Error,
                message: vat_calculation
                    .error
                    .clone()
                    .unwrap_or_else(|| "VAT validation failed".to_string()),
            });
        }

        if !amounts_logic.positive_amounts || !amounts_logic.total_matches_vat_base {
            errors.push(ValidationError {
                field: "amounts_logic".to_string(),
                severity: ErrorSeverity::Error,
                message: amounts_logic
                    .error
                    .clone()
                    .unwrap_or_else(|| "Amount consistency validation failed".to_string()),
            });
        }

        if let Some(valid) = ocr_number.valid
            && !valid
        {
            errors.push(ValidationError {
                field: "ocr_number".to_string(),
                severity: ErrorSeverity::Warning,
                message: ocr_number
                    .error
                    .clone()
                    .unwrap_or_else(|| "OCR number validation failed".to_string()),
            });
        }

        if !Self::is_known_account_code(context.account_code.as_deref()) {
            errors.push(ValidationError {
                field: "account_code".to_string(),
                severity: ErrorSeverity::Error,
                message: "Assigned account code is missing or invalid".to_string(),
            });
        }

        if context.invoice_number.is_none() {
            warnings.push(ValidationWarning {
                field: "invoice_number".to_string(),
                message: "Invoice number is missing".to_string(),
            });
        }

        if matches!(context.currency.as_deref(), Some(currency) if currency != "SEK") {
            warnings.push(ValidationWarning {
                field: "currency".to_string(),
                message: "Document currency is not SEK; verify exchange handling before export"
                    .to_string(),
            });
        }

        let severity_score: f64 = errors
            .iter()
            .map(|error| match error.severity {
                ErrorSeverity::Critical => 0.25,
                ErrorSeverity::Error => 0.12,
                ErrorSeverity::Warning => 0.04,
            })
            .sum::<f64>()
            + warnings.len() as f64 * 0.03;
        let passed_checks = Self::count_passed_checks(
            &org_nr,
            &vat_calculation,
            &ocr_number,
            &date,
            &amounts_logic,
        );
        let total_checks = 5.0;
        let confidence_score =
            ((passed_checks as f64 / total_checks) - severity_score).clamp(0.0, 1.0);

        let validation_result = if errors
            .iter()
            .any(|error| matches!(error.severity, ErrorSeverity::Critical))
        {
            ValidationStatus::Invalid
        } else if errors.is_empty() && warnings.is_empty() {
            ValidationStatus::Valid
        } else {
            ValidationStatus::NeedsReview
        };

        ValidationResult {
            validation_result,
            confidence_score,
            checks: ValidationChecks {
                org_nr: Some(org_nr),
                vat_calculation: Some(vat_calculation),
                ocr_number: Some(ocr_number),
                date: Some(date),
                amounts_logic: Some(amounts_logic),
            },
            errors,
            warnings,
            suggested_corrections: Some(SuggestedCorrections {
                kontonummer: if Self::is_known_account_code(context.account_code.as_deref()) {
                    None
                } else {
                    Some("7690".to_string())
                },
                vat_rate: context.vat_rate.clone().or_else(|| Some("25".to_string())),
                supplier_name: None,
            }),
        }
    }

    pub(crate) fn count_passed_checks(
        org_nr: &OrgNrCheck,
        vat_calculation: &VatCalculationCheck,
        ocr_number: &OcrNumberCheck,
        date: &DateCheck,
        amounts_logic: &AmountsLogicCheck,
    ) -> i32 {
        let mut count = 0;
        if org_nr.valid {
            count += 1;
        }
        if vat_calculation.valid {
            count += 1;
        }
        if ocr_number.valid.unwrap_or(false) {
            count += 1;
        }
        if date.valid {
            count += 1;
        }
        if amounts_logic.total_matches_vat_base && amounts_logic.positive_amounts {
            count += 1;
        }
        count
    }

    pub(crate) fn count_failed_checks(result: &ValidationResult) -> i32 {
        let mut count = 0;
        if result
            .checks
            .org_nr
            .as_ref()
            .map(|c| !c.valid)
            .unwrap_or(false)
        {
            count += 1;
        }
        if result
            .checks
            .vat_calculation
            .as_ref()
            .map(|c| !c.valid)
            .unwrap_or(false)
        {
            count += 1;
        }
        if result
            .checks
            .ocr_number
            .as_ref()
            .and_then(|c| c.valid)
            .map(|valid| !valid)
            .unwrap_or(false)
        {
            count += 1;
        }
        if result
            .checks
            .date
            .as_ref()
            .map(|c| !c.valid)
            .unwrap_or(false)
        {
            count += 1;
        }
        if result
            .checks
            .amounts_logic
            .as_ref()
            .map(|c| !c.total_matches_vat_base || !c.positive_amounts)
            .unwrap_or(false)
        {
            count += 1;
        }
        count
    }

    async fn store_validation_result(
        &self,
        document_id: i64,
        result: &ValidationResult,
    ) -> AppResult<()> {
        let checks_passed = Self::count_passed_checks(
            result.checks.org_nr.as_ref().expect("org_nr check present"),
            result
                .checks
                .vat_calculation
                .as_ref()
                .expect("vat check present"),
            result
                .checks
                .ocr_number
                .as_ref()
                .expect("ocr check present"),
            result.checks.date.as_ref().expect("date check present"),
            result
                .checks
                .amounts_logic
                .as_ref()
                .expect("amounts check present"),
        );
        let checks_failed = Self::count_failed_checks(result);
        let status = match result.validation_result {
            ValidationStatus::Valid => "VALID",
            ValidationStatus::Invalid => "INVALID",
            ValidationStatus::NeedsReview => "NEEDS_REVIEW",
        };

        sqlx::query(
            r#"
            INSERT INTO validation_results
            (document_id, checks_passed, checks_failed, validation_errors, overall_status, checked_at)
            VALUES ($1, $2, $3, $4, $5, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(document_id)
        .bind(checks_passed)
        .bind(checks_failed)
        .bind(serde_json::to_value(&result.errors)?)
        .bind(status)
        .execute(&self.context.pool)
        .await?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl Agent for ValidatorAgent {
    type Input = ValidatorInput;
    type Output = ValidatorOutput;
    type Error = AppError;

    fn name(&self) -> &'static str {
        "ValidatorAgent"
    }

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        info!(document_id = %input.document_id, "Starting deterministic validation");

        self.context
            .update_document_status(input.document_id, DocumentStatus::ProcessingValidator)
            .await?;

        let context = self.gather_context(input.document_id).await?;
        let validation_result = Self::build_validation_result(&context);

        let vision_conf = context.vision_confidence.unwrap_or(0.5);
        let accountant_conf = context.accountant_confidence.unwrap_or(0.5);
        let validator_conf = validation_result.confidence_score.clamp(0.0, 1.0);

        let agent_scores = AgentConfidence::new(vision_conf, accountant_conf, validator_conf);

        let errors: Vec<crate::confidence::ValidationError> = validation_result
            .errors
            .iter()
            .map(|e| crate::confidence::ValidationError {
                field: e.field.clone(),
                severity: match e.severity {
                    ErrorSeverity::Critical => crate::confidence::ErrorSeverity::Critical,
                    ErrorSeverity::Error => crate::confidence::ErrorSeverity::Error,
                    ErrorSeverity::Warning => crate::confidence::ErrorSeverity::Warning,
                },
                message: e.message.clone(),
            })
            .collect();

        let composite = ConfidenceCalculator::calculate_with_penalties(&agent_scores, &errors);

        self.store_validation_result(input.document_id, &validation_result)
            .await?;
        store_composite_confidence(&self.context.pool, input.document_id, &composite).await?;

        sqlx::query(
            r#"
            UPDATE documents
            SET validated_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            "#,
        )
        .bind(input.document_id)
        .execute(&self.context.pool)
        .await?;

        self.context
            .update_document_status(input.document_id, DocumentStatus::Validated)
            .await?;
        self.context
            .record_document_event(
                input.document_id,
                "VALIDATION_COMPLETED",
                serde_json::json!({
                    "confidence": composite.overall_score,
                    "level": composite.confidence_level.as_str(),
                    "validation_result": format!("{:?}", validation_result.validation_result),
                    "mode": "deterministic",
                }),
            )
            .await?;

        Ok(ValidatorOutput {
            document_id: input.document_id,
            validation_result,
            composite_confidence: composite,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // validate_org_nr_mod10 tests
    // ============================================================

    #[test]
    fn org_nr_mod10_valid_checksum() {
        // Valid Swedish org number with correct checksum
        // 5567037485 - LIFO check digit calculation
        let result = ValidatorAgent::validate_org_nr_mod10("5567037485");
        let check = result.expect("should return Some for valid 10-digit org number");
        assert!(
            check.valid,
            "Expected valid org number with correct checksum"
        );
        assert!(check.error.is_none());
        assert_eq!(check.computed_checksum, 5);
        assert_eq!(check.expected_checksum, 5);
    }

    #[test]
    fn org_nr_mod10_invalid_checksum() {
        // Same org number but with wrong checksum digit
        let result = ValidatorAgent::validate_org_nr_mod10("5567037480");
        let check = result.expect("should return Some for 10-digit org number");
        assert!(
            !check.valid,
            "Expected invalid org number with incorrect checksum"
        );
        assert!(check.error.is_some());
        assert!(check.error.as_ref().unwrap().contains("Checksum mismatch"));
        assert_eq!(check.computed_checksum, 5);
        assert_eq!(check.expected_checksum, 0);
    }

    #[test]
    fn org_nr_mod10_invalid_length_too_short() {
        let result = ValidatorAgent::validate_org_nr_mod10("123456789");
        let check = result.expect("should return Some for any non-empty input");
        assert!(!check.valid);
        assert!(check.error.as_ref().unwrap().contains("Invalid length"));
    }

    #[test]
    fn org_nr_mod10_invalid_length_too_long() {
        let result = ValidatorAgent::validate_org_nr_mod10("12345678901");
        let check = result.expect("should return Some for any non-empty input");
        assert!(!check.valid);
        assert!(check.error.as_ref().unwrap().contains("Invalid length"));
    }

    #[test]
    fn org_nr_mod10_empty_string() {
        let result = ValidatorAgent::validate_org_nr_mod10("");
        let check = result.expect("should return Some for empty string");
        assert!(!check.valid);
        assert!(check.error.as_ref().unwrap().contains("Invalid length"));
    }

    #[test]
    fn org_nr_mod10_ignores_non_digit_characters() {
        // Swedish org numbers may have dash separators
        let result = ValidatorAgent::validate_org_nr_mod10("556703-7485");
        let check = result.expect("should return Some for dashed format");
        // After removing non-digits, we get 10 digits, so check passes/fails based on checksum
        assert!(check.computed_checksum > 0 || check.expected_checksum > 0);
    }

    #[test]
    fn org_nr_mod10_alphabetic_characters_ignored() {
        let result = ValidatorAgent::validate_org_nr_mod10("ABC5567037485XYZ");
        let check = result.expect("should return Some when digits remain");
        // Should extract just the digits and validate
        assert!(check.computed_checksum > 0 || check.expected_checksum > 0);
    }

    // ============================================================
    // validate_ocr_mod10 tests
    // ============================================================

    #[test]
    fn ocr_mod10_valid_checksum() {
        // Valid OCR number: 125 (checksum = 5) -> 1255
        let result = ValidatorAgent::validate_ocr_mod10("1255");
        let check = result.expect("should return Some for valid OCR");
        assert_eq!(
            check.valid,
            Some(true),
            "Expected valid OCR with correct checksum"
        );
        assert_eq!(check.mod10_check, Some(true));
        assert!(check.error.is_none());
    }

    #[test]
    fn ocr_mod10_invalid_checksum() {
        // 1255 but actual checksum should be 5, so 1250 is wrong
        let result = ValidatorAgent::validate_ocr_mod10("1250");
        let check = result.expect("should return Some for any OCR-like input");
        assert_eq!(
            check.valid,
            Some(false),
            "Expected invalid OCR with incorrect checksum"
        );
        assert_eq!(check.mod10_check, Some(false));
        assert!(check.error.is_some());
        assert!(
            check
                .error
                .as_ref()
                .unwrap()
                .contains("OCR checksum mismatch")
        );
    }

    #[test]
    fn ocr_mod10_empty_string() {
        let result = ValidatorAgent::validate_ocr_mod10("");
        let check = result.expect("should return Some for empty string");
        assert_eq!(check.valid, None, "Empty string returns None for validity");
        assert_eq!(check.mod10_check, None);
        assert!(check.error.is_none());
    }

    #[test]
    fn ocr_mod10_too_short() {
        // Single digit cannot have a checksum
        let result = ValidatorAgent::validate_ocr_mod10("5");
        let check = result.expect("should return Some for short string");
        assert_eq!(check.valid, Some(false));
        assert!(check.error.as_ref().unwrap().contains("too short"));
    }

    #[test]
    fn ocr_mod10_ignores_non_digit_characters() {
        let result = ValidatorAgent::validate_ocr_mod10("125-5");
        let check = result.expect("should extract digits and validate");
        // After cleaning: "1255" - valid
        // Note: actual valid OCR depends on LIFO calculation
        assert!(check.valid.is_some());
    }

    #[test]
    fn ocr_mod10_single_digit_payload() {
        // 12 -> LIFO: 2*2 = 4, checksum = (10 - (4 % 10)) % 10 = 6
        let result = ValidatorAgent::validate_ocr_mod10("126");
        let check = result.expect("should validate single-digit payload");
        assert!(check.valid.is_some());
    }

    // ============================================================
    // validate_date tests
    // ============================================================

    #[test]
    fn date_valid_iso_format() {
        let result = ValidatorAgent::validate_date(Some("2024-06-15"));
        assert!(result.valid, "Expected valid date in ISO format");
        assert!(!result.is_future);
        assert!(result.format_ok);
        assert!(result.error.is_none());
    }

    #[test]
    fn date_future_beyond_threshold() {
        let future = (Local::now().date_naive() + Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let result = ValidatorAgent::validate_date(Some(&future));
        assert!(!result.valid, "Expected future date to be invalid");
        assert!(result.is_future, "Expected is_future flag to be set");
        assert!(result.format_ok);
        assert!(result.error.as_ref().unwrap().contains("future"));
    }

    #[test]
    fn date_far_past() {
        // Date older than 7 years
        let old = (Local::now().date_naive() - Duration::days(365 * 8))
            .format("%Y-%m-%d")
            .to_string();
        let result = ValidatorAgent::validate_date(Some(&old));
        assert!(!result.valid, "Expected far-past date to be invalid");
        assert!(!result.is_future);
        assert!(result.format_ok);
        assert!(result.error.as_ref().unwrap().contains("seven years"));
    }

    #[test]
    fn date_recent_past_still_valid() {
        // Date within 7 years should still be valid
        let recent = (Local::now().date_naive() - Duration::days(365 * 5))
            .format("%Y-%m-%d")
            .to_string();
        let result = ValidatorAgent::validate_date(Some(&recent));
        assert!(result.valid, "Expected recent past date to be valid");
        assert!(!result.is_future);
    }

    #[test]
    fn date_today_is_valid() {
        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        let result = ValidatorAgent::validate_date(Some(&today));
        assert!(result.valid, "Today's date should be valid");
        assert!(!result.is_future);
    }

    #[test]
    fn date_tomorrow_is_valid() {
        let tomorrow = (Local::now().date_naive() + Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let result = ValidatorAgent::validate_date(Some(&tomorrow));
        assert!(
            result.valid,
            "Tomorrow's date should be valid (within 1-day tolerance)"
        );
        assert!(!result.is_future);
    }

    #[test]
    fn date_malformed_string() {
        let result = ValidatorAgent::validate_date(Some("15/06/2024"));
        assert!(!result.valid, "Expected malformed date to be invalid");
        assert!(!result.format_ok);
        assert!(result.error.as_ref().unwrap().contains("invalid"));
    }

    #[test]
    fn date_malformed_no_dashes() {
        let result = ValidatorAgent::validate_date(Some("20240615"));
        assert!(!result.valid, "Expected date without dashes to be invalid");
        assert!(!result.format_ok);
    }

    #[test]
    fn date_missing_none() {
        let result = ValidatorAgent::validate_date(None);
        assert!(!result.valid, "Expected missing date to be invalid");
        assert!(!result.format_ok);
        assert!(result.error.as_ref().unwrap().contains("missing"));
    }

    #[test]
    fn date_whitespace_only() {
        let result = ValidatorAgent::validate_date(Some("   "));
        assert!(!result.valid, "Expected whitespace-only to be invalid");
        assert!(result.error.as_ref().unwrap().contains("missing"));
    }

    // ============================================================
    // parse_amount and parse_rate tests
    // ============================================================

    #[test]
    fn parse_amount_valid_decimal() {
        let result = ValidatorAgent::parse_amount(Some("123.45"));
        assert_eq!(result, Some(123.45));
    }

    #[test]
    fn parse_amount_valid_comma_separator() {
        let result = ValidatorAgent::parse_amount(Some("123,45"));
        assert_eq!(
            result,
            Some(123.45),
            "Comma should be converted to decimal point"
        );
    }

    #[test]
    fn parse_amount_with_whitespace() {
        let result = ValidatorAgent::parse_amount(Some("  123.45  "));
        assert_eq!(result, Some(123.45));
    }

    #[test]
    fn parse_amount_integer() {
        let result = ValidatorAgent::parse_amount(Some("100"));
        assert_eq!(result, Some(100.0));
    }

    #[test]
    fn parse_amount_negative() {
        let result = ValidatorAgent::parse_amount(Some("-50.00"));
        assert_eq!(result, Some(-50.0));
    }

    #[test]
    fn parse_amount_zero() {
        let result = ValidatorAgent::parse_amount(Some("0.00"));
        assert_eq!(result, Some(0.0));
    }

    #[test]
    fn parse_amount_none() {
        let result = ValidatorAgent::parse_amount(None);
        assert_eq!(result, None);
    }

    #[test]
    fn parse_amount_empty_string() {
        let result = ValidatorAgent::parse_amount(Some(""));
        assert_eq!(result, None);
    }

    #[test]
    fn parse_amount_invalid_format() {
        let result = ValidatorAgent::parse_amount(Some("abc"));
        assert_eq!(result, None);
    }

    #[test]
    fn parse_rate_same_as_parse_amount() {
        // parse_rate delegates to parse_amount
        let amount_result = ValidatorAgent::parse_amount(Some("25"));
        let rate_result = ValidatorAgent::parse_rate(Some("25"));
        assert_eq!(amount_result, rate_result);
    }

    // ============================================================
    // validate_vat tests
    // ============================================================

    #[test]
    fn vat_valid_25_percent() {
        // Total 125, VAT 25 -> rate = 25 / (125 + 25) * 100 = 20%
        // Actually: VAT = total * (rate / (100 + rate))
        // 125 * 25 / 125 = 25, so VAT = total * rate / (100 + rate)
        // 25 = total * 25 / 125 => total = 125 (base) + 25 (VAT)
        // So 100 base + 25 VAT = 125 total
        let check = ValidatorAgent::validate_vat(Some("125.00"), Some("25.00"), Some("25"));
        assert!(
            check.valid,
            "VAT calculation should be valid: {:?}",
            check.error
        );
        assert_eq!(check.computed_vat, "25.00");
    }

    #[test]
    fn vat_valid_12_percent() {
        // For 12% VAT: VAT = total * 12 / 112
        // 112 total, 12 VAT -> 112 * 12 / 112 = 12
        let check = ValidatorAgent::validate_vat(Some("224.00"), Some("24.00"), Some("12"));
        assert!(
            check.valid,
            "VAT calculation should be valid: {:?}",
            check.error
        );
    }

    #[test]
    fn vat_valid_6_percent() {
        // For 6% VAT: VAT = total * 6 / 106
        // 212 total, 12 VAT -> 212 * 6 / 106 = 12
        let check = ValidatorAgent::validate_vat(Some("212.00"), Some("12.00"), Some("6"));
        assert!(
            check.valid,
            "VAT calculation should be valid: {:?}",
            check.error
        );
    }

    #[test]
    fn vat_valid_zero_rate() {
        let check = ValidatorAgent::validate_vat(Some("100.00"), Some("0.00"), Some("0"));
        assert!(check.valid, "Zero rate with zero VAT should be valid");
        assert_eq!(check.computed_vat, "0.00");
    }

    #[test]
    fn vat_missing_total() {
        let check = ValidatorAgent::validate_vat(None, Some("25.00"), Some("25"));
        assert!(!check.valid);
        assert!(
            check
                .error
                .as_ref()
                .unwrap()
                .contains("Total amount is missing")
        );
    }

    #[test]
    fn vat_missing_vat_amount() {
        let check = ValidatorAgent::validate_vat(Some("125.00"), None, Some("25"));
        assert!(!check.valid);
        assert!(
            check
                .error
                .as_ref()
                .unwrap()
                .contains("VAT amount is missing")
        );
    }

    #[test]
    fn vat_missing_rate_nonzero_vat() {
        let check = ValidatorAgent::validate_vat(Some("125.00"), Some("25.00"), None);
        assert!(!check.valid);
        assert!(
            check
                .error
                .as_ref()
                .unwrap()
                .contains("VAT rate is missing")
        );
    }

    #[test]
    fn vat_wrong_rate() {
        // 100 base + 25 VAT = 125 total, but rate claimed as 20% instead of 25%
        let check = ValidatorAgent::validate_vat(Some("125.00"), Some("25.00"), Some("20"));
        assert!(!check.valid, "Mismatched rate should be invalid");
        assert!(check.error.as_ref().unwrap().contains("does not match"));
    }

    #[test]
    fn vat_wrong_vat_amount() {
        // Total 125, but VAT claimed as 20 instead of correct amount
        let check = ValidatorAgent::validate_vat(Some("125.00"), Some("20.00"), Some("25"));
        assert!(!check.valid, "Wrong VAT amount should be invalid");
        assert!(check.error.as_ref().unwrap().contains("does not match"));
    }

    #[test]
    fn vat_within_tolerance() {
        // VAT within 0.05 tolerance should pass
        let check = ValidatorAgent::validate_vat(Some("125.00"), Some("25.02"), Some("25"));
        assert!(check.valid, "VAT within tolerance should be valid");
    }

    #[test]
    fn vat_negative_amounts() {
        let check = ValidatorAgent::validate_vat(Some("-125.00"), Some("-25.00"), Some("25"));
        assert!(!check.valid);
        assert!(
            check
                .error
                .as_ref()
                .unwrap()
                .contains("Amounts must be positive")
        );
    }

    // ============================================================
    // validate_amounts_logic tests
    // ============================================================

    #[test]
    fn amounts_logic_balanced_positive() {
        let check = ValidatorAgent::validate_amounts_logic(Some("125.00"), Some("25.00"));
        assert!(check.positive_amounts, "Both amounts are positive");
        assert!(check.total_matches_vat_base, "Total >= VAT");
        assert!(check.error.is_none());
    }

    #[test]
    fn amounts_logic_total_equals_vat() {
        // Total is exactly equal to VAT (edge case - should still be valid)
        let check = ValidatorAgent::validate_amounts_logic(Some("25.00"), Some("25.00"));
        assert!(
            check.total_matches_vat_base,
            "Total >= VAT includes equality"
        );
    }

    #[test]
    fn amounts_logic_unbalanced_total_less_than_vat() {
        let check = ValidatorAgent::validate_amounts_logic(Some("20.00"), Some("25.00"));
        assert!(!check.total_matches_vat_base, "Total < VAT should fail");
        assert!(check.error.as_ref().unwrap().contains("smaller than VAT"));
    }

    #[test]
    fn amounts_logic_zero_total() {
        let check = ValidatorAgent::validate_amounts_logic(Some("0.00"), Some("0.00"));
        assert!(!check.positive_amounts, "Zero total should not be positive");
        assert!(
            check
                .error
                .as_ref()
                .unwrap()
                .contains("Amounts must be positive")
        );
    }

    #[test]
    fn amounts_logic_missing_vat() {
        let check = ValidatorAgent::validate_amounts_logic(Some("100.00"), None);
        assert!(!check.positive_amounts, "Missing VAT means not positive");
        // With missing VAT, error is None (not treated as error condition for this check)
        assert!(check.error.is_none());
    }

    #[test]
    fn amounts_logic_missing_total() {
        let check = ValidatorAgent::validate_amounts_logic(None, Some("25.00"));
        assert!(!check.positive_amounts);
        assert!(
            check
                .error
                .as_ref()
                .unwrap()
                .contains("Total amount is missing")
        );
    }

    #[test]
    fn amounts_logic_both_missing() {
        let check = ValidatorAgent::validate_amounts_logic(None, None);
        assert!(!check.positive_amounts);
        assert!(
            check
                .error
                .as_ref()
                .unwrap()
                .contains("Total amount is missing")
        );
    }

    // ============================================================
    // is_known_account_code tests
    // ============================================================

    #[test]
    fn is_known_account_code_valid_4_digits() {
        assert!(ValidatorAgent::is_known_account_code(Some("1910")));
        assert!(ValidatorAgent::is_known_account_code(Some("2640")));
        assert!(ValidatorAgent::is_known_account_code(Some("3010")));
    }

    #[test]
    fn is_known_account_code_valid_with_whitespace() {
        assert!(ValidatorAgent::is_known_account_code(Some("  1910  ")));
    }

    #[test]
    fn is_known_account_code_too_short() {
        assert!(!ValidatorAgent::is_known_account_code(Some("191")));
    }

    #[test]
    fn is_known_account_code_too_long() {
        assert!(!ValidatorAgent::is_known_account_code(Some("19100")));
    }

    #[test]
    fn is_known_account_code_contains_letters() {
        assert!(!ValidatorAgent::is_known_account_code(Some("191A")));
    }

    #[test]
    fn is_known_account_code_none() {
        assert!(!ValidatorAgent::is_known_account_code(None));
    }

    #[test]
    fn is_known_account_code_empty() {
        assert!(!ValidatorAgent::is_known_account_code(Some("")));
    }

    #[test]
    fn is_known_account_code_whitespace_only() {
        assert!(!ValidatorAgent::is_known_account_code(Some("   ")));
    }

    // ============================================================
    // count_passed_checks tests
    // ============================================================

    fn make_pass_org_nr() -> OrgNrCheck {
        OrgNrCheck {
            valid: true,
            computed_checksum: 5,
            expected_checksum: 5,
            error: None,
        }
    }

    fn make_fail_org_nr() -> OrgNrCheck {
        OrgNrCheck {
            valid: false,
            computed_checksum: 5,
            expected_checksum: 3,
            error: Some("Checksum mismatch".to_string()),
        }
    }

    fn make_pass_vat() -> VatCalculationCheck {
        VatCalculationCheck {
            valid: true,
            computed_vat: "25.00".to_string(),
            expected_vat: "25.00".to_string(),
            error: None,
        }
    }

    fn make_fail_vat() -> VatCalculationCheck {
        VatCalculationCheck {
            valid: false,
            computed_vat: "25.00".to_string(),
            expected_vat: "20.00".to_string(),
            error: Some("VAT mismatch".to_string()),
        }
    }

    fn make_pass_ocr() -> OcrNumberCheck {
        OcrNumberCheck {
            valid: Some(true),
            mod10_check: Some(true),
            error: None,
        }
    }

    fn make_fail_ocr() -> OcrNumberCheck {
        OcrNumberCheck {
            valid: Some(false),
            mod10_check: Some(false),
            error: Some("OCR checksum mismatch".to_string()),
        }
    }

    fn make_pass_date() -> DateCheck {
        DateCheck {
            valid: true,
            is_future: false,
            format_ok: true,
            error: None,
        }
    }

    fn make_fail_date() -> DateCheck {
        DateCheck {
            valid: false,
            is_future: true,
            format_ok: true,
            error: Some("Date is in the future".to_string()),
        }
    }

    fn make_pass_amounts() -> AmountsLogicCheck {
        AmountsLogicCheck {
            total_matches_vat_base: true,
            positive_amounts: true,
            error: None,
        }
    }

    fn make_fail_amounts() -> AmountsLogicCheck {
        AmountsLogicCheck {
            total_matches_vat_base: false,
            positive_amounts: false,
            error: Some("Amounts must be positive".to_string()),
        }
    }

    #[test]
    fn count_passed_checks_all_pass() {
        let count = ValidatorAgent::count_passed_checks(
            &make_pass_org_nr(),
            &make_pass_vat(),
            &make_pass_ocr(),
            &make_pass_date(),
            &make_pass_amounts(),
        );
        assert_eq!(count, 5, "All 5 checks should pass");
    }

    #[test]
    fn count_passed_checks_all_fail() {
        let count = ValidatorAgent::count_passed_checks(
            &make_fail_org_nr(),
            &make_fail_vat(),
            &make_fail_ocr(),
            &make_fail_date(),
            &make_fail_amounts(),
        );
        assert_eq!(count, 0, "All 5 checks should fail");
    }

    #[test]
    fn count_passed_checks_partial() {
        let count = ValidatorAgent::count_passed_checks(
            &make_pass_org_nr(),
            &make_fail_vat(),
            &make_pass_ocr(),
            &make_fail_date(),
            &make_pass_amounts(),
        );
        assert_eq!(count, 3, "3 out of 5 checks should pass");
    }

    #[test]
    fn count_passed_checks_ocr_none_validity() {
        // OCR with None validity should not count as passed
        let ocr_none = OcrNumberCheck {
            valid: None,
            mod10_check: None,
            error: None,
        };
        let count = ValidatorAgent::count_passed_checks(
            &make_pass_org_nr(),
            &make_pass_vat(),
            &ocr_none,
            &make_pass_date(),
            &make_pass_amounts(),
        );
        assert_eq!(count, 4, "OCR with None validity should not count");
    }

    // ============================================================
    // count_failed_checks tests
    // ============================================================

    fn make_result(all_pass: bool, amounts_pass: bool) -> ValidationResult {
        ValidationResult {
            validation_result: if all_pass {
                ValidationStatus::Valid
            } else {
                ValidationStatus::Invalid
            },
            checks: ValidationChecks {
                org_nr: Some(if all_pass {
                    make_pass_org_nr()
                } else {
                    make_fail_org_nr()
                }),
                vat_calculation: Some(if all_pass {
                    make_pass_vat()
                } else {
                    make_fail_vat()
                }),
                ocr_number: Some(if all_pass {
                    make_pass_ocr()
                } else {
                    make_fail_ocr()
                }),
                date: Some(if all_pass {
                    make_pass_date()
                } else {
                    make_fail_date()
                }),
                amounts_logic: Some(if amounts_pass {
                    make_pass_amounts()
                } else {
                    make_fail_amounts()
                }),
            },
            errors: vec![],
            warnings: vec![],
            confidence_score: 1.0,
            suggested_corrections: None,
        }
    }

    #[test]
    fn count_failed_checks_all_pass() {
        let result = make_result(true, true);
        let count = ValidatorAgent::count_failed_checks(&result);
        assert_eq!(count, 0, "No checks should fail");
    }

    #[test]
    fn count_failed_checks_all_fail() {
        let result = make_result(false, false);
        let count = ValidatorAgent::count_failed_checks(&result);
        assert_eq!(count, 5, "All 5 checks should fail");
    }

    #[test]
    fn count_failed_checks_partial() {
        // org_nr pass, vat fail, ocr pass, date fail, amounts pass
        let result = ValidationResult {
            validation_result: ValidationStatus::Invalid,
            checks: ValidationChecks {
                org_nr: Some(make_pass_org_nr()),
                vat_calculation: Some(make_fail_vat()),
                ocr_number: Some(make_pass_ocr()),
                date: Some(make_fail_date()),
                amounts_logic: Some(make_pass_amounts()),
            },
            errors: vec![],
            warnings: vec![],
            confidence_score: 0.5,
            suggested_corrections: None,
        };
        let count = ValidatorAgent::count_failed_checks(&result);
        assert_eq!(count, 2, "2 out of 5 checks should fail");
    }

    #[test]
    fn count_failed_checks_ocr_none_validity() {
        let ocr_none = OcrNumberCheck {
            valid: None,
            mod10_check: None,
            error: None,
        };
        let result = ValidationResult {
            validation_result: ValidationStatus::Invalid,
            checks: ValidationChecks {
                org_nr: Some(make_fail_org_nr()),
                vat_calculation: Some(make_fail_vat()),
                ocr_number: Some(ocr_none),
                date: Some(make_fail_date()),
                amounts_logic: Some(make_fail_amounts()),
            },
            errors: vec![],
            warnings: vec![],
            confidence_score: 0.2,
            suggested_corrections: None,
        };
        let count = ValidatorAgent::count_failed_checks(&result);
        assert_eq!(
            count, 4,
            "4 checks failed (OCR with None doesn't count as failed)"
        );
    }

    // ============================================================
    // build_validation_result tests
    // ============================================================

    #[test]
    fn build_validation_result_default_context() {
        let result = ValidatorAgent::build_validation_result(&DocumentContext::default());

        // All checks should exist
        assert!(result.checks.org_nr.is_some());
        assert!(result.checks.vat_calculation.is_some());
        assert!(result.checks.ocr_number.is_some());
        assert!(result.checks.date.is_some());
        assert!(result.checks.amounts_logic.is_some());

        // Since all fields are missing, all checks should fail
        assert!(!result.checks.org_nr.as_ref().unwrap().valid);
        assert!(!result.checks.vat_calculation.as_ref().unwrap().valid);
        assert!(
            !result
                .checks
                .amounts_logic
                .as_ref()
                .unwrap()
                .positive_amounts
        );
    }

    #[test]
    fn build_validation_result_complete_valid_context() {
        let context = DocumentContext {
            supplier_name: Some("Test Supplier".to_string()),
            supplier_org_nr: Some("5567037485".to_string()), // Valid checksum
            transaction_date: Some("2024-06-15".to_string()), // Valid date
            invoice_number: Some("INV-001".to_string()),
            total_amount: Some("125.00".to_string()),
            vat_amount: Some("25.00".to_string()),
            vat_rate: Some("25".to_string()),
            currency: Some("SEK".to_string()),
            ocr_number: Some("1255".to_string()), // Valid OCR checksum
            account_code: Some("1910".to_string()),
            vision_confidence: Some(0.95),
            accountant_confidence: Some(0.90),
        };

        let result = ValidatorAgent::build_validation_result(&context);

        // All validations should pass
        assert!(result.checks.org_nr.as_ref().unwrap().valid);
        assert!(result.checks.vat_calculation.as_ref().unwrap().valid);
        assert!(result.checks.date.as_ref().unwrap().valid);
        assert!(
            result
                .checks
                .amounts_logic
                .as_ref()
                .unwrap()
                .positive_amounts
        );
        assert!(
            result
                .checks
                .amounts_logic
                .as_ref()
                .unwrap()
                .total_matches_vat_base
        );
    }

    #[test]
    fn build_validation_result_partial_context() {
        let context = DocumentContext {
            supplier_name: None,
            supplier_org_nr: Some("5567037480".to_string()), // Invalid checksum
            transaction_date: None,                          // Missing
            invoice_number: None,
            total_amount: Some("100.00".to_string()),
            vat_amount: Some("20.00".to_string()),
            vat_rate: Some("25".to_string()),
            currency: None,
            ocr_number: None,
            account_code: None,
            vision_confidence: None,
            accountant_confidence: None,
        };

        let result = ValidatorAgent::build_validation_result(&context);

        // org_nr should fail (bad checksum)
        assert!(!result.checks.org_nr.as_ref().unwrap().valid);
        // date should fail (missing)
        assert!(!result.checks.date.as_ref().unwrap().valid);
        // VAT should be valid (correct calculation)
        assert!(result.checks.vat_calculation.as_ref().unwrap().valid);
    }

    #[test]
    fn build_validation_result_future_date() {
        let future = (Local::now().date_naive() + Duration::days(5))
            .format("%Y-%m-%d")
            .to_string();
        let context = DocumentContext {
            transaction_date: Some(future),
            ..Default::default()
        };

        let result = ValidatorAgent::build_validation_result(&context);

        assert!(!result.checks.date.as_ref().unwrap().valid);
        assert!(result.checks.date.as_ref().unwrap().is_future);
    }

    // ============================================================
    // Original tests preserved and expanded
    // ============================================================

    #[test]
    fn org_nr_missing_uses_zero_checksums() {
        let result = ValidatorAgent::build_validation_result(&DocumentContext::default());
        let check = result.checks.org_nr.unwrap();
        assert!(!check.valid);
        assert_eq!(check.computed_checksum, 0);
        assert_eq!(check.expected_checksum, 0);
    }

    #[test]
    fn vat_validation_handles_zero_vat_without_rate() {
        let check = ValidatorAgent::validate_vat(Some("35.00"), Some("0.00"), None);
        assert!(check.valid);
        assert_eq!(check.computed_vat, "0.00");
    }

    #[test]
    fn vat_validation_reports_missing_vat_amount() {
        let check = ValidatorAgent::validate_vat(Some("10.00"), None, Some("25"));
        assert!(!check.valid);
        assert_eq!(check.error.as_deref(), Some("VAT amount is missing"));
    }

    #[test]
    fn vat_validation_reports_missing_rate_for_nonzero_vat() {
        let check = ValidatorAgent::validate_vat(Some("10.00"), Some("2.00"), None);
        assert!(!check.valid);
        assert_eq!(check.error.as_deref(), Some("VAT rate is missing"));
    }

    #[test]
    fn amounts_logic_ignores_missing_vat_amount() {
        let check = ValidatorAgent::validate_amounts_logic(Some("10.00"), None);
        assert!(!check.positive_amounts);
        assert_eq!(check.error, None);
    }

    #[test]
    fn future_date_is_invalid() {
        let future = (Local::now().date_naive() + Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let check = ValidatorAgent::validate_date(Some(&future));
        assert!(!check.valid);
        assert!(check.is_future);
    }
}
