---
name: Document Workflow
description: Help with document intake, processing, review, retry or reprocess requests, and export-related workflow questions.
category: operations
keywords:
  - documents
  - intake
  - processing
  - review
  - retry
  - export
---

# Document Workflow

## Overview

Use this skill for document-related workflow requests in Finelor.

This skill covers:

- processing uploaded documents such as receipts, invoices, and financial documents
- managing accounting review flows and exports
- helping users send a receipt, invoice, file, or image directly in the chat when they want to upload one
- review, retry, reprocess, and export workflow requests when they are explicitly asked for

## When to Use

Use this skill when the user asks about:

- uploading or sending a receipt, invoice, or financial document
- processing a document
- document review workflow
- retrying or reprocessing a document
- exporting documents
- document workflow questions generally

## When NOT to Use

Do not use this skill when:

- the request is unrelated to Finelor or accounting operations
- the request is only about static assistant boundaries, legal/accounting caution, or runtime/tool rules
- another more specific skill is clearly a better fit for the task

## Workflow

1. If the user wants to upload a receipt or invoice, tell them to send the file or image directly in the chat.
2. Help users with document-related workflow requests involving processing, review, retry, reprocess, and export.
3. Follow skill-guided workflows when they are available.
4. Mutating tools only prepare a user confirmation prompt; they do not execute changes.
5. Use prepare tools only when the user explicitly asks to review, retry, reprocess, or export.
6. For ambiguous wording such as whether something can be retried or exported, answer or ask a clarifying question instead of preparing a confirmation.

## Examples

- `I want to upload a receipt`
- `Process this invoice`
- `Can you review this document?`
- `Retry this document`
- `Reprocess this invoice`
- `Export these documents`
