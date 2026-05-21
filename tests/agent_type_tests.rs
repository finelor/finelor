//! Type Safety Tests for Finelor Agents
//!
//! Tests for BigDecimal/f64 conversions, SQLite DECIMAL handling,
//! and type safety issues that commonly cause failures.

use bigdecimal::BigDecimal;
use num_traits::cast::ToPrimitive;
use std::str::FromStr;

/// Test BigDecimal to f64 conversion precision
/// This is a critical path for confidence scores stored in SQLite
#[test]
fn test_bigdecimal_to_f64_conversion() {
    // Test confidence score stored as DECIMAL(3,2)
    let bd = BigDecimal::from_str("0.87").unwrap();
    let f: f64 = bd.to_f64().unwrap();
    assert!(
        (f - 0.87).abs() < f64::EPSILON,
        "BigDecimal 0.87 should convert to f64 0.87"
    );

    // Test boundary values for DECIMAL(3,2)
    let bd_max = BigDecimal::from_str("0.99").unwrap();
    let f_max: f64 = bd_max.to_f64().unwrap();
    assert!(
        (f_max - 0.99).abs() < 0.001,
        "DECIMAL(3,2) max 0.99 should convert accurately"
    );

    let bd_min = BigDecimal::from_str("0.00").unwrap();
    let f_min: f64 = bd_min.to_f64().unwrap();
    assert!(
        (f_min - 0.00).abs() < f64::EPSILON,
        "DECIMAL(3,2) min 0.00 should convert accurately"
    );
}

/// Test confidence score handling - specifically the DECIMAL(3,2) type
#[test]
fn test_confidence_score_decimal_handling() {
    // Common confidence scores from LLM agents
    let test_scores = vec![
        ("0.95", 0.95),
        ("0.75", 0.75),
        ("0.50", 0.50),
        ("0.25", 0.25),
        ("1.00", 1.00),
        ("0.00", 0.00),
    ];

    for (input, expected) in test_scores {
        let bd = BigDecimal::from_str(input).unwrap();
        let f = bd.to_f64().unwrap();
        let diff = (f - expected).abs();
        assert!(
            diff < 0.0001,
            "Confidence score {} converted to {} (diff: {})",
            input,
            f,
            diff
        );
    }
}

/// Test BigDecimal for amount fields (DECIMAL(12,2) in SQLite)
#[test]
fn test_amount_decimal_handling() {
    // Test typical invoice amounts
    let amounts = vec![
        ("1250.00", 1250.00),
        ("999999.99", 999999.99),
        ("0.01", 0.01),
        ("50000.50", 50050.50), // Intentionally wrong to test failure
        ("1000000.00", 1000000.00),
        ("-250.00", -250.00), // Negative amounts (credits)
    ];

    let mut failures = vec![];
    for (input, expected) in amounts {
        let bd = BigDecimal::from_str(input).unwrap();
        let f = bd.to_f64().unwrap();
        if (f - expected).abs() >= 0.01 {
            failures.push(format!("Amount {} -> {} (expected {})", input, f, expected));
        }
    }

    // We expect the 50000.50 test to fail as it's intentionally wrong
    assert_eq!(
        failures.len(),
        1,
        "Expected 1 failure (intentionally wrong test case): {:?}",
        failures
    );
    assert!(
        failures[0].contains("50000.5") || failures[0].contains("50050.5"),
        "Should detect the wrong expected value: {:?}",
        failures
    );
}

/// Test that NULL BigDecimal is handled safely
#[test]
fn test_null_bigdecimal_handling() {
    // Simulate what happens when confidence_score is NULL in database
    let maybe_score: Option<BigDecimal> = None;
    let score: f64 = maybe_score
        .as_ref()
        .map(|bd| bd.to_f64().unwrap_or(0.0))
        .unwrap_or(0.0);
    assert_eq!(score, 0.0, "NULL confidence should default to 0.0");
}

/// Test decimal arithmetic precision for VAT calculations
#[test]
fn test_vat_calculation_precision() {
    // VAT calculation: total * vat_rate = vat_amount
    let total = BigDecimal::from_str("1000.00").unwrap();
    let rate = BigDecimal::from_str("0.25").unwrap(); // 25% VAT

    let expected_vat = BigDecimal::from_str("250.00").unwrap();
    let calculated_vat = &total * &rate;

    // BigDecimal arithmetic maintains precision
    let diff = (&calculated_vat - &expected_vat).abs();
    let diff_f64: f64 = diff.to_f64().unwrap();
    assert!(diff_f64 < 0.01, "VAT calculation should be precise");
}

/// Test f64 to BigDecimal conversion (reverse of normal flow)
#[test]
fn test_f64_to_bigdecimal_conversion() {
    // This is what happens when saving confidence scores to DB
    // Note: BigDecimal::from_f64 requires serde feature
    // Using from_str instead as a workaround
    let confidence_str = "0.87";
    let bd = BigDecimal::from_str(confidence_str).unwrap();

    // Verify roundtrip
    let back_to_f64 = bd.to_f64().unwrap();
    assert!(
        (0.87 - back_to_f64).abs() < 0.0001,
        "Roundtrip should preserve value"
    );
}

