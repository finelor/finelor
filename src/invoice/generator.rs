//! HTML generation service for invoices

use anyhow::{Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};

/// An invoice item line
#[derive(Debug, Clone)]
pub struct InvoiceItem {
    pub description: String,
    pub qty: f64,
    pub price: f64,
    pub total: f64,
}

impl InvoiceItem {
    /// Calculate the total based on qty and price
    pub fn calculate_total(&self) -> f64 {
        self.qty * self.price
    }
}

/// Invoice data for invoice document generation
#[derive(Debug, Clone)]
pub struct InvoiceData {
    /// Company issuing the invoice
    pub company_name: String,
    pub company_address: String,
    /// Invoice metadata
    pub invoice_number: String,
    pub invoice_date: String,
    pub due_date: String,
    /// Customer information
    pub customer_name: String,
    pub customer_address: String,
    /// Invoice items
    pub items: Vec<InvoiceItem>,
    /// Totals
    pub subtotal: f64,
    pub tax_rate: f64,
    pub tax_amount: f64,
    pub total: f64,
    /// Additional info
    pub payment_terms: String,
    pub notes: Option<String>,
}

impl InvoiceData {
    /// Calculate totals from items
    pub fn calculate_totals(&mut self) {
        self.subtotal = self.items.iter().map(|i| i.calculate_total()).sum();
        self.tax_amount = self.subtotal * (self.tax_rate / 100.0);
        self.total = self.subtotal + self.tax_amount;
    }

    /// Format currency for display
    pub fn format_currency(&self, amount: f64) -> String {
        format!("{:.2}", amount)
    }
}

/// Generate invoice HTML content from the template and data.
pub async fn render_invoice_html(data: &InvoiceData) -> Result<String> {
    let template_path = invoice_template_path()?;
    let template = tokio::fs::read_to_string(&template_path)
        .await
        .with_context(|| {
            format!(
                "Failed to load invoice HTML template from {}",
                template_path.display()
            )
        })?;

    // Process template with mustache-style variable replacement
    process_template(&template, data)
}

/// Generate an invoice HTML file from the invoice template.
///
/// Returns the path to the generated HTML file.
pub async fn generate_invoice_html(data: &InvoiceData) -> Result<PathBuf> {
    let html = render_invoice_html(data).await?;

    // Create temp file for HTML output
    let temp_dir = std::env::temp_dir();
    let timestamp = Utc::now().timestamp();
    let html_path = temp_dir.join(format!(
        "invoice_{}_{}.html",
        data.invoice_number.replace(" ", "_"),
        timestamp
    ));

    tokio::fs::write(&html_path, html)
        .await
        .context("Failed to write invoice HTML file")?;

    Ok(html_path)
}

/// Process mustache-style template with invoice data
fn process_template(template: &str, data: &InvoiceData) -> Result<String> {
    let mut html = template.to_string();

    // Basic variable replacements
    html = html.replace("{{company_name}}", &escape_html(&data.company_name));
    html = html.replace("{{company_address}}", &escape_html(&data.company_address));
    html = html.replace("{{invoice_number}}", &escape_html(&data.invoice_number));
    html = html.replace("{{invoice_date}}", &escape_html(&data.invoice_date));
    html = html.replace("{{due_date}}", &escape_html(&data.due_date));
    html = html.replace("{{customer_name}}", &escape_html(&data.customer_name));
    html = html.replace(
        "{{customer_address}}",
        &format_address_html(&data.customer_address),
    );
    html = html.replace("{{subtotal}}", &data.format_currency(data.subtotal));
    html = html.replace("{{tax_rate}}", &format!("{:.1}", data.tax_rate));
    html = html.replace("{{tax_amount}}", &data.format_currency(data.tax_amount));
    html = html.replace("{{total}}", &data.format_currency(data.total));
    html = html.replace("{{payment_terms}}", &escape_html(&data.payment_terms));

    // Optional notes field
    if let Some(notes) = &data.notes {
        html = html.replace("{{notes}}", &escape_html(notes));
    } else {
        // Remove notes section if no notes
        html = remove_conditional_block(&html, "notes");
    }

    // Handle items array with {{#items}}...{{/items}} block
    html = process_items_block(&html, &data.items)?;

    Ok(html)
}

/// Process items block in template
fn process_items_block(template: &str, items: &[InvoiceItem]) -> Result<String> {
    let items_start = "{{#items}}";
    let items_end = "{{/items}}";

    if let Some(start_idx) = template.find(items_start) {
        if let Some(end_idx) = template.find(items_end) {
            let before = &template[..start_idx];
            let item_template = &template[start_idx + items_start.len()..end_idx];
            let after = &template[end_idx + items_end.len()..];

            let mut items_html = String::new();
            for item in items {
                let item_row = item_template
                    .replace("{{description}}", &escape_html(&item.description))
                    .replace("{{qty}}", &format!("{:.0}", item.qty))
                    .replace("{{price}}", &format!("{:.2}", item.price))
                    .replace("{{total}}", &format!("{:.2}", item.total));
                items_html.push_str(&item_row);
            }

            return Ok(format!("{}{}{}", before, items_html, after));
        }
    }

    // If no items block found, return as-is
    Ok(template.to_string())
}

