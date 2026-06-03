# Finelor Skills System - Additional Skills Proposal
## Future Implementation Plan (Phase 7+)

**Status**: Deferred (not part of initial 5-phase rollout)  
**Priority**: Medium to High (after invoice-generation is production-ready)  
**Estimated Timeline**: Weeks 8-12 (rolling release after core infrastructure)

---

## Objective

Extend the Finelor skill library beyond the initial `invoice-generation` skill with comprehensive accounting workflows covering payroll, reconciliation, financial reporting, and document management.

---

## Proposed Skill Library

### Category: Accounting

#### 1. Payment Reconciliation
**Priority**: High  
**Stage**: Processing → Review  
**Required Tools**: `get_document`, `list_documents`, `document_status_summary`, `prepare_open_review`

```markdown
---
name: payment-reconciliation
description: "Match incoming payments to outstanding invoices and resolve discrepancies"
required_tools:
  - get_document
  - list_documents
  - document_status_summary
  - prepare_open_review
---

# Payment Reconciliation

## Overview
This skill matches bank payments to unpaid invoices, handles partial payments, 
overpayments, and creates reviews for unclear matches.

## When to Use
- Payment received in bank account
- User asks to "reconcile this payment"
- Automatic reconciliation fails with low confidence

## Workflow

### Step 1: Load Payment Document
```json
{
  "tool": "get_document",
  "params": { "document_ref": "{{payment_id}}" }
}
```

### Step 2: Find Outstanding Invoices
```json
{
  "tool": "list_documents",
  "params": {
    "document_type": "invoice",
    "status": ["pending", "partial"],
    "sort": "date_desc"
  }
}
```

### Step 3: Match Strategies

| Match Type | Criteria | Action |
|------------|----------|--------|
| **Exact** | Payment = Invoice total AND customer matches | Auto-reconcile |
| **Reference** | Payment note contains invoice number | Confirm invoice |
| **Partial** | Payment < Invoice total | Apply partial payment |
| **Overpayment** | Payment > Invoice total | Create credit or review |
| **Multi-invoice** | Sum of multiple invoices = Payment | Split payment |
| **No match** | No corresponding invoice | Create review |

### Step 4: Handle Match

**For partial payments**:
```json
{
  "tool": "prepare_open_review",
  "params": {
    "document_ids": ["{{payment_id}}"],
    "reason": "partial_payment_allocation",
    "suggested_action": "Apply ${payment_amount} to invoice ${invoice_id} (${balance_remaining} remaining)"
  }
}
```

**For unclear matches**:
```json
{
  "tool": "prepare_open_review",
  "params": {
    "document_ids": ["{{payment_id}}"],
    "reason": "ambiguous_payment_match",
    "candidates": ["{{candidate_invoice_ids}}"]
  }
}
```

## Common Pitfalls

1. **Currency mismatches** - Payment in EUR, invoice in USD. Always check before matching.
2. **Duplicate application** - Verify invoice not already paid by another method.
3. **Timing** - Payment may arrive before invoice is created (advance payment).
4. **Reference typos** - "INV-123" might mean "INV-0123" or "INV-1234".

## Verification Checklist

- [ ] Customer name matches exactly (case-insensitive)
- [ ] Currency is consistent
- [ ] Invoice not already fully paid
- [ ] Payment amount allocated correctly
- [ ] Review created if any uncertainty
```

---

#### 2. Tax Calculation Review
**Priority**: High  
**Stage**: Validation  
**Required Tools**: `explain_line_item`, `get_document`, `prepare_retry_document`

