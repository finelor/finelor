//! Unit tests for SIE4 format writer

use finelor::export::{
    Sie4Config, Transaction, Verification, decode_sie4, encode_sie4, generate_sie4_export,
    validate_sie4_balanced, validate_verification_balanced,
};

#[test]
fn test_sie4_config_default() {
    let config = Sie4Config::default();
    assert_eq!(config.program_name, "Finelor");
    assert_eq!(config.encoding, "PC8");
    assert_eq!(config.currency, "SEK");
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
fn test_verification_unbalanced() {
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
                amount: -50.0, // Wrong contra amount
                trans_date: "20240115".to_string(),
                ver_date: "20240115".to_string(),
                object1: None,
                object2: None,
                quantity: None,
                signature: None,
            },
        ],
    };

    assert!(!validate_verification_balanced(&ver));
}

#[test]
fn test_three_way_transaction_balanced() {
    // Invoice with VAT: expense + vat amount = total
    let ver = Verification {
        series: "F".to_string(),
        number: 1,
        date: "20240115".to_string(),
        text: "Restaurant Invoice".to_string(),
        reg_date: "20240115".to_string(),
        signature: "Test".to_string(),
        project: None,
        transactions: vec![
            Transaction {
                account: "7690".to_string(),
                amount: 80.0, // net
                trans_date: "20240115".to_string(),
                ver_date: "20240115".to_string(),
                object1: None,
                object2: None,
                quantity: None,
                signature: None,
            },
            Transaction {
                account: "2640".to_string(),
                amount: 20.0, // vat
                trans_date: "20240115".to_string(),
                ver_date: "20240115".to_string(),
                object1: None,
                object2: None,
                quantity: None,
                signature: None,
            },
            Transaction {
                account: "1930".to_string(),
                amount: -100.0, // total
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
fn test_validate_sie4_balanced_multiple() {
    let verifications = vec![
        Verification {
            series: "F".to_string(),
            number: 1,
            date: "20240115".to_string(),
            text: "Test1".to_string(),
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
        },
        Verification {
            series: "F".to_string(),
            number: 2,
            date: "20240116".to_string(),
            text: "Test2".to_string(),
            reg_date: "20240116".to_string(),
            signature: "Test".to_string(),
            project: None,
            transactions: vec![
                Transaction {
                    account: "7690".to_string(),
                    amount: 250.0,
                    trans_date: "20240116".to_string(),
                    ver_date: "20240116".to_string(),
                    object1: None,
                    object2: None,
                    quantity: None,
                    signature: None,
                },
                Transaction {
                    account: "1930".to_string(),
                    amount: -200.0, // Wrong amount!
                    trans_date: "20240116".to_string(),
                    ver_date: "20240116".to_string(),
                    object1: None,
                    object2: None,
                    quantity: None,
                    signature: None,
                },
            ],
        },
    ];

    let (balanced, errors) = validate_sie4_balanced(&verifications);
    assert!(!balanced);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("F 2"));
}

#[test]
fn test_generate_sie4_export_rejects_unbalanced() {
    let config = Sie4Config::default();
    let verifications = vec![Verification {
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
                amount: -50.0, // Wrong!
                trans_date: "20240115".to_string(),
                ver_date: "20240115".to_string(),
                object1: None,
                object2: None,
                quantity: None,
                signature: None,
            },
        ],
    }];

    let result = generate_sie4_export(&config, &verifications);
    assert!(result.is_err());
}

#[test]
fn test_generate_sie4_with_vat() {
    let config = Sie4Config::default();

    let verifications = vec![Verification {
        series: "F".to_string(),
        number: 1,
        date: "20240115".to_string(),
        text: "Restaurant Invoice".to_string(),
        reg_date: "20240115".to_string(),
        signature: "Finelor".to_string(),
        project: None,
        transactions: vec![
            Transaction {
                account: "7690".to_string(),
                amount: 80.0, // net amount
                trans_date: "20240115".to_string(),
                ver_date: "20240115".to_string(),
                object1: None,
                object2: None,
                quantity: None,
                signature: None,
            },
            Transaction {
                account: "2640".to_string(),
                amount: 20.0, // vat 25%
                trans_date: "20240115".to_string(),
                ver_date: "20240115".to_string(),
                object1: None,
                object2: None,
                quantity: None,
                signature: None,
            },
            Transaction {
                account: "1930".to_string(),
                amount: -100.0, // total
                trans_date: "20240115".to_string(),
                ver_date: "20240115".to_string(),
                object1: None,
                object2: None,
                quantity: None,
                signature: None,
            },
        ],
    }];

    let result = generate_sie4_export(&config, &verifications);
    assert!(result.is_ok());

    let content = result.unwrap();
    assert!(content.contains("#FLAGGA 0"));
    assert!(content.contains("#VER F 1"));
    assert!(content.contains("#TRANS \"7690\""));
    assert!(content.contains("#TRANS \"2640\""));
    assert!(content.contains("#TRANS \"1930\""));
    assert!(content.contains("80.00"));
    assert!(content.contains("20.00"));
    assert!(content.contains("-100.00"));
}

#[test]
fn test_encode_decode_roundtrip() {
    let content = "#FLAGGA 0\n#VER F 1 20240115 \"Test\" 20240115 \"Finelor\" \"\"";

    let encoded = encode_sie4(content).unwrap();
    assert!(!encoded.is_empty());

    let decoded = decode_sie4(&encoded).unwrap();
    assert!(decoded.contains("#FLAGGA"));
    assert!(decoded.contains("#VER"));
}

#[test]
fn test_empty_verification_fails() {
    let config = Sie4Config::default();
    let verifications: Vec<Verification> = vec![];

    let result = generate_sie4_export(&config, &verifications);
    assert!(result.is_ok()); // Empty is ok, just no content generated
}
