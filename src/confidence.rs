//! Confidence scoring module for Finelor
//!
//! Implements composite confidence scoring based on three agents:
//! - Vision Agent: 40%
//! - Accountant Agent: 40%
//! - Validator Agent: 20%

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::db::DbPool;

/// Weights for composite confidence calculation
const VISION_WEIGHT: f64 = 0.40;
const ACCOUNTANT_WEIGHT: f64 = 0.40;
const VALIDATOR_WEIGHT: f64 = 0.20;

/// Individual agent confidence scores
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfidence {
    pub vision_confidence: f64,
    pub accountant_confidence: f64,
    pub validator_confidence: f64,
}

impl AgentConfidence {
    /// Create a new confidence container
    pub fn new(
        vision_confidence: f64,
        accountant_confidence: f64,
        validator_confidence: f64,
    ) -> Self {
        Self {
            vision_confidence: vision_confidence.clamp(0.0, 1.0),
            accountant_confidence: accountant_confidence.clamp(0.0, 1.0),
            validator_confidence: validator_confidence.clamp(0.0, 1.0),
        }
    }
}

/// Composite confidence score with breakdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositeConfidence {
    pub overall_score: f64,
    pub vision_contribution: f64,
    pub accountant_contribution: f64,
    pub validator_contribution: f64,
    pub confidence_level: ConfidenceLevel,
}

/// Confidence level categories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConfidenceLevel {
    High,     // >= 0.85
    Medium,   // 0.70 - 0.85
    Low,      // 0.50 - 0.70
    Critical, // < 0.50
}

impl ConfidenceLevel {
    pub fn from_score(score: f64) -> Self {
        match score {
            s if s >= 0.85 => ConfidenceLevel::High,
            s if s >= 0.70 => ConfidenceLevel::Medium,
            s if s >= 0.50 => ConfidenceLevel::Low,
            _ => ConfidenceLevel::Critical,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ConfidenceLevel::High => "HIGH",
            ConfidenceLevel::Medium => "MEDIUM",
            ConfidenceLevel::Low => "LOW",
            ConfidenceLevel::Critical => "CRITICAL",
        }
    }

    /// Returns true if human review is required
    pub fn requires_human_review(&self) -> bool {
        matches!(self, ConfidenceLevel::Low | ConfidenceLevel::Critical)
    }

    /// Returns true if document should be quarantined
    pub fn should_quarantine(&self) -> bool {
        matches!(self, ConfidenceLevel::Critical)
    }
}

/// Confidence calculator
pub struct ConfidenceCalculator;

impl ConfidenceCalculator {
    /// Calculate composite confidence score
    ///
    /// Formula: (vision * 0.40) + (accountant * 0.40) + (validator * 0.20)
    pub fn calculate(agent_scores: &AgentConfidence) -> CompositeConfidence {
        let vision_contribution = agent_scores.vision_confidence * VISION_WEIGHT;
        let accountant_contribution = agent_scores.accountant_confidence * ACCOUNTANT_WEIGHT;
        let validator_contribution = agent_scores.validator_confidence * VALIDATOR_WEIGHT;

        let overall_score = vision_contribution + accountant_contribution + validator_contribution;

        CompositeConfidence {
            overall_score,
            vision_contribution,
            accountant_contribution,
            validator_contribution,
            confidence_level: ConfidenceLevel::from_score(overall_score),
        }
    }

    /// Calculate with validation penalties
    pub fn calculate_with_penalties(
        agent_scores: &AgentConfidence,
        validation_errors: &[ValidationError],
    ) -> CompositeConfidence {
        let mut base = Self::calculate(agent_scores);

        // Apply penalties for validation errors
        for error in validation_errors {
            let penalty = match error.severity {
                ErrorSeverity::Critical => 0.15,
                ErrorSeverity::Error => 0.08,
                ErrorSeverity::Warning => 0.02,
            };
            base.overall_score = (base.overall_score - penalty).max(0.0);
        }

        // Recalculate level after penalties
        base.confidence_level = ConfidenceLevel::from_score(base.overall_score);

        base
    }
}

