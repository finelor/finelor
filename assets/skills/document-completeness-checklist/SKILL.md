---
name: Document Completeness Checklist
description: Review recent documents for missing supplier name, invoice date, or total amount and present the result as a per-document checklist. Use list_documents by default and optionally get_document only when the user explicitly asks for deeper detail on one document.
category: quality-control
keywords:
  - completeness
  - checklist
  - missing fields
  - document quality
  - audit
---

# Overview

Use this skill when the user wants a checklist-style audit of document completeness. The default scope is the recent documents currently available through `list_documents`, not the full historical workspace.

This skill is designed to help the assistant identify visible missing fields in a simple, deterministic way. It should not infer data that is not present and should not claim that a document is accounting-ready or export-ready based only on the visible fields.

# When to Use

Use this skill when the user asks for any of the following:

- a document completeness checklist
- a missing-fields audit
- a quality check of recent documents
- a checklist of which documents are missing supplier name, invoice date, or total amount

This skill is also appropriate when the user wants to follow up on one checklist result and inspect one specific document more closely.

# When NOT to Use

Do not use this skill when the user is asking for:

- a full historical report across all documents in the workspace
- export actions or export preparation
- retry, review-opening, or other workflow mutations
- a month-by-month report or grouped financial reporting

If the user needs a full historical dataset, be explicit that the current tool surface only exposes recent documents through `list_documents`.

# Workflow

1. Call `list_documents` first and treat the returned recent items as the checklist scope.
2. For each returned document, inspect these visible fields:
   - `supplier_name`
   - `invoice_date`
   - `total_amount`
   - `status` for reporting context
3. Build one checklist block per document rather than one combined grid table.
4. Mark each checklist item clearly as complete or missing.
5. Include the document `short_ref` and `status` at the top of each checklist block.
6. Use simple deterministic wording for missing data, such as `Missing supplier name`.
7. If the user asks for “all documents” or otherwise implies full coverage, include a short limitation note that this checklist reflects the recent documents currently accessible through tools, not a guaranteed full workspace-wide audit.
8. If the user explicitly asks to inspect one document more closely, confirm a specific field on one document, or continue from a checklist result into document-level detail, optionally call `get_document` for that one document.
9. Do not fan out into many `get_document` calls for the default checklist flow.
10. If no recent documents are returned, say so plainly instead of producing empty checklist sections.

Read `references/checklist-rules.md` for the completeness rules and `templates/document-completeness-checklist.md` for the response shape.

# Examples

Example requests that should trigger this skill:

- `Give me a document completeness checklist`
- `Which recent documents are missing key fields?`
- `Audit recent documents for missing supplier name, invoice date, or total amount`
- `Check these recent documents for completeness`

Example follow-up that may justify a single `get_document` call:

- `Inspect D000123 more closely`
- `Confirm what fields are missing on D000123`
