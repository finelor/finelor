//! Shared validation data contract for the deterministic validator pipeline.

use serde::{Deserialize, Deserializer, Serialize};

use crate::confidence::CompositeConfidence;

/// Input for validation.
#[derive(Debug, Clone)]
pub struct ValidatorInput {
    pub document_id: i64,
}

/// Output from validation.
#[derive(Debug, Clone)]
pub struct ValidatorOutput {
    pub document_id: i64,
    pub validation_result: ValidationResult,
    pub composite_confidence: CompositeConfidence,
}

/// Structured validation result persisted by the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ValidationResult {
    pub validation_result: ValidationStatus,
    pub confidence_score: f64,
    pub checks: ValidationChecks,
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_corrections: Option<SuggestedCorrections>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ValidationStatus {
    Valid,
    Invalid,
    NeedsReview,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ValidationChecks {
    pub org_nr: Option<OrgNrCheck>,
    pub vat_calculation: Option<VatCalculationCheck>,
    pub ocr_number: Option<OcrNumberCheck>,
    pub date: Option<DateCheck>,
    pub amounts_logic: Option<AmountsLogicCheck>,
}

#[cfg(test)]
impl ValidationChecks {
    fn count_passed(&self) -> i32 {
        let mut count = 0;
        if self.org_nr.as_ref().map(|c| c.valid).unwrap_or(false) {
            count += 1;
        }
        if self
            .vat_calculation
            .as_ref()
            .map(|c| c.valid)
            .unwrap_or(false)
        {
            count += 1;
        }
        if self
            .ocr_number
            .as_ref()
            .and_then(|c| c.valid)
            .unwrap_or(false)
        {
            count += 1;
        }
        if self.date.as_ref().map(|c| c.valid).unwrap_or(false) {
            count += 1;
        }
        if self
            .amounts_logic
            .as_ref()
            .map(|c| c.total_matches_vat_base && c.positive_amounts)
            .unwrap_or(false)
        {
            count += 1;
        }
        count
    }

    fn count_failed(&self) -> i32 {
        let mut count = 0;
        if self.org_nr.as_ref().map(|c| !c.valid).unwrap_or(false) {
            count += 1;
        }
        if self
            .vat_calculation
            .as_ref()
            .map(|c| !c.valid)
            .unwrap_or(false)
        {
            count += 1;
        }
        if self
            .ocr_number
            .as_ref()
            .and_then(|c| c.valid)
            .map(|valid| !valid)
            .unwrap_or(false)
        {
            count += 1;
        }
        if self.date.as_ref().map(|c| !c.valid).unwrap_or(false) {
            count += 1;
        }
        if self
            .amounts_logic
            .as_ref()
            .map(|c| !c.total_matches_vat_base || !c.positive_amounts)
            .unwrap_or(false)
        {
            count += 1;
        }
        count
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OrgNrCheck {
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub computed_checksum: i32,
    #[serde(default)]
    pub expected_checksum: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VatCalculationCheck {
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub computed_vat: String,
    #[serde(default)]
    pub expected_vat: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OcrNumberCheck {
    #[serde(default)]
    pub valid: Option<bool>,
    #[serde(default)]
    pub mod10_check: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DateCheck {
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub is_future: bool,
    #[serde(default)]
    pub format_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AmountsLogicCheck {
    #[serde(default)]
    pub total_matches_vat_base: bool,
    #[serde(default)]
    pub positive_amounts: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ValidationError {
    pub field: String,
    pub severity: ErrorSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ValidationWarning {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct SuggestedCorrections {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kontonummer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vat_rate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supplier_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub enum ErrorSeverity {
    Warning,
    Error,
    Critical,
}

impl<'de> Deserialize<'de> for ErrorSeverity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        match raw.trim().to_ascii_uppercase().as_str() {
            "WARNING" => Ok(Self::Warning),
            "ERROR" => Ok(Self::Error),
            "CRITICAL" => Ok(Self::Critical),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["WARNING", "ERROR", "CRITICAL"],
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_result_allows_nullable_ocr_boolean() {
        let json = serde_json::json!({
            "checks": {
                "amounts_logic": {
                    "error": null,
                    "positive_amounts": true,
                    "total_matches_vat_base": true
                },
                "date": {
                    "error": null,
                    "format_ok": true,
                    "is_future": false,
                    "valid": true
                },
                "ocr_number": {
                    "error": null,
                    "mod10_check": null,
                    "valid": null
                },
                "org_nr": {
                    "computed_checksum": 7,
                    "error": null,
                    "expected_checksum": 7,
                    "valid": true
                },
                "vat_calculation": {
                    "computed_vat": "12.50",
                    "error": null,
                    "expected_vat": "12.50",
                    "valid": true
                }
            },
            "confidence_score": 0.95,
            "errors": [],
            "suggested_corrections": {},
            "validation_result": "VALID",
            "warnings": []
        });

        let result: ValidationResult = serde_json::from_value(json).unwrap();

        assert_eq!(
            result
                .checks
                .ocr_number
                .as_ref()
                .and_then(|check| check.valid),
            None
        );
        assert_eq!(result.checks.count_passed(), 4);
        assert_eq!(result.checks.count_failed(), 0);
    }

    #[test]
    fn validation_result_accepts_uppercase_error_severity() {
        let json = serde_json::json!({
            "validation_result": "NEEDS_REVIEW",
            "confidence_score": 0.72,
            "checks": {},
            "errors": [
                {
                    "field": "org_nr",
                    "severity": "CRITICAL",
                    "message": "Checksum verification failed"
                }
            ],
            "warnings": [],
            "suggested_corrections": {}
        });

        let result: ValidationResult = serde_json::from_value(json).unwrap();
        assert!(matches!(
            result.errors.first().map(|error| &error.severity),
            Some(ErrorSeverity::Critical)
        ));
    }
}
