# Skills Guide

Finelor skills are packaged instruction sets for the assistant.

They let us teach the assistant a focused workflow without changing the core agent loop every time we want a new operational behavior. A skill can describe:

- when to use a workflow
- when not to use it
- how to use existing tools
- which supporting references or templates belong to that workflow

The current design is intentionally lightweight:

- skills are loaded from disk into the skills registry at startup
- the assistant sees a summary of available skills in prompt context
- the assistant can load a skill and its supporting files on demand through skill tools

## How Skills Work

At a high level, the runtime flow is:

1. Finelor loads skill packages from `assets/skills/`
2. `SkillRegistry` caches the parsed skills in memory
3. the assistant prompt includes a summary list of available skills
4. when needed, the assistant can call:
   - `skill_list` to see available skills
   - `skill_view(name)` to load the main skill instructions
   - `skill_view(name, path)` to load a specific file from `references/` or `templates/`

This is a progressive-disclosure model:

- prompt context stays compact
- full skill content is only loaded when the assistant needs it

## Skill Package Structure

Each skill lives in its own directory under `assets/skills/`.

```text
assets/skills/
  <skill-id>/
    SKILL.md
    references/
      *.md
    templates/
      *
```

### Required

- `SKILL.md`

### Optional

- `references/`
  - supporting Markdown files with rules, caveats, field notes, or deeper guidance
- `templates/`
  - reusable output shapes or workflow artifacts

## What Finelor Loads

When a skill is loaded, Finelor stores:

### Main skill

From `SKILL.md` YAML frontmatter:

- `name`
- `description`
- `category`
- optional metadata such as:
  - `version`
  - `author`
  - `keywords`
  - `depends_on`
  - `file_extensions`
  - `file_globs`
  - extra frontmatter fields

From the Markdown body:

- the full raw body as one Markdown instruction document

Finelor does not parse Markdown headings into typed fields. Headings like
`Overview`, `When to Use`, `When NOT to Use`, `Workflow`, and `Examples` are
recommended structure for clarity, not a parser contract.

### References

For each file in `references/*.md`, Finelor stores:

- `name`
- `path`
- `content`

### Templates

For each file in `templates/`, Finelor stores:

- `name`
- `path`
- `content`

## Creating a Good Skill

A good skill should be:

- narrow
- explicit
- tool-aware
- honest about limits
- easy for the assistant to follow without guessing

Prefer skills that tell the assistant:

- exactly when the skill should trigger
- exactly which tools to use first
- exactly what output shape to produce
- exactly what not to claim

Avoid writing skills that:

- assume tools exist when they do not
- imply full historical coverage if tools only expose recent data
- mix several unrelated workflows into one package
- leave the assistant to infer critical rules from vague prose

## Recommended `SKILL.md` Shape

Use this as the default structure for new skills.

```md
---
name: Example Skill Name
description: Short explanation of what this skill helps the assistant do.
category: operations
keywords:
  - example
  - workflow
  - audit
---

# Example Skill Name

## Overview

Explain the goal of the skill and the default scope.

## When to Use

- user request pattern 1
- user request pattern 2
- user request pattern 3

## When NOT to Use

- out-of-scope case 1
- out-of-scope case 2
- unsupported case 3

## Workflow

1. Tell the assistant which tool to call first.
2. Tell the assistant which fields or outputs matter.
3. Tell the assistant how to structure the answer.
4. Tell the assistant what limitation note to include when needed.
5. Tell the assistant what follow-up tools are optional versus default.

## Examples

- Example user request 1
- Example user request 2
```

## Simple Skill Template

Use this as a starting point for a new skill package.

```text
assets/skills/example-skill/
  SKILL.md
  references/
    rules.md
  templates/
    response-template.md
```

Example `SKILL.md`:

```md
---
name: Example Review Skill
description: Review recent documents with a simple checklist and report visible issues.
category: quality-control
keywords:
  - review
  - checklist
  - quality
---

# Example Review Skill

## Overview

Use this skill to review recent visible documents with a deterministic checklist.

## When to Use

- the user asks for a checklist
- the user asks for a missing-fields audit
- the user asks for a recent document quality review

## When NOT to Use

- the user asks for a full historical report
- the user asks for an export action
- the user asks for a workflow mutation

## Workflow

1. Call `list_documents` first.
2. Use the returned items as the scope unless the user explicitly narrows it further.
3. Apply the checklist only to visible tool output.
4. Use `references/rules.md` for detailed rules if needed.
5. Use `templates/response-template.md` when the user wants the standard response shape.

## Examples

- `Give me a review checklist for recent documents`
- `Check recent documents for visible missing fields`
```

Example `references/rules.md`:

```md
# Rules

- Only report what is visible in tool output.
- Do not infer missing values.
- Do not claim accounting readiness unless a tool explicitly proves it.
```

Example `templates/response-template.md`:

```md
## <document_short_ref>
Status: `<status>`

- Check 1: complete or missing
- Check 2: complete or missing
- Missing items: ...
```

## Authoring Tips

When writing a skill:

- keep the frontmatter description short and concrete
- make the `Workflow` section actionable, not aspirational
- name supporting files clearly
- use `references/` for rules and caveats
- use `templates/` for reusable response shapes
- keep supporting files small and purpose-specific

If the assistant should be careful about scope, say so plainly in the skill:

- recent documents only
- visible tool output only
- no full-workspace guarantee
- no mutation unless explicitly requested

## Current Assistant Behavior

Today, the assistant can:

- see available skill summaries from prompt context
- load full skill instructions with `skill_view(name)`
- load a specific reference or template with `skill_view(name, path)`

That means a good skill should work well in layers:

1. a clear summary in frontmatter
2. strong main instructions in `SKILL.md`
3. optional deeper files in `references/` and `templates/`

## Checklist for New Skills

Before adding a new skill, check:

- does it solve one clear workflow?
- does the description say what it actually does?
- does `When NOT to Use` block misleading use cases?
- does the workflow name the right existing tools?
- does it clearly state any data-scope limitation?
- do references/templates add real value instead of noise?
- can the assistant follow it without guessing?
