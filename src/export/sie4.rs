//! SIE4 format writer for Swedish invoice export
//!
//! Implements SIE4 ASCII format per Swedish accounting standard
//! Documentation: https://www.sie.se/

use chrono::Utc;
use std::collections::HashSet;

pub use crate::error::{AppError, AppResult};

/// SIE4 export configuration
#[derive(Debug, Clone)]
pub struct Sie4Config {
    pub program_name: String,
    pub program_version: String,
    pub encoding: String,
    pub company_org_nr: String,
    pub company_name: String,
    pub fiscal_year_start: String,
    pub fiscal_year_end: String,
    pub currency: String,
}

impl Default for Sie4Config {
    fn default() -> Self {
        Self {
            program_name: "Finelor".to_string(),
            program_version: "1.0.0".to_string(),
            encoding: "PC8".to_string(),
            company_org_nr: "5560000000".to_string(),
            company_name: "Unknown Company".to_string(),
            fiscal_year_start: "20240101".to_string(),
            fiscal_year_end: "20241231".to_string(),
            currency: "SEK".to_string(),
        }
    }
}

/// A verification (journal entry) in SIE4 format
#[derive(Debug, Clone)]
pub struct Verification {
    pub series: String,
    pub number: i32,
    pub date: String,
    pub text: String,
    pub reg_date: String,
    pub signature: String,
    pub project: Option<String>,
    pub transactions: Vec<Transaction>,
}

/// A transaction line within a verification
#[derive(Debug, Clone)]
pub struct Transaction {
    pub account: String,
    pub amount: f64,
    pub trans_date: String,
    pub ver_date: String,
    pub object1: Option<String>,
    pub object2: Option<String>,
    pub quantity: Option<f64>,
    pub signature: Option<String>,
}

/// SIE4 file writer
pub struct Sie4Writer;

impl Sie4Writer {
    /// Generate complete SIE4 file content
    pub fn generate(config: &Sie4Config, verifications: &[Verification]) -> String {
        let mut lines = Vec::new();

        // Header section
        Self::write_header(&mut lines, config);

        // Chart of accounts section (collect unique accounts)
        let accounts = Self::collect_accounts(verifications);
        Self::write_chart_of_accounts(&mut lines, &accounts);

        // Transactions section
        Self::write_verifications(&mut lines, verifications);

        lines.join("\r\n")
    }

    /// Write header section
    fn write_header(lines: &mut Vec<String>, config: &Sie4Config) {
        let now = Utc::now();
        let gen_date = now.format("%Y%m%d").to_string();
        let gen_time = now.format("%H%M%S").to_string();

        lines.push("#FLAGGA 0".to_string());
        lines.push(format!(
            "#PROGRAM \"{}\" {}",
            Self::escape_string(&config.program_name),
            config.program_version
        ));
        lines.push(format!("#FORMAT {}", config.encoding));
        lines.push(format!("#GEN {} {}", gen_date, gen_time));
        lines.push("#SIETYP 4".to_string());
        lines.push("#PROSA".to_string());
        lines.push("Exported from Finelor - Swedish Invoice Processing Agent".to_string());
        lines.push("#KONTO".to_string());
        lines.push("#KTYP SR".to_string());
        lines.push("#TYP 4".to_string());
        lines.push(format!("#ORGNR {}", config.company_org_nr));
        lines.push(format!(
            "#FNAMN \"{}\"",
            Self::escape_string(&config.company_name)
        ));
        lines.push(format!(
            "#RAR 0 {} {}",
            config.fiscal_year_start, config.fiscal_year_end
        ));
        lines.push(format!("#VALUTA {}", config.currency));
        lines.push(String::new());
    }

    /// Collect unique accounts from all verifications
    fn collect_accounts(verifications: &[Verification]) -> HashSet<String> {
        let mut accounts = HashSet::new();

        // Include standard VAT accounts
        accounts.insert("2610".to_string());
        accounts.insert("2611".to_string());
        accounts.insert("2612".to_string());
        accounts.insert("2640".to_string());
        accounts.insert("2641".to_string());
        accounts.insert("2642".to_string());
        accounts.insert("1930".to_string()); // Checking account

        for ver in verifications {
            for trans in &ver.transactions {
                accounts.insert(trans.account.clone());
            }
        }

        accounts
    }