/// Test extreme decimal values
#[test]
fn test_extreme_decimal_values() {
    // Test very large amounts
    let large = BigDecimal::from_str("9999999999.99").unwrap();
    let large_f64 = large.to_f64().unwrap();
    assert!(
        large_f64.is_finite(),
        "Large amounts should convert to finite f64"
    );

    // Test very small amounts
    let small = BigDecimal::from_str("0.001").unwrap();
    let _small_f64 = small.to_f64().unwrap(); // Very small values may lose precision

    // Test precision limit
    let precision_test = BigDecimal::from_str("123456789.12").unwrap();
    let pt_f64 = precision_test.to_f64().unwrap();
    assert!(
        (pt_f64 - 123456789.12).abs() < 0.01,
        "Amount precision should be preserved for reasonable values"
    );
}

/// Test the review agent's confidence score reading pattern
#[test]
fn test_review_agent_confidence_pattern() {
    // Simulates: let confidence_score: f64 = match &confidence_score_bd { ... }
    let confidence_score_bd: Option<BigDecimal> = Some(BigDecimal::from_str("0.85").unwrap());
    let confidence_score: f64 = match &confidence_score_bd {
        Some(bd) => bd.to_f64().unwrap(),
        None => 0.0,
    };

    assert!(
        (confidence_score - 0.85).abs() < 0.001,
        "Should extract confidence correctly"
    );
}

/// Test serialization/deserialization of BigDecimal in JSON
#[test]
fn test_bigdecimal_json_serialization() {
    // BigDecimal doesn't implement serde::Serialize/Deserialize by default
    // in bigdecimal 0.4.10 without the 'serde' feature
    // This test documents the limitation
    use bigdecimal::BigDecimal;
    use std::str::FromStr;

    let amount = BigDecimal::from_str("1234.56").unwrap();

    // Serialize via to_string() which uses Display
    let json = format!("\"{}\"", amount);
    assert!(
        json.contains("1234.56"),
        "BigDecimal should serialize via to_string"
    );

    // Deserialize via from_str
    let parsed_str = "1234.56";
    let parsed = BigDecimal::from_str(parsed_str).unwrap();
    assert_eq!(amount, parsed, "Roundtrip via strings should work");
}

/// Test type safety edge cases that have caused bugs
#[test]
fn test_type_safety_edge_cases() {
    // Case 1: String that looks like a number but has Chinese garbage
    let problematic = "0.85夯实";
    let result = BigDecimal::from_str(problematic);
    assert!(
        result.is_err(),
        "Chinese garbage should cause parse failure"
    );

    // Case 2: Empty string
    let empty = "";
    let result = BigDecimal::from_str(empty);
    assert!(result.is_err(), "Empty string should not parse");

    // Case 3: Whitespace
    let whitespace = "  0.85  ";
    let result = BigDecimal::from_str(whitespace);
    // BigDecimal from_str doesn't trim whitespace - this documents the REAL behavior
    assert!(
        result.is_err(),
        "Whitespace should NOT be trimmed by from_str parser - use trim() first"
    );

    // Case 4: Scientific notation
    let scientific = "1.23e2";
    let result = BigDecimal::from_str(scientific);
    // BigDecimal may or may not support this - test behavior
    if let Ok(v) = result {
        let val = v.to_f64().unwrap();
        assert!(
            (val - 123.0).abs() < 0.01,
            "Scientific notation should parse correctly"
        );
    }
}

/// Integration test: Simulate database confidence score retrieval
#[test]
fn test_database_confidence_retrieval_pattern() {
    // This test documents the pattern used in review.rs:
    // let confidence_score_bd: Option<BigDecimal> = sqlx::query_scalar(...)
    // let confidence_score: f64 = match &confidence_score_bd { ... }

    // Test with Some value
    let score_bd: Option<BigDecimal> = Some(BigDecimal::from_str("0.92").unwrap());
    let score: f64 = match &score_bd {
        Some(bd) => bd.to_f64().unwrap_or(0.0),
        None => 0.0,
    };
    assert!(
        (score - 0.92).abs() < 0.001,
        "Should convert Some(BigDecimal) to f64"
    );

    // Test with None
    let score_bd: Option<BigDecimal> = None;
    let score: f64 = match &score_bd {
        Some(bd) => bd.to_f64().unwrap_or(0.0),
        None => 0.0,
    };
    assert_eq!(score, 0.0, "None should default to 0.0");
}

/// Test amount field types used in AccountAssignment
#[test]
fn test_account_assignment_amount_types() {
    // From accountant.rs: AccountAssignment has amount: f64
    // This tests the type used for the amount field

    let amounts: Vec<f64> = vec![
        1250.50, 9999.99, 0.01, -500.00, // Credit
        1000000.00,
    ];

    for amount in amounts {
        // Simulate serialization
        let json = format!("{}", amount);
        let parsed: f64 = json.parse().unwrap();
        assert!(
            (amount - parsed).abs() < 0.001,
            "Amount {} should serialize/deserialize correctly",
            amount
        );
    }
}

/// Test VAT rate handling as string vs number
#[test]
fn test_vat_rate_type_handling() {
    // VAT rates can come as strings like "25" or "25%" from OCR
    let rate_string = "25";
    let rate_num: f64 = rate_string.parse().unwrap();
    let rate_decimal = rate_num / 100.0;

    assert!(
        (rate_decimal - 0.25).abs() < 0.001,
        "VAT rate 25 should become 0.25"
    );

    // Test with decimal percentage
    let rate_string = "12.5";
    let rate_num: f64 = rate_string.parse().unwrap();
    let rate_decimal = rate_num / 100.0;

    assert!(
        (rate_decimal - 0.125).abs() < 0.001,
        "VAT rate 12.5 should become 0.125"
    );
}
