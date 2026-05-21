//! Tests for Review Agent Telegram message formatting.

use finelor::telegram::{
    MarkdownSegment, MarkdownStyle, escape_markdown, markdown_label_code_value,
    markdown_label_value, markdown_message,
};

/// Test that invoice numbers with # are properly escaped
#[test]
fn test_invoice_number_with_hash() {
    let invoice_number = "INV#12345";
    let escaped = escape_markdown(invoice_number);
    assert_eq!(
        escaped, "INV\\#12345",
        "Hash symbol must be escaped for Telegram MarkdownV2"
    );
}

/// Test that supplier names with special chars are escaped
#[test]
fn test_supplier_name_escaping() {
    let supplier = "A&B GmbH (Test)";
    let escaped = escape_markdown(supplier);
    assert_eq!(
        escaped, "A\\&B GmbH \\(Test\\)",
        "Special chars must be escaped"
    );
}

/// Test complete review message formatting
#[test]
fn test_review_message_formatting() {
    // Simulate building the message with various edge cases
    let invoice_number = "INV#2024-001";
    let supplier = "Test & Co. (Sweden)";

    let message = markdown_message(&[
        MarkdownSegment {
            text: "🔍 ",
            style: MarkdownStyle::Plain,
        },
        MarkdownSegment {
            text: "Human Review Required",
            style: MarkdownStyle::Bold,
        },
    ]);
    let message = format!(
        "{}\n{}\n{}",
        message,
        markdown_label_value("Supplier", supplier),
        markdown_label_value("Invoice #", invoice_number)
    );

    // Verify no unescaped special characters
    assert!(!message.contains("INV#"), "Invoice # must be escaped");
    assert!(message.contains("INV\\#"), "Invoice # should be escaped");
    assert!(message.contains("\\& Co"), "Ampersand must be escaped");
}

/// Test that amounts with dots don't get mangled
#[test]
fn test_amount_formatting() {
    let amount = "1,234.50";
    let escaped = escape_markdown(amount);
    assert_eq!(escaped, "1,234\\.50");
}

/// Test empty fields don't cause issues
#[test]
fn test_empty_field_handling() {
    let empty = "";
    let escaped = escape_markdown(empty);
    assert_eq!(escaped, "");
}

/// Test VAT rate formatting (includes %)
#[test]
fn test_vat_rate_formatting() {
    let vat_rate = "25%";
    let escaped = escape_markdown(vat_rate);
    assert_eq!(escaped, "25%");
}

/// Comprehensive test with real-world invoice data
#[test]
fn test_real_world_invoice_formatting() {
    let test_cases = vec![
        ("BrewDog #42 AB", "BrewDog \\#42 AB"),
        ("Pizza & Pasta Co.", "Pizza \\& Pasta Co\\."),
        ("Test (Stockholm)", "Test \\(Stockholm\\)"),
        ("Invoice #12345", "Invoice \\#12345"),
    ];

    for (input, expected) in test_cases {
        let escaped = escape_markdown(input);
        assert_eq!(escaped, expected, "Failed to escape: {}", input);
    }
}

#[test]
fn test_static_period_is_escaped_in_markdown_message() {
    let message = markdown_message(&[
        MarkdownSegment {
            text: "Document Approved",
            style: MarkdownStyle::Bold,
        },
        MarkdownSegment {
            text: "\n\nThe document has been automatically approved and queued for export.",
            style: MarkdownStyle::Plain,
        },
    ]);

    assert!(message.contains("queued for export\\.") || message.contains("export pool\\."));
}

#[test]
fn test_code_label_helper_escapes_document_id() {
    let formatted =
        markdown_label_code_value("Document ID", "7c1bf3c8-d0f4-49a8-98e8-19acde1e0415");
    assert_eq!(
        formatted,
        "Document ID: `7c1bf3c8\\-d0f4\\-49a8\\-98e8\\-19acde1e0415`"
    );
}