    /// Write chart of accounts section
    fn write_chart_of_accounts(lines: &mut Vec<String>, accounts: &HashSet<String>) {
        let mut sorted_accounts: Vec<String> = accounts.iter().cloned().collect();
        sorted_accounts.sort();

        for account in sorted_accounts {
            lines.push(format!("#KONTO \"{}\"", account));
        }

        if !accounts.is_empty() {
            lines.push(String::new());
        }
    }

    /// Write all verifications
    fn write_verifications(lines: &mut Vec<String>, verifications: &[Verification]) {
        for ver in verifications {
            Self::write_verification(lines, ver);
        }
    }

    /// Write a single verification with its transactions
    fn write_verification(lines: &mut Vec<String>, ver: &Verification) {
        // #VER line
        let project = ver.project.as_deref().unwrap_or("\"\"");
        lines.push(format!(
            "#VER {} {} \"{}\" {} \"{}\" {}",
            esc(&ver.series),
            ver.number,
            esc(&ver.text),
            ver.reg_date,
            esc(&ver.signature),
            project
        ));

        // #TRANS lines
        for trans in &ver.transactions {
            let amount_str = Self::format_amount(trans.amount);
            let obj1 = esc(trans.object1.as_deref().unwrap_or(""));
            let obj2 = esc(trans.object2.as_deref().unwrap_or(""));
            let qty = trans.quantity.map_or(0.0, |q| q);
            let sign = esc(trans.signature.as_deref().unwrap_or(""));

            lines.push(format!(
                "   #TRANS \"{}\" {} {} {} \"{}\" \"{}\" {} \"{}\"",
                esc(&trans.account),
                amount_str,
                trans.trans_date,
                trans.ver_date,
                obj1,
                obj2,
                qty,
                sign
            ));
        }

        lines.push("{}".to_string());
    }

    /// Format amount for SIE4 (dot decimal, no thousands separator)
    fn format_amount(amount: f64) -> String {
        // Round to 2 decimal places
        let rounded = (amount * 100.0).round() / 100.0;

        // Format with exactly 2 decimal places
        format!("{:.2}", rounded)
    }

    /// Escape special characters in string values
    fn escape_string(s: &str) -> String {
        // Replace quotes with escaped quotes, limit length
        let escaped = s.replace('"', "\\\"");
        if escaped.len() > 35 {
            format!("{}...", &escaped[..32])
        } else {
            escaped
        }
    }
}

/// Helper function to escape values for SIE4
fn esc(s: &str) -> String {
    s.replace('"', "\\\"")
}

/// Generate complete SIE4 export from configuration and verifications
pub fn generate_sie4_export(
    config: &Sie4Config,
    verifications: &[Verification],
) -> AppResult<String> {
    // Validate that all verifications are balanced
    for ver in verifications {
        if !validate_verification_balanced(ver) {
            return Err(AppError::Agent(format!(
                "Verification {} {} is not balanced",
                ver.series, ver.number
            )));
        }
    }

    Ok(Sie4Writer::generate(config, verifications))
}

/// Validate that a verification sums to zero
pub fn validate_verification_balanced(ver: &Verification) -> bool {
    let sum: f64 = ver.transactions.iter().map(|t| t.amount).sum();
    sum.abs() < 0.01 // Allow for small floating point errors
}

/// Validate that all verifications in a set are balanced
pub fn validate_sie4_balanced(verifications: &[Verification]) -> (bool, Vec<String>) {
    let mut errors = Vec::new();
    let mut all_balanced = true;

    for ver in verifications {
        if !validate_verification_balanced(ver) {
            let sum: f64 = ver.transactions.iter().map(|t| t.amount).sum();
            errors.push(format!(
                "Verification {} {} is unbalanced (sum: {:.2})",
                ver.series, ver.number, sum
            ));
            all_balanced = false;
        }
    }

    (all_balanced, errors)
}