/// Remove a conditional block from template when condition is not met
fn remove_conditional_block(template: &str, block_name: &str) -> String {
    let start_tag = format!("{{{{#{}}}}}", block_name);
    let end_tag = format!("{{{{/{}}}}}", block_name);

    if let Some(start_idx) = template.find(&start_tag) {
        if let Some(end_idx) = template.find(&end_tag) {
            let before = &template[..start_idx];
            let after = &template[end_idx + end_tag.len()..];
            return format!("{}{}", before, after);
        }
    }

    template.to_string()
}

fn invoice_template_path() -> Result<PathBuf> {
    let candidates = [
        Path::new("/app/assets/skills/invoice-generation/templates/invoice.html"),
        Path::new("./assets/skills/invoice-generation/templates/invoice.html"),
    ];

    candidates
        .iter()
        .find(|path| path.exists())
        .map(|path| path.to_path_buf())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Invoice template not found in any expected location: {}",
                candidates
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// Escape HTML special characters
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Format multi-line address for HTML display (uses triple braces in template)
fn format_address_html(address: &str) -> String {
    // The template uses {{{customer_address}}} which is unescaped,
    // so we can include <br> tags for line breaks
    escape_html(address).replace("\n", "<br>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_html() {
        assert_eq!(escape_html("Test & Co."), "Test &amp; Co.");
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
    }

    #[test]
    fn test_process_items_block() {
        let template = "{{#items}}<tr><td>{{description}}</td></tr>{{/items}}";
        let items = vec![InvoiceItem {
            description: "Service A".to_string(),
            qty: 1.0,
            price: 100.0,
            total: 100.0,
        }];
        let result = process_items_block(template, &items).unwrap();
        assert_eq!(result, "<tr><td>Service A</td></tr>");
    }

    #[tokio::test]
    async fn render_invoice_html_includes_core_fields() {
        let mut invoice = InvoiceData {
            company_name: "Finelor".to_string(),
            company_address: "Stockholm".to_string(),
            invoice_number: "INV-123".to_string(),
            invoice_date: "June 3, 2026".to_string(),
            due_date: "July 3, 2026".to_string(),
            customer_name: "Acme Corp".to_string(),
            customer_address: "123 Main St\nStockholm".to_string(),
            items: vec![InvoiceItem {
                description: "Consulting".to_string(),
                qty: 2.0,
                price: 150.0,
                total: 300.0,
            }],
            subtotal: 0.0,
            tax_rate: 25.0,
            tax_amount: 0.0,
            total: 0.0,
            payment_terms: "Net 30".to_string(),
            notes: Some("Thanks!".to_string()),
        };
        invoice.calculate_totals();

        let html = render_invoice_html(&invoice).await.unwrap();
        assert!(html.contains("INV-123"));
        assert!(html.contains("Acme Corp"));
        assert!(html.contains("Consulting"));
        assert!(html.contains("300.00"));
        assert!(html.contains("375.00"));
        assert!(html.contains("Thanks!"));
    }

    #[tokio::test]
    async fn render_invoice_html_omits_notes_section_when_notes_missing() {
        let mut invoice = InvoiceData {
            company_name: "Finelor".to_string(),
            company_address: "Stockholm".to_string(),
            invoice_number: "INV-124".to_string(),
            invoice_date: "June 3, 2026".to_string(),
            due_date: "July 3, 2026".to_string(),
            customer_name: "Acme Corp".to_string(),
            customer_address: "123 Main St".to_string(),
            items: vec![InvoiceItem {
                description: "Consulting".to_string(),
                qty: 1.0,
                price: 100.0,
                total: 100.0,
            }],
            subtotal: 0.0,
            tax_rate: 0.0,
            tax_amount: 0.0,
            total: 0.0,
            payment_terms: "Net 30".to_string(),
            notes: None,
        };
        invoice.calculate_totals();

        let html = render_invoice_html(&invoice).await.unwrap();
        assert!(!html.contains("<h4 style=\"margin-top: 15px;\">Notes</h4>"));
        assert!(!html.contains("{{notes}}"));
    }

    #[tokio::test]
    async fn generate_invoice_html_writes_html_file() {
        let mut invoice = InvoiceData {
            company_name: "Finelor".to_string(),
            company_address: "Stockholm".to_string(),
            invoice_number: "INV-125".to_string(),
            invoice_date: "June 3, 2026".to_string(),
            due_date: "July 3, 2026".to_string(),
            customer_name: "Acme Corp".to_string(),
            customer_address: "123 Main St".to_string(),
            items: vec![InvoiceItem {
                description: "Consulting".to_string(),
                qty: 1.0,
                price: 100.0,
                total: 100.0,
            }],
            subtotal: 0.0,
            tax_rate: 0.0,
            tax_amount: 0.0,
            total: 0.0,
            payment_terms: "Net 30".to_string(),
            notes: None,
        };
        invoice.calculate_totals();

        let path = generate_invoice_html(&invoice).await.unwrap();
        assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("html"));
        let html = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(html.contains("INV-125"));
        let _ = tokio::fs::remove_file(path).await;
    }
}
