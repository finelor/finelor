---
name: invoice-generation
description: "Create customer invoices and generate PDF documents"
version: "1.0.0"
category: accounting
tags: [accounting, invoicing, pdf]
required_tools: [skill_view]
---

# Invoice Generation

Generate professional PDF invoices for customers.

## CRITICAL INSTRUCTION

**DO NOT call `prepare_generate_invoice` until you have collected ALL required information from the user.**

This tool requires complete data in a single call. You CANNOT call it incrementally. Collect everything first through conversation, then call it once with all fields populated.

## STRICT TOOL FIELD NAMES

When you call `prepare_generate_invoice`, you must use the exact field names from the tool schema.

- Use `customer_name`, not `customer`
- Use `items`, not `line_items`
- For each item, use:
  - `description`
  - `qty`
  - `price`

Do not invent alternate field names.

- Use `qty`, not `quantity`
- Use `price`, not `unit_price`
- Numeric values must be numbers, not strings

## Required Data Checklist (MUST HAVE)

Before calling `prepare_generate_invoice`, you must have:

- [ ] **Customer name** (required)
- [ ] **At least one invoice item** with ALL of:
  - [ ] Description (required)
  - [ ] Quantity (required)
  - [ ] Unit price (required)

## Optional Data You Can Also Collect

- Customer address (recommended)
- Customer email (recommended)
- Tax rate percentage (e.g., 25 for 25% - will default to 0% if not provided)
- Due date (will default to 30 days from now)
- Payment terms (will default to "Net 30")
- Additional notes

## Conversation Flow

### Step 1: Collect Customer Information

Ask the user:
> "I'll help you create an invoice. First, I need some information:
> 1. What is the customer's name?
> 2. What is their address (optional but recommended)?
> 3. What is their email (optional)?"

**WAIT** for their response. Do not proceed until they provide at least the customer name.

### Step 2: Collect Invoice Items

Ask the user:
> "What items should I include on the invoice? For each item, I need:
> - Description
> - Quantity
> - Unit price"

**WAIT** for their response with complete items. Do not proceed until you have at least one item with description, quantity, AND price.

### Step 3: Ask for Optional Details

Ask the user:
> "Great! A few more optional details:
> - What tax rate should I apply (or 0 for no tax)?"
> - "When should this invoice be due? (I'll default to 30 days from now)"
> - "Any specific payment terms or notes to include?"

**WAIT** for their response. Use defaults if they don't specify.

### Step 4: Confirm Details and Generate

Once you have ALL required data, summarize:

> "I'll create an invoice with:
> - Customer: [name]
> - Items: [list items with qty × price = total]
> - Subtotal: $[amount]
> - Tax ([rate]%): $[amount]
> - **Total: $[amount]**
> - Due: [date]
>
> Shall I generate the PDF?"

Before calling the tool, convert the collected natural-language details into the exact tool schema.

For example:

- "15 hours of consulting at 1000 SEK per hour"

must become:

```json
{
  "items": [
    {
      "description": "Consulting",
      "qty": 15,
      "price": 1000
    }
  ]
}
```

Do not pass conversational labels like `quantity` or `unit_price` to the tool. Convert them to `qty` and `price` first.

**Only after they confirm**, call `prepare_generate_invoice` with ALL the data:

```json
{
  "customer_name": "Acme Corp",
  "customer_address": "123 Main St\nNew York, NY 10001\nUSA",
  "customer_email": "billing@acmecorp.com",
  "items": [
    {"description": "Consulting Services", "qty": 10, "price": 150.00},
    {"description": "Travel Expenses", "qty": 1, "price": 250.00}
  ],
  "tax_rate": 25,
  "due_date": "30 days",
  "payment_terms": "Net 30",
  "notes": "Thank you for your business!"
}
```

## Tool Schema Reference

Use the `prepare_generate_invoice` tool with this exact structure:

- `customer_name` (string, **REQUIRED**) - Customer or company name
- `items` (array, **REQUIRED**) - Invoice line items, each with:
  - `description` (string, **REQUIRED**)
  - `qty` (number, **REQUIRED**)
  - `price` (number, **REQUIRED**)
- `customer_address` (string, optional) - Multi-line address
- `customer_email` (string, optional) - Email address
- `tax_rate` (number, optional) - Tax percentage (e.g., 25 for 25%), defaults to 0
- `due_date` (string, optional) - Due date or relative term like "30 days", defaults to 30 days from now
- `payment_terms` (string, optional) - Terms like "Net 30", defaults to "Net 30"
- `notes` (string, optional) - Additional notes for the invoice

## NEVER DO THIS

❌ **WRONG**: Call `prepare_generate_invoice` with only customer_name, then call again with items
❌ **WRONG**: Call `prepare_generate_invoice` with incomplete items missing qty or price
❌ **WRONG**: Assume data from previous turns - always use what the user explicitly provided in the current conversation

✅ **CORRECT**: Collect all data through conversation, then call `prepare_generate_invoice` ONCE with complete data

## Example Output Format

Once generated, the user will see:

```
**Invoice Preview**

Customer: Acme Corp
Items: 2
Subtotal: $1,750.00
Tax (25%): $437.50
**Total: $2,187.50**

Confirm generate invoice PDF?
[Confirm] [Cancel]
```

After they click Confirm, the PDF will be generated and sent with:
- Invoice number (auto-generated)
- Invoice date (today)
- All items and totals
- Payment terms and due date
