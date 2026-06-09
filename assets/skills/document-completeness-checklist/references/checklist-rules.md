# Completeness Rules

Use these rules when building the document completeness checklist.

## Field Presence

Treat these fields as the checklist basis:

- `supplier_name`
- `invoice_date`
- `total_amount`

A field is **present** when the tool result contains a non-empty visible value for that field.

A field is **missing** when the tool result shows:

- `null`
- an empty string
- no value at all

Do not infer a value from other fields or from general context. If the tool output does not visibly contain the value, mark it as missing.

## Status Handling

`status` is reported for context only. It is **not** a completeness field.

For example:

- a document can have a valid `status` and still be missing one or more checklist fields
- a document can look complete on visible fields and still not be accounting-ready or export-ready

Do not treat status as proof of accounting quality.

## Missing-Field Wording

Use short deterministic wording:

- `Supplier name present` / `Missing supplier name`
- `Invoice date present` / `Missing invoice date`
- `Total amount present` / `Missing total amount`

Avoid speculative language such as:

- `probably missing`
- `seems incomplete because`
- `likely needs review because`

Only report what is visible.

## Scope Limitation

The default checklist is based on the recent documents returned by `list_documents`.

If the user asks for “all documents” or implies full historical coverage, say clearly that the checklist reflects the recent documents currently accessible through tools and may not represent the entire workspace history.

## Drilldown Rule

Use `get_document` only when the user explicitly asks for deeper inspection of one document or explicitly asks to confirm details for one specific `document_short_ref`.

Do not use `get_document` across many documents as part of the default checklist flow.

## Safety Boundary

This skill should not claim:

- that a document is accounting-ready
- that a document is export-ready
- that the checklist proves business-rule completeness

This skill only reports visible field completeness from the current read-only tool output.