```markdown
---
name: tax-calculation-review
description: "Validate tax calculations on invoices and line items"
required_tools:
  - explain_line_item
  - get_document
  - prepare_retry_document
---

# Tax Calculation Review

## Overview
Verify tax rates are correct for jurisdiction, product type, and customer status.
Flags discrepancies for human review.

## When to Use
- Document flagged for tax validation
- User asks to "check tax on this invoice"
- Pre-export validation

## Tax Rules Reference

| Jurisdiction | Standard Rate | Reduced | Zero | Exempt |
|--------------|---------------|---------|------|--------|
| UK | 20% | 5% | 0% | N/A |
| EU (DE) | 19% | 7% | 0% | ✓ |
| USA (varies) | State-specific | | | |

## Workflow

### Step 1: Identify Line Items
```json
{
  "tool": "get_document",
  "params": { 
    "document_ref": "{{doc_id}}",
    "include_line_items": true
  }
}
```

### Step 2: Analyze Each Line
For each line item with tax:

```json
{
  "tool": "explain_line_item",
  "params": {
    "document_ref": "{{doc_id}}",
    "line_index": {{line_number}},
    "question": "What tax rate is applied and is it correct for this product type and customer location?"
  }
}
```

### Step 3: Calculate Expected Tax

**Expected formula**: `subtotal × rate = tax`  
**Round to 2 decimal places**

Example red flags:
- Tax rate doesn't match jurisdiction
- Zero tax on taxable item
- Wrong rate for food/medical items
- VAT on reverse charge transaction

### Step 4: Flag Issues

If tax calculation incorrect:
```json
{
  "tool": "prepare_retry_document",
  "params": {
    "document_refs": ["{{doc_id}}"],
    "reason": "tax_calculation_error",
    "note": "Line {{line_num}}: Expected 20% tax (£{{expected}}), found {{actual}}"
  }
}
```

## Common Pitfalls

1. **Reverse charge VAT** - Customer pays VAT, not supplier
2. **Intra-EU B2B** - No VAT charged (reverse charge applies)
3. **Mixed rates** - Invoice has items with different rates
4. **Tax ID validation** - Invalid VAT numbers mean normal rates apply
5. **Discount application** - Tax on net or gross? (varies by jurisdiction)

## Verification Checklist

- [ ] Tax rate appropriate for jurisdiction
- [ ] Product classification correct (standard/reduced/zero)
- [ ] Customer status verified (B2B vs B2C)
- [ ] Calculations use rounded values correctly
- [ ] Reverse charge considered for applicable transactions
```

---

### Category: Payroll

#### 3. Payslip Generation
**Priority**: Medium  
**Stage**: Processing  
**Required Tools**: `get_document`, `calculate_payroll`, `create_payslip`

```markdown
---
name: payslip-generation
description: "Generate payslips from employee timesheets and salary data"
required_tools:
  - get_document
  - get_employee_data
  - calculate_payroll
  - create_payslip
tags: [payroll, hr]
---

# Payslip Generation

## Overview
This skill creates payslips including base salary, overtime, deductions, 
taxes, and net pay calculations.

## When to Use
- Monthly payroll run
- Employee requests payslip
- Off-cycle payment needed

## Workflow

### Step 1: Load Employee Record
```json
{
  "tool": "get_employee_data",
  "params": { "employee_id": "{{employee_id}}" }
}
```

### Step 2: Load Timesheet (if hourly)
```json
{
  "tool": "get_document",
  "params": { 
    "document_ref": "{{timesheet_doc_id}}",
    "document_type": "timesheet"
  }
}
```

### Step 3: Calculate Payroll
```json
{
  "tool": "calculate_payroll",
  "params": {
    "employee_id": "{{employee_id}}",
    "period": "{{payroll_period}}",
    "timesheet_id": "{{timesheet_doc_id}}"  // optional
  }
}
```

## Calculation Components

| Component | Description | Example |
|-----------|-------------|---------|
| Base Salary | Fixed monthly/annual | £3,500.00 |
| Gross Pay | Base + adjustments | £3,550.00 |
| Income Tax | PAYE calculation | -£420.00 |
| National Insurance | Employee contribution | -£280.00 |
| Pension | Employee contribution | -£150.00 |
| Student Loan | If applicable | -£50.00 |
| Other Deductions | Benefits, advances | -£20.00 |
| **Net Pay** | Final take-home | **£2,630.00** |

## Common Pitfalls

1. **Wrong tax code** - Always verify current tax code
2. **YTD calculations** - Annual thresholds affect monthly amounts
3. **Pro-rata calculations** - Mid-month joiners/leaver adjustments
4. **Benefits in kind** - Taxable benefits added to gross
5. **Statutory pay** - Maternity/paternity/sick pay different rules

## Verification Checklist

- [ ] Employee tax code is current
- [ ] YTD totals updated correctly
- [ ] All deductions authorized
- [ ] Net pay positive and reasonable
- [ ] Pension contribution within limits
- [ ] Payslip format compliant (UK: itemized breakdown required)
```

