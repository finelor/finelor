# Finelor Agent Instructions

These instructions apply to all coding agents and human contributors working in this repository.

## Before You Start

Read this file first, then use `README.md` and current docs in `docs/` to find the active source-of-truth docs but don't only rely on documents, the main source-of-truth is the code files and scripts.

## Project Purpose

Finelor is "Your own accounting department, on autopilot."

The current repository is a WIP Rust application moving toward a production-ready system for processing invoices and receipts. It receives documents through messaging channels and connectors, extracts data with vision and accounting agents, validates the results, routes uncertain cases to human review, and exports approved documents in country specific format or integration with accounting systems.

The long-term product direction is broader: an agentic accounting department for entrepreneurs, solo consultants, SMEs, and eventually larger companies. `Finelor` is the likely future user-facing agent across channels such as Telegram, Slack, and similar interfaces.

`Finelor` should be understood as the user-facing coordination layer, not the only source of accounting work. Background connectors may eventually discover invoices, receipts, transactions, or other artifacts from systems such as Google Drive, email, bank feeds, APIs, or webhooks.

## Architecture Overview

At basic level the product is centered on a quality-control pipeline:

1. Intake
2. Vision extraction
3. Accounting analysis
4. Deterministic validation
5. Review and human approval
6. Export

This pipeline is critical. It controls the quality of accounting data entering the system and is likely to be one of the highest-change areas for human development. It is also likely to become the core accounting-quality engine that future product and agent layers build around.

## Setup Commands

Prefer docker environment:

## Test, Lint, and Typecheck Commands

Prefer make commands.

Required baseline checks for most changes:

```bash
make check
```

Optional Docker smoke check:

```bash
make dev-up
make health
make dev-down
```

## Development Rules

- Keep changes scoped to the selected task.
- Prefer existing project patterns over new abstractions.
- Add or update tests when behavior changes.
- Update documentation when behavior, setup, workflow, or validation commands change.
- For web-related work, keep styling/patterns consistent with Tailwind + DaisyUI conventions.
- Prefer DaisyUI components before custom utility-only HTML for common UI primitives. Current pinned DaisyUI version is `5.5.19`; use the matching latest official docs at <https://daisyui.com/components/> and the v5 docs at <https://daisyui.com/docs/v5/>.
- Use the Leptos-compatible icon stack `leptos_icons` + `icondata` for icons. The project enables the Lucide icon pack through `icondata` to keep the icon set consistent and avoid ad hoc inline SVG duplication.
- Treat the current pipeline as the likely accounting-quality core, but not as a frozen implementation shape.
- Treat the current `IntakeAgent` as an MVP intake-boundary implementation, not as the final `Finelor` concept.
- Prefer changes that move `Finelor` toward the documented accounting-department vision while preserving pipeline guarantees.
- Avoid generic maintenance tasks unless they directly support the current phase or a named production risk.
- Evolve architecture when a task has a concrete goal, constraints, and acceptance criteria.
- Treat accounting, tax, and compliance behavior as high risk.
- Do not hide failing checks. Record the exact commands run and their outcomes.
- Do not use destructive commands unless explicitly approved by a human.

## Definition of Done

A task is done when:

- the requested behavior or documentation change is complete;
- the relevant tests/checks have been run and recorded;
- any required docs are updated;
- known limitations or follow-up questions are recorded;
- no unrelated changes are included.

## Forbidden Changes Without Human Approval

Do not change these without explicit human approval:

- `README.md` shall only be updated by Human only.
- prompt assets under `assets/prompts/`;
- prompt path/model configuration values;
- accounting policy, tax interpretation, export business rules, or approval thresholds;
- database migrations or destructive data operations;
- production deployment configuration;
- secrets or credential handling;
- branch protection, release, or deployment behavior;
- direct pushes, merges, rebases, or force-pushes involving `main`.
- speculative `Finelor`, jurisdiction, or future-architecture decisions not documented in a Ready task.

## Stop Conditions

Stop and request human input when:

- the task is unclear or acceptance criteria are missing;
- required secrets or external credentials are unavailable;
- a change requires accounting, tax, or legal judgment not already documented;
- required tests fail for reasons you cannot confidently attribute;
- documentation contradicts code and the intended behavior is unclear;
- the requested change would touch forbidden areas without approval.