/// Validation error with severity
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub field: String,
    pub severity: ErrorSeverity,
    pub message: String,
}

/// Error severity levels
#[derive(Debug, Clone, Copy)]
pub enum ErrorSeverity {
    Critical,
    Error,
    Warning,
}

/// Database helper for storing confidence scores
pub async fn store_composite_confidence(
    pool: &DbPool,
    document_id: i64,
    confidence: &CompositeConfidence,
) -> crate::error::AppResult<()> {
    sqlx::query(
        r#"
        INSERT INTO review_decisions
        (document_id, confidence_score, decision_type, human_review_required, review_reason, reviewed_at)
        VALUES ($1, $2, 'AUTO_CALCULATED', $3, $4, CURRENT_TIMESTAMP)
        ON CONFLICT (document_id) DO UPDATE SET
            confidence_score = EXCLUDED.confidence_score,
            decision_type = EXCLUDED.decision_type,
            human_review_required = EXCLUDED.human_review_required,
            review_reason = EXCLUDED.review_reason,
            reviewed_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(document_id)
    .bind(confidence.overall_score)
    .bind(confidence.confidence_level.requires_human_review())
    .bind(format!(
        "Vision: {:.2}, Accountant: {:.2}, Validator: {:.2}",
        confidence.vision_contribution / VISION_WEIGHT,
        confidence.accountant_contribution / ACCOUNTANT_WEIGHT,
        confidence.validator_contribution / VALIDATOR_WEIGHT
    ))
    .execute(pool)
    .await?;

    info!(
        document_id = %document_id,
        score = confidence.overall_score,
        level = %confidence.confidence_level.as_str(),
        "Stored composite confidence score"
    );

    Ok(())
}

/// Get confidence thresholds for review decisions
pub struct ReviewThresholds;

impl ReviewThresholds {
    /// Auto-approve threshold
    pub const AUTO_APPROVE: f64 = 0.75;

    /// Human review threshold (range)
    pub const HUMAN_REVIEW_MIN: f64 = 0.50;
    pub const HUMAN_REVIEW_MAX: f64 = 0.75;

    /// Quarantine threshold
    pub const QUARANTINE: f64 = 0.50;

    /// Determine action based on confidence score
    pub fn determine_action(score: f64) -> ReviewAction {
        match score {
            s if s >= Self::AUTO_APPROVE => ReviewAction::AutoApprove,
            s if s >= Self::HUMAN_REVIEW_MIN => ReviewAction::HumanReview,
            _ => ReviewAction::Quarantine,
        }
    }
}

/// Possible review actions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewAction {
    AutoApprove,
    HumanReview,
    Quarantine,
}

impl ReviewAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReviewAction::AutoApprove => "AUTO_APPROVE",
            ReviewAction::HumanReview => "HUMAN_REVIEW",
            ReviewAction::Quarantine => "QUARANTINE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_composite_calculation() {
        let scores = AgentConfidence::new(0.9, 0.85, 0.95);
        let composite = ConfidenceCalculator::calculate(&scores);

        assert!((composite.overall_score - 0.89).abs() < 0.001);
        assert_eq!(composite.confidence_level, ConfidenceLevel::High);
    }

    #[test]
    fn test_review_thresholds() {
        assert_eq!(
            ReviewThresholds::determine_action(0.80),
            ReviewAction::AutoApprove
        );
        assert_eq!(
            ReviewThresholds::determine_action(0.65),
            ReviewAction::HumanReview
        );
        assert_eq!(
            ReviewThresholds::determine_action(0.40),
            ReviewAction::Quarantine
        );
    }

    #[test]
    fn test_score_clamping() {
        let scores = AgentConfidence::new(1.5, -0.2, 2.0);
        assert_eq!(scores.vision_confidence, 1.0);
        assert_eq!(scores.accountant_confidence, 0.0);
        assert_eq!(scores.validator_confidence, 1.0);
    }
}
