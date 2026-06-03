//! Invoice module for Finelor
//!
//! Handles:
//! - Invoice data structures
//! - HTML invoice generation from templates

pub mod generator;

pub use generator::{InvoiceData, InvoiceItem, generate_invoice_html, render_invoice_html};
