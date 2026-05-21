# SIE4 Export Specification - Finelor
# Version: SIE4 ASCII 4B (Standard Swedish Accounting Format)
# Reference: www.sie.se (Swedish standard for accounting exports)

## File Format Overview

SIE4 is a fixed-format text file used for exporting accounting data to Swedish accounting systems (Fortnox, Visma, Hogia, etc.)

### Character Encoding
- **PC8** (CP850) or **ISO-8859-1** (Latin-1)
- Line endings: CRLF (`\r\n`)

### File Extension: `.si` or `.sie`

## File Structure

```
#FLAGGA 0                    ; First line always "0"
#PROGRAM "Finelor" 1.0      ; Program name and version
#FORMAT PC8                  ; Character encoding
#GEN {YYYYMMDD} {HHMMSS}     ; Generation date/time
#SIETYP 4                   ; File type (4 = SIE4 export)
#PROSA                      ; Optional free text
Exported from Finelor - Swedish Invoice Processing Agent
#KONTO                      ; Chart of accounts section
#KTYP SR                    ; Account type (SR = result)
#TYP        4               ; Verification type
#ORGNR {ORGANIZATION_NUMBER} ; Company org.nr
#FNAMN {COMPANY_NAME}        ; Company name
#ADRESS                      ; Address block
{R_ADDRESS}
{R_POSTCODE} {R_CITY}
{R_COUNTRY}
#RAR 0 {YYYY0101} {YYYY1231} ; Fiscal year 0 dates
#VALUTA SEK                 ; Currency

#VER {SERIES} {VER_NUMBER} {VER_DATE} {VER_TEXT} {REG_DATE} {SIGNATURE}
   ; Transactions
   #TRANS {KONTO} {AMOUNT} {TRANS_DATE} {VER_DATE} {OBJ1} {OBJ2} {QUANTITY} {SIGN}
   #TRANS {MOTKONTO} {-AMOUNT} ...
#VER ...
```

## Field Definitions

### Header Fields

| Tag | Description | Format | Example |
|-----|-------------|--------|---------|
| #FLAGGA | Always 0 | N/A | `#FLAGGA 0` |
| #PROGRAM | Exporter name & version | "Name" version | `#PROGRAM "Finelor" 1.0` |
| #FORMAT | Encoding | ASCII/PC8/ISO-8859-1 | `#FORMAT PC8` |
| #GEN | Generation datetime | YYYYMMDD HHMMSS | `#GEN 20240115 143022` |
| #SIETYP | SIE version | 1, 2, 3, 4, I1-I4 | `#SIETYP 4` |
| #ORGNR | Company org.nr | 10 digits | `#ORGNR 5561234567` |
| #FNAMN | Company name | Max 35 chars | `#FNAMN "Finelor AB"` |
| #RAR | Fiscal year | index start end | `#RAR 0 20240101 20241231` |
| #VALUTA | Currency code | 3 letters | `#VALUTA SEK` |

### Transaction Fields (#VER / #TRANS)

**#VER Line** (Verification/Entry header):
```
#VER {VERIFICATION_SERIES} {NUMBER} {DATE} {TEXT} {REG_DATE} {SIGNATURE} {PROJECT}
```

| Position | Field | Format | Description |
|----------|-------|--------|-------------|
| 1 | Series | BB, F, etc. | Batch series identifier |
| 2 | Number | numeric | Sequential verification number |
| 3 | Date | YYYYMMDD | Transaction date |
| 4 | Text | "string" | Description (max 30 chars) |
| 5 | RegDate | YYYYMMDD | Registration date |
| 6 | Signature | "string" | Who registered (max 35 chars) |
| 7 | Project | "string" | Optional project code |

**#TRANS Line** (Individual transaction):
```
#TRANS {KONTO} {AMOUNT} {TRANS_DATE} {VER_DATE} {OBJ1} {OBJ2} {QUANTITY} {SIGN}
```

| Position | Field | Format | Description |
|----------|-------|--------|-------------|
| 1 | Account | 1-4 digits | BAS account number |
| 2 | Amount | decimal | Signed, use . for decimal |
| 3 | TransDate | YYYYMMDD | Transaction date |
| 4 | VerDate | YYYYMMDD | Verification date |
| 5-6 | Objects | "object" | Optional accounting objects |
| 7 | Quantity | number | Optional quantity |
| 8 | Sign | "signature" | Optional signer |

## Amount Formatting

- Use dot (`.`) as decimal separator
- No thousands separator
- Negative amounts: prefix with minus sign (`-`)
- Positive amounts: no sign prefix
- Maximum 2 decimal places

Examples:
```
575.00    ; Positive amount
-575.00   ; Negative amount (credit/contra)
1234.56   ; Standard decimal
```

## Complete Example

```
#FLAGGA 0
#PROGRAM "Finelor" 1.0.0
#FORMAT PC8
#GEN 20240115 143022
#SIETYP 4
#PROSA
Exported from Finelor
#KONTO
#KTYP SR
#TYP 4
#ORGNR 5561234567
#FNAMN "Testbolaget AB"
#ADRESS
Storgatan 1
111 22 Stockholm
Sverige
#RAR 0 20240101 20241231
#VALUTA SEK

#VER F 1 20240115 "Restaurang X - Lunch" 20240115 "Finelor" ""
   #TRANS 7690 250.00 20240115 20240115 "" "" 0 ""
   #TRANS 1930 -250.00 20240115 20240115 "" "" 0 ""
#VER F 2 20240116 "Kontorsmaterial Inköp" 20240116 "Finelor" ""
   #TRANS 5400 599.00 20240116 20240116 "" "" 0 ""
   #TRANS 2610 119.80 20240116 20240116 "" "" 0 ""
   #TRANS 1930 -718.80 20240116 20240116 "" "" 0 ""
```

## Mapping from Finelor Data

| Finelor Field | SIE4 Field | Notes |
|---------------|------------|-------|
| `transaction.date` | VER date, TRANS date | YYYYMMDD |
| `supplier.name` | VER text | Truncated to 30 chars |
| `accounting_decision.kontonummer` | TRANS account | 4 digits |
| `accounting_decision.motkonto` | TRANS account (2nd line) | Contra entry |
| `amounts.total_incl_vat` | TRANS amounts | Split for VAT |
| `amounts.vat_amount` | TRANS on 261X/264X | VAT account |

## VAT Account Mapping

| VAT Type | Account | Description |
|----------|---------|-------------|
| Outgoing VAT 25% | 2610 | Utgående moms, 25% |
| Outgoing VAT 12% | 2620 | Utgående moms, 12% |
| Outgoing VAT 6% | 2630 | Utgående moms, 6% |
| Incoming VAT 25% | 2640 | Ingående moms, 25% |
| Incoming VAT 12% | 2641 | Ingående moms, 12% |
| Incoming VAT 6% | 2642 | Ingående moms, 6% |

## Validation Rules

1. **Balanced entries**: Sum of amounts in each #VER must equal 0
2. **Unique verification numbers**: Series + number must be unique
3. **Date ranges**: All dates within declared #RAR fiscal year
4. **Account existence**: All accounts must be defined in #KONTO section
5. **Character limits**: Respect max lengths to avoid truncation

## Multi-Document Export

When exporting multiple documents:
- Use continuous verification numbering
- One #VER block per document
- Group by series (suggest "F" for Finelor)
- Include all #TRANS lines before closing #VER