---

### Category: Validation

#### 4. Document Validation
**Priority**: High  
**Stage**: Validation  
**Required Tools**: `document_status_summary`, `explain_document`, `prepare_retry_document`

```markdown
---
name: document-validation
description: "Comprehensive validation of extracted document data"
required_tools:
  - document_status_summary
  - explain_document
  - prepare_retry_document
---

# Document Validation

## Overview
Validates document extraction confidence, data completeness, and 
business logic. Routes to retry or review based on issues found.

## When to Use
- Post-extraction validation pipeline
- User asks to "validate this document"
- Batch pre-export validation

## Validation Dimensions

### 1. Extraction Confidence
Check `document_status_summary`:

```json
{
  "tool": "document_status_summary",
  "params": { "document_ids": ["{{doc_id}}"] }
}
```

**Thresholds**:
- Overall confidence > 0.85: Proceed
- Overall confidence 0.70-0.85: Validate fields individually
- Overall confidence < 0.70: Auto-retry extraction

### 2. Field Completeness

Required fields by document type:

| Document Type | Required Fields |
|---------------|-----------------|
| invoice | customer, line_items, total, date, invoice_number |
| receipt | merchant, amount, date, payment_method |
| expense | employee, amount, purpose, receipt_attached |
| payslip | employee_id, gross_pay, deductions, net_pay |

### 3. Business Logic

**Amount checks**:
- Total = sum of line items
- Subtotal + tax = total
- Currency symbol present
- Amount > 0

**Date checks**:
- Date not in future
- Date within last 2 years (stale document?)
- Fiscal year alignment (if applicable)

**Customer checks**:
- Customer exists in directory
- Address format valid
- Tax ID format correct for jurisdiction

## Workflow

### Step 1: Check Status
```json
{
  "tool": "document_status_summary",
  "params": { "document_ids": ["{{doc_id}}"] }
}
```

If confidence low → Retry

### Step 2: Deep Validation
```json
{
  "tool": "explain_document",
  "params": { 
    "document_ref": "{{doc_id}}",
    "question": "Are all required fields present and do amounts add up correctly?"
  }
}
```

### Step 3: Route Based on Issues

| Severity | Action |
|----------|--------|
| Critical (missing totals, no customer) | `prepare_retry_document` |
| Warning (low confidence, missing optional) | `prepare_open_review` |
| Pass | Forward to next stage |

## Common Pitfalls

1. **Over-reliance on confidence** - High confidence ≠ correctness
2. **Threshold too rigid** - Some documents will never achieve 0.90
3. **Missing edge cases** - Foreign currencies, negative amounts
4. **Stale documents** - Old expense from 2 years ago

## Verification Checklist

- [ ] All required fields extracted
- [ ] Calculations (total, tax) verified
- [ ] Customer in directory
- [ ] Dates reasonable
- [ ] Confidence above threshold
- [ ] Business rules satisfied
```

---

### Category: Reporting

#### 5. Financial Statements
**Priority**: Medium  
**Stage**: Export  
**Required Tools**: `list_documents`, `prepare_export_documents`, `generate_report`