/// Encode SIE4 content to PC850 (CP850) encoding
pub fn encode_sie4(content: &str) -> AppResult<Vec<u8>> {
    use encoding_rs::WINDOWS_1252;

    // Use Windows-1252 as fallback (similar to PC8)
    let (encoded, _, _) = WINDOWS_1252.encode(content);
    Ok(encoded.into_owned())
}

/// Decode PC850/Windows-1252 bytes to string
pub fn decode_sie4(bytes: &[u8]) -> AppResult<String> {
    use encoding_rs::WINDOWS_1252;

    let (decoded, _, had_errors) = WINDOWS_1252.decode(bytes);
    if had_errors {
        // Try lossy decode
        Ok(decoded.to_string())
    } else {
        Ok(decoded.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_amount() {
        assert_eq!(Sie4Writer::format_amount(1234.5), "1234.50");
        assert_eq!(Sie4Writer::format_amount(-1234.5), "-1234.50");
        assert_eq!(Sie4Writer::format_amount(100.0), "100.00");
        assert_eq!(Sie4Writer::format_amount(0.0), "0.00");
    }

    #[test]
    fn test_verification_balanced() {
        let ver = Verification {
            series: "F".to_string(),
            number: 1,
            date: "20240115".to_string(),
            text: "Test".to_string(),
            reg_date: "20240115".to_string(),
            signature: "Test".to_string(),
            project: None,
            transactions: vec![
                Transaction {
                    account: "5400".to_string(),
                    amount: 100.0,
                    trans_date: "20240115".to_string(),
                    ver_date: "20240115".to_string(),
                    object1: None,
                    object2: None,
                    quantity: None,
                    signature: None,
                },
                Transaction {
                    account: "1930".to_string(),
                    amount: -100.0,
                    trans_date: "20240115".to_string(),
                    ver_date: "20240115".to_string(),
                    object1: None,
                    object2: None,
                    quantity: None,
                    signature: None,
                },
            ],
        };

        assert!(validate_verification_balanced(&ver));
    }

    #[test]
    fn test_collect_accounts() {
        let ver = Verification {
            series: "F".to_string(),
            number: 1,
            date: "20240115".to_string(),
            text: "Test".to_string(),
            reg_date: "20240115".to_string(),
            signature: "Test".to_string(),
            project: None,
            transactions: vec![Transaction {
                account: "5400".to_string(),
                amount: 100.0,
                trans_date: "20240115".to_string(),
                ver_date: "20240115".to_string(),
                object1: None,
                object2: None,
                quantity: None,
                signature: None,
            }],
        };

        let accounts = Sie4Writer::collect_accounts(&[ver]);
        assert!(accounts.contains("5400"));
        assert!(accounts.contains("1930")); // Always included
    }

    #[test]
    fn test_generate_sie4_header() {
        let config = Sie4Config::default();

        let content = Sie4Writer::generate(
            &config,
            &[Verification {
                series: "F".to_string(),
                number: 1,
                date: "20240115".to_string(),
                text: "Invoice".to_string(),
                reg_date: "20240115".to_string(),
                signature: "Finelor".to_string(),
                project: None,
                transactions: vec![
                    Transaction {
                        account: "5400".to_string(),
                        amount: 100.0,
                        trans_date: "20240115".to_string(),
                        ver_date: "20240115".to_string(),
                        object1: None,
                        object2: None,
                        quantity: None,
                        signature: None,
                    },
                    Transaction {
                        account: "1930".to_string(),
                        amount: -100.0,
                        trans_date: "20240115".to_string(),
                        ver_date: "20240115".to_string(),
                        object1: None,
                        object2: None,
                        quantity: None,
                        signature: None,
                    },
                ],
            }],
        );

        assert!(content.contains("#FLAGGA 0"));
        assert!(content.contains("#PROGRAM \"Finelor\""));
        assert!(content.contains("#FORMAT PC8"));
        assert!(content.contains("#ORGNR 5560000000"));
        assert!(content.contains("#FNAMN \"Unknown Company\""));
        assert!(content.contains("#RAR 0 20240101 20241231"));
        assert!(content.contains("#VALUTA SEK"));
        assert!(content.contains("#VER F 1"));
    }
}
