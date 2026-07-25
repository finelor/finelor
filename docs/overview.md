# Finelor Overview

## Our Vision
<div align="center">
 
> We believe the future of accounting is not another dashboard, ERP system, or bookkeeping tool, but an autonomous, conversational, AI-native finance department that lives directly inside the communication channels businesses already use every day. Instead of forcing companies to adapt to complex software and manual processes, Finelor enables AI agents to handle operational finance tasks such as bookkeeping, invoice processing, reconciliation, approvals, reporting, and financial coordination autonomously while interacting naturally with humans through platforms like Telegram, Slack, WhatsApp, and email. 
>
> Our vision is to build the foundational infrastructure for agentic financial operations where finance becomes embedded, intelligent, transparent, scalable, and continuously operational, giving every business access to enterprise-grade financial capabilities powered by AI rather than traditional human-heavy accounting structures.

</div>

## What Is Finelor?

Finelor is an agentic platform AI-native finance and accounting departments designed to help business users run core accounting and finance operations with less manual effort.

It enables companies to interact with an AI-driven accounting department directly through the communication tools they already use.

In practical terms, users can send invoices, receipts, questions, approvals, and financial tasks conversationally, while Finelor’s internal AI agents process, classify, reconcile, verify, export, and coordinate accounting operations in the background.

Finelor is not another accounting SaaS — It is infrastructure for agentic financial operations.

## Who It Is For

Finelor is built for business users who handle recurring accounting paperwork and want more reliable, lower-friction processing. The immediate fit is founders, operators, and small teams that need accounting operations to run consistently without becoming a full-time manual task.

## What It Does

At a high level, Finelor turns incoming accounting documents into structured, reviewable, and export-oriented accounting outcomes. It aims to reduce repetitive work, improve consistency, and keep humans focused on exceptions instead of routine processing.

The result is an operating flow where users can:

- ingest invoices/receipts from supported channels
- get extracted and ingested document data into the system first
- explicitly request accounting processing when they want documents to move into accounting
- get analyzed accounting data after that request
- review uncertain cases before finalization
- move approved items toward export

## Channels

Finelor can receive user interactions and document intake through messaging channels. In the current flow, Telegram is the primary supported channel.

## Assistant

Finelor includes an agentic assistant layer that users can chat with directly through channels such as Telegram. Users can ask operational questions, get procedural guidance, and interact with accounting flows through normal chat conversations.

## Agents

### Intake Agent

Receives incoming documents, registers them in the system, and prepares them for processing.

### Vision Agent

Extracts structured information from document images so accounting steps can work with usable data.

### Accountant Agent

Analyzes extracted data and produces draft accounting interpretation for downstream validation.

### Validator Agent

Runs deterministic checks to assess quality, consistency, and confidence before approval.

### Review Agent

Routes uncertain or low-confidence cases to human review so risky outputs are not auto-finalized.

### Export Agent

Packages approved outputs into export-ready accounting artifacts for downstream accounting systems.

## How It Works (High Level)

1. Intake: documents are received and registered.
2. Extraction: document content is extracted into structured fields and the document becomes ingested.
3. Accounting request: accounting work begins only when explicitly requested.
4. Accounting analysis and validation: accounting interpretation is produced and checked for quality and consistency.
5. Human review: uncertain or low-confidence cases are routed for review.
6. Export: approved outputs proceed to export-ready accounting artifacts.