```markdown
---
name: financial-statements
description: "Generate balance sheets, P&L, and cash flow statements"
required_tools:
  - list_documents
  - prepare_export_documents
  - generate_report
---

# Financial Statements

## Overview
Compile period-end financial reports from approved documents in Finelor.
Generates trial balance, P&L, and balance sheet.

## When to Use
- Month/quarter end closing
- Annual accounts preparation
- Management reporting
- Investor updates

## Report Types

### Profit & Loss (P&L)
**Period**: Income minus expenses  
**Key sections**:
- Revenue
- Cost of Goods Sold
- Gross Profit
- Operating Expenses
- Net Profit

### Balance Sheet
**Snapshot**: Assets = Liabilities + Equity  
**Key sections**:
- Current Assets (cash, receivables)
- Fixed Assets
- Current Liabilities (payables)
- Long-term Liabilities
- Equity

### Cash Flow
**Movement**: Operating, Investing, Financing  
**Sources**:
- P&L (net profit)
- Balance sheet changes (receivables, payables)
- Manual adjustments

## Workflow

### Step 1: Define Period
```json
{
  "tool": "generate_report",
  "params": {
    "report_type": "financial_statements",
    "period_start": "{{start_date}}",
    "period_end": "{{end_date}}",
    "entity_id": "{{company_id}}"
  }
}
```

### Step 2: Gather Source Data

Implicit via `generate_report`:
```
- All invoices approved in period
- All expenses approved in period  
- All payments reconciled
- Opening/closing balances
```

### Step 3: Validate Completeness

Before finalizing, check:
- All documents exported?
- Missing approvals flagged?
- Period boundaries correct?
- Comparative periods aligned?

## Common Pitfalls

1. **Cut-off issues** - Invoice dated Dec 31, approved Jan 2
2. **Accrual vs cash** - Different basis for different reports
3. **Currency conversion** - FX rates at transaction vs period end
4. **Opening balances** - Must match prior period closing
5. **Intercompany** - Eliminate internal transactions

## Verification Checklist

- [ ] Period selection correct
- [ ] All source documents approved and exported
- [ ] Cut-off applied consistently
- [ ] Currency treatment documented
- [ ] Opening balances reconciled
- [ ] Cross-figures agree (P&L net = Balance sheet change)
- [ ] Comparative periods restated if needed
```

---

### Category: Export

#### 6. Export Preparation
**Priority**: High  
**Stage**: Export  
**Required Tools**: `list_export_ready`, `prepare_export_documents`, `export_to_integration`

```markdown
---
name: export-preparation
description: "Prepare documents for export to accounting systems"
required_tools:
  - list_export_ready
  - prepare_export_documents
  - export_to_integration
---

# Export Preparation

## Overview
Final validation and formatting of documents before exporting to 
QuickBooks, Xero, or other accounting integrations.

## When to Use
- Pre-export validation
- User asks to "export to [system]"
- Scheduled batch export job

## Supported Integrations

| System | Format | Authentication |
|--------|--------|----------------|
| QuickBooks Online | API OAuth | OAuth 2.0 |
| Xero | API OAuth | OAuth 2.0 |
| Sage | API Key | API Key |
| CSV | File | None |
| MT940 | Bank format | None |

## Workflow

### Step 1: Identify Export-Ready Documents
```json
{
  "tool": "list_export_ready",
  "params": {
    "status": "approved",
    "period": "{{export_period}}",
    "limit": 100
  }
}
```

### Step 2: Prepare Export Package
```json
{
  "tool": "prepare_export_documents",
  "params": {
    "document_refs": ["{{doc_ids}}"],
    "target_system": "{{integration}}",
    "options": {
      "create_customers": true,
      "create_suppliers": true,
      "match_existing": true
    }
  }
}
```

**Preparation steps**:
- Map Finelor fields to target system
- Check customer/supplier exists (create if needed)
- Validate tax codes mapped correctly
- Generate import file or API calls

### Step 3: Execute Export
```json
{
  "tool": "export_to_integration",
  "params": {
    "prepared_export_id": "{{export_id}}",
    "confirmed": false  // Preview first
  }
}
```

Review preview, then confirm:
```json
{
  "tool": "export_to_integration",
  "params": {
    "prepared_export_id": "{{export_id}}",
    "confirmed": true
  }
}
```

## Common Pitfalls

1. **Duplicate exports** - Check if already in target system
2. **Tax code mapping** - "20% VAT" may map to different codes per system
3. **Account mapping** - GL account codes differ between systems
4. **Payment status** - Export as paid or unpaid?
5. **Multi-currency** - Some systems handle differently

## Verification Checklist

- [ ] All documents approved and complete
- [ ] Customer/supplier mapping verified
- [ ] Tax codes matched to target system
- [ ] Account codes mapped
- [ ] Duplicate check passed
- [ ] Preview reviewed for anomalies
- [ ] Export logged with IDs for reconciliation
```

