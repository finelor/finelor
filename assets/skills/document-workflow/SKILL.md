---
name: Document Review, Status, and Actions
description: Handle document status questions, review flows, blocked or stuck document explanations, retry or reprocess requests, export requests, and upload-related document operations. Load this skill before using document-operation tools.
category: operations
keywords:
  - document status
  - status
  - pending review
  - processing
  - review
  - open review
  - retry
  - reprocess
  - export
  - export ready
  - blocked document
  - stuck document
  - why blocked
  - upload
  - send file
  - document actions
---

# Document Review, Status, and Actions

## Overview

Use this skill for document-operation requests in Finelor when the user needs status, inspection, review, retry, reprocess, export, or upload guidance.

This skill covers:

- document status overviews and operational document questions
- identifying what needs attention or what is ready for export
- explaining why one document is blocked, pending, failed, or ready
- processing uploaded documents such as receipts, invoices, and financial documents
- managing accounting review flows and document actions
- helping users send a receipt, invoice, file, or image directly in the chat when they want to upload one
- review, retry, reprocess, and export workflow requests when they are explicitly asked for

## When to Use

Use this skill when the user asks about:

- document status now
- what needs attention
- which documents are pending review
- which documents are export ready
- why a document is blocked or stuck
- one-document operational follow-ups
- uploading or sending a receipt, invoice, or financial document
- processing a document
- opening review or document review workflow
- retrying or reprocessing a document
- exporting documents
- document actions or document workflow questions generally

## When NOT to Use

Do not use this skill when:

- the request is unrelated to Finelor or accounting operations
- the request is only about static assistant boundaries, legal/accounting caution, or runtime/tool rules
- the user wants a completeness or missing-fields audit; use `Document Completeness Checklist`
- the user wants a month-by-month or grouped reporting view; use `Monthly Document Report`
- another more specific skill is clearly a better fit for the task

## Workflow

1. If the user wants to upload a receipt or invoice, tell them to send the file or image directly in the chat.
2. For status or count questions, use `document_status_summary`.
3. For recent operational overviews, use `list_documents`.
4. For attention or review queues, use `list_pending_reviews`.
5. For export-ready queue questions, use `list_export_ready`.
6. For one-document detail lookups, use `get_document`.
7. For questions about why one document is blocked, pending, failed, or ready, use `explain_document`.
8. Mutating tools only prepare a user confirmation prompt; they do not execute changes.
9. Use `prepare_open_review` only when the user explicitly asks to review or open review for a document.
10. Use `prepare_retry_document` only when the user explicitly asks to retry or reprocess a document.
11. Use `prepare_export_documents` only when the user explicitly asks to export documents.
12. For ambiguous wording such as whether something should be reviewed, retried, or exported, answer or ask a clarifying question instead of preparing a confirmation.

## Examples

- `What is the status now?`
- `What needs attention?`
- `Which documents are pending review?`
- `Show export-ready documents`
- `Why is D000123 blocked?`
- `I want to upload a receipt`
- `Process this invoice`
- `Open review for this document`
- `Retry this document`
- `Reprocess this invoice`
- `Export these documents`
