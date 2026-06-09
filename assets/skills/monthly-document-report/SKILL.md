---
name: Monthly Document Report
description: Create a month-by-month Markdown table of recent documents using the existing list_documents tool and group rows by invoice_date month.
category: reporting
keywords:
  - reporting
  - monthly
  - documents
  - table
---

# Monthly Document Report

## Overview

Use this skill to create a month-by-month Markdown table of documents that are currently accessible through Finelor's existing read-only tools.

This skill is limited by the current tool surface. `list_documents` returns recent documents, not a complete all-documents monthly dataset.

## When to Use

Use this skill when the user asks for:

- a month-by-month document report
- a document table grouped by month
- a monthly summary of recent documents

## When NOT to Use

Do not use this skill when:

- the user wants exact all-documents reporting across the full workspace
- the user wants export, analytics, charts, or accounting totals not available from the current tools
- the user asks about one specific document and a direct document lookup is more appropriate

## Workflow

1. Call `list_documents` to retrieve the recent documents currently available through the assistant tools.
2. Use `invoice_date` as the month basis.
3. Convert each `invoice_date` into `YYYY-MM`.
4. If `invoice_date` is missing, place the document in an `Unknown month` bucket.
5. Sort month buckets descending. Keep `Unknown month` last.
6. Within each month, sort documents by `invoice_date` descending when available.
7. Return a compact Markdown table with these columns:
   `Month | Short ref | Status | Supplier | Invoice date | Total amount`
8. Before or after the table, include a short limitation note stating that the report reflects the recent documents currently accessible through tools, not a guaranteed full all-documents workspace report.
9. If `list_documents` returns no items, say plainly that no recent documents are available for a month-by-month report right now instead of producing an empty table.

When the user asks for "all documents month by month", still provide the table for the currently accessible recent documents, but do not imply the result is complete across the entire workspace.