---

## Additional Proposed Skills

### Quick Wins (Lower Effort)

| Skill | Category | Description |
|-------|----------|-------------|
| date-extraction | Validation | Parse various date formats, validate reasonableness |
| currency-conversion | Processing | Convert amounts using current/historic rates |
| duplicate-detection | Validation | Identify likely duplicate documents |
| customer-lookup | Processing | Find or create customer from partial info |

### Medium Complexity

| Skill | Category | Description |
|-------|----------|-------------|
| credit-note-generation | Accounting | Create credit notes for returns/refunds |
| expense-categorization | Processing | Assign spending categories |
| receipt-matching | Processing | Match receipt image to expense claim |
| approval-workflow | Processing | Route documents for approval |

### High Complexity (Research Phase)

| Skill | Category | Description |
|-------|----------|-------------|
| fraud-detection | Validation | Flag suspicious patterns in documents |
| audit-trail-generation | Reporting | Create compliance audit documentation |
| multi-entity-consolidation | Reporting | Combine multiple company accounts |
| predictive-cashflow | Reporting | Forecast based on patterns |

---

## Implementation Priority Matrix

### Immediate (Month 2-3 after Phase 5)
- [ ] **payment-reconciliation** - High value, builds on invoice skill
- [ ] **document-validation** - Core pipeline piece
- [ ] **tax-calculation-review** - Compliance critical

### Near-term (Month 4-5)
- [ ] **export-preparation** - User-facing, daily use
- [ ] **payslip-generation** - Payroll use case
- [ ] **credit-note-generation** - Completes invoicing workflow

### Mid-term (Month 6+)
- [ ] **financial-statements** - Complex, high value
- [ ] **expense-categorization** - AI-enabled classification
- [ ] **receipt-matching** - Vision + document correlation

### Research (>6 months)
- [ ] **fraud-detection** - Requires training data
- [ ] **predictive-cashflow** - ML-based forecasting

---

## Skill Dependencies Graph

```
invoice-generation
    │
    ├─► payment-reconciliation (uses invoices)
    │
    └─► credit-note-generation (references invoices)

document-validation
    │
    ├─► tax-calculation-review (specific validation)
    ├─► duplicate-detection (specific validation)
    └─► export-preparation (uses validated docs)

payment-reconciliation
    │
    ├─► export-preparation (reconciled transactions)
    └─► financial-statements (reconciliation data)
```

---

## Resource Estimates

| Phase | Skills | Developer Days | Notes |
|-------|--------|------------------|-------|
| Immediate | 3 | 15-20 | Similar complexity to invoice |
| Near-term | 3 | 20-25 | Moderate complexity |
| Mid-term | 3 | 30-40 | Higher complexity |
| Research | 2 | 20+ | Unpredictable R&D |

---

## Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| **Skill bloat** | Strict prioritization, usage tracking required before adding |
| **Skill drift** | Built-in versioning, curator review process |
| **Overlapping responsibilities** | Clear skill boundaries, dependency graph |
| **Maintenance overhead** | User contribution model, shared skill marketplace |
| **Jurisdiction complexity** | Focus on UK/EU first, modular jurisdiction handling |

---

## Open Questions

1. **Skill Marketplace**: Should Finelor host a public skill registry?
2. **Vendor Integration**: Should accounting software vendors provide official skills?
3. **Community Contributions**: User-submitted skills? Review process?
4. **Industry Specialization**: Per-industry skill packs (restaurant, retail, consulting)?
5. **Regulatory Bundles**: Compliance-specific skill sets for audits?

---

## Success Metrics

| Metric | Target |
|--------|--------|
| Skills in library | 10+ within 3 months of Phase 5 |
| Skill coverage | 80% of common accounting workflows |
| User-created skills | 5+ custom skills per active deployment |
| Skill adoption | >70% of tasks use skills vs ad-hoc |

---

This proposal is **ready for review** once the `invoice-generation` skill from Phase 4 is production-ready and stable.
