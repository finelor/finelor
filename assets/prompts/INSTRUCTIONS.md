# Prompt Pack Instructions

Use this guide to add a new company prompt pack under `assets/prompts/`.

## 1) Create a new pack folder

Create a new folder:

```bash
mkdir -p assets/prompts/<company_or_pack_name>
```

## 2) Copy the required prompt files

Copy the baseline files from the existing Sweden pack:

- `vision_agent.md`
- `accountant_agent.md`
- `validator_agent.md`

Example:

```bash
cp assets/prompts/sweden/vision_agent.md assets/prompts/<company_or_pack_name>/
cp assets/prompts/sweden/accountant_agent.md assets/prompts/<company_or_pack_name>/
cp assets/prompts/sweden/validator_agent.md assets/prompts/<company_or_pack_name>/
```

## 3) Shared prompt default

Communication SOUL is shared by default and lives in:

- `assets/prompts/_shared/assistant_agent_soul.md`

Keep it in `_shared` unless you intentionally need company-specific communication behavior.

## 4) Wire the new pack in config

Point prompt paths to your new pack in `config.yaml` or `config.local.yaml`:

- `ollama.vision_prompt_path`
- `ollama.accountant_prompt_path`

You can also override via environment/config layering where needed.

## 5) Minimal validation

- Confirm all configured prompt paths exist.
- Start the app and ensure it boots normally.
- Run:

```bash
cargo check --features ssr
```

- Do one quick manual flow:
  - ingest a document (vision/accountant path)
  - run one chat interaction (communication/SOUL path)

