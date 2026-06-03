# Hermes Skills System Analysis & Finelor Implementation Plan

## Executive Summary

After deep analysis of the Hermes Agent codebase, this document provides a comprehensive breakdown of how Hermes implements its skill system, the core architectural patterns used, and a detailed implementation plan for bringing equivalent agentic skill capabilities to Finelor.

**Key Finding**: Hermes' skill system is a document-centered knowledge architecture where skills are self-contained markdown documents (SKILL.md) with YAML frontmatter that get dynamically discovered, loaded into context, and executed via tool calls. This "document-as-code" approach provides progressive disclosure, version control compatibility, and natural extensibility.

---

## Part 1: Hermes Skills System - Deep Architecture Analysis

### 1.1 Core Philosophy: Progressive Disclosure Document Architecture

Hermes treats skills as **living documents** rather than compiled code. This design philosophy prioritizes:

- **Human-readable knowledge**: Skills are markdown files that can be edited, versioned, and reasoned about
- **Progressive loading**: Metadata (name, description) is cheap; full content loads on-demand
- **Self-describing capabilities**: Skills declare when they should be used, what they need, and how to verify success
- **Community shareability**: Standard format (agentskills.io compatible) enables skill registries

### 1.2 Document Structure

```
skills/
├── <category>/
│   ├── DESCRIPTION.md           # Category description with YAML frontmatter
│   └── <skill-name>/
│       ├── SKILL.md             # Main skill document (REQUIRED)
│       ├── references/          # Supporting documentation
│       │   ├── api.md
│       │   └── examples.md
│       ├── templates/           # Jinja2 templates for outputs
│       │   └── report.md.j2
│       ├── scripts/             # Executable scripts (Python, bash, etc.)
│       │   └── validate.py
│       └── assets/              # Static files, images, config samples
│           └── config.yaml
```

### 1.3 SKILL.md Format Specification

```yaml
---
name: skill-name                  # Required, max 64 chars, lowercase + hyphens
description: "Use when..."       # Required, max 1024 chars, MUST start with trigger
tags: [tag1, tag2]               # Optional categorization
version: 1.0.0                    # Optional semantic version
author: "Name"                   # Optional attribution
license: MIT                     # Optional license
platforms: [linux, macos]        # Optional OS restrictions
toolsets: [terminal, file]       # Optional: only show when these toolsets are active
tools: [read_file, terminal]     # Optional: only show when these tools are available
prerequisites:                   # Optional runtime requirements
  env_vars: [API_KEY]
  commands: [curl, jq]
required_environment_variables: # Optional structured requirements
  - name: API_KEY
    prompt: "Enter your API key"
    help: "Get from dashboard.example.com"
    optional: false
setup:                          # Optional setup instructions
  help: "See https://docs.example.com/setup"
  collect_secrets:
    - env_var: API_KEY
      prompt: "API key"
metadata:                       # Optional arbitrary metadata
  hermes:
    tags: [ml, training]
    related_skills: [axolotl, unsloth]
    homepage: "https://..."
    fallback_for_toolsets: [web]
    conditions:                 # Conditional skill visibility
      toolsets: [terminal]
      tools: [execute_code]
---

# Skill Title

## Overview
One or two paragraphs explaining what this skill enables.

## When to Use
- Specific triggers for when this skill applies
- "Don't use for:" counter-indications

## Prerequisites
Any setup needed before using this skill.

## Core Workflow
Step-by-step instructions for executing the skill.

## Common Pitfalls
Numbered list of mistakes and how to avoid them.

## Verification Checklist
- [ ] Checkbox list of success criteria

## One-Shot Recipes
Named scenarios → concrete command sequences.
```

### 1.4 Discovery System

**Multi-Source Discovery** (`tools/skills_tool.py`):

1. **Primary**: `~/.hermes/skills/` - User-local skills directory
2. **External**: Additional directories configured in `skills.external_dirs`
3. **In-Package**: Bundled skills shipped with hermes-agent package
4. **Plugin-Provided**: Skills from loaded plugins (`plugin:skill` qualified names)

**Discovery Algorithm** (`iter_skill_index_files`):
- Recursively scan for `SKILL.md` files
- Skip excluded directories (`.git`, `__pycache__`, `node_modules`, etc.)
- Parse frontmatter for metadata
- Platform compatibility filtering (`platforms: [linux, macos]`)
- Disabled skill filtering (respects user config)
- Tool/Toolset conditional filtering (only show relevant skills)

### 1.5 Loading System

**Three-Tier Progressive Disclosure** (`skill_view` function):

| Tier | Function | Data Returned | Cost |
|------|----------|-----------------|------|
| 1 | `skills_list()` | name, description, category only | ~10 tokens/skill |
| 2 | `skill_view(name)` | Full SKILL.md content + linked files list | ~500-3000 tokens |
| 3 | `skill_view(name, file_path="...")` | Specific linked file content | Variable |

**Loading Process**:
1. Resolve skill name to file path (handles collisions)
2. Parse YAML frontmatter
3. Validate platform compatibility
4. Check disabled status
5. Apply template variable substitution
6. Execute inline shell expansion (if enabled)
7. Return JSON with content + linked_files catalog

### 1.6 Runtime Integration

**System Prompt Injection** (`build_skills_system_prompt`):

Every session gets skills automatically injected into system prompt:

```markdown
## Skills (mandatory)
Before replying, scan the skills below. If a skill matches or is even partially relevant 
to your task, you MUST load it with skill_view(name) and follow its instructions.
Err on the side of loading — it is always better to have context you don't need than 
to miss critical steps, pitfalls, or established workflows.

<available_skills>
  mlops:
    - axolotl: Use when fine-tuning LLMs with YAML configs
    - unsloth: Use when you need 2-5x faster LoRA training
  software-development:
    - hermes-agent: Configure, extend, or contribute to Hermes Agent
</available_skills>
```

**Skill Execution via Tools**:
- `skills_list(category=None)` - Discovery
- `skill_view(name, file_path=None)` - Loading
- `skill_manage(action, name, ...)` - CRUD operations

**Slash Command Integration**:
- Skills auto-register as `/skill-name` commands
- Normalized naming: lowercase, hyphens, alphanumeric only
- Collision detection across categories

### 1.7 Caching & Performance

**Multi-Layer Caching**:

1. **In-Process LRU Cache**: 8-entry cache for `build_skills_system_prompt`
   - Key: `(skills_dir, external_dirs, tools, toolsets, platform, disabled_set)`
   - Invalidates on filesystem changes

2. **Disk Snapshot**: `.skills_prompt_snapshot.json`
   - Pre-parsed metadata for fast cold-start
   - Regenerated on cache miss with file modification detection

3. **Tool Registry Cache**: Skill tools registered once at startup
   - `skills_list` and `skill_view` available in tool schema

### 1.8 State Management

**Configuration Persistence**:

```yaml
# ~/.hermes/config.yaml
skills:
  disabled: [skill-a, skill-b]              # Global disabled list
  platform_disabled:                        # Per-platform overrides
    telegram: [skill-c]
    cli: []
  external_dirs: ["~/team-skills"]         # Additional skill sources
```

**Usage Tracking** (`tools/skill_usage.py`):
- `bump_use(skill_name)` - Increment usage counter
- Curator background process archives unused skills
- `.usage.json` tracks engagement for lifecycle management

### 1.9 Security Model

**Prompt Injection Prevention**:
- Pattern scanning: `ignore previous instructions`, `you are now`, `<system>` tags
- Content sanitization before system prompt injection
- Warnings logged for suspicious patterns

**Path Traversal Protection**:
- `validate_within_dir()` checks resolved paths stay within skill directory
- Rejects `../` components in file_path parameter
- Symlink resolution safety

**Cross-Profile Guard**:
- Active profile tracking prevents accidental writes to other profiles
- Hermes Home isolation: `~/.hermes/profiles/<name>/`

### 1.10 Plugin Skill Extension

**Qualified Names**: `plugin:skill-name`
- Namespace isolation from local skills
- Plugin manager routes to correct provider
- Sibling skill discovery within namespace

---

## Part 2: Extracted Core Patterns & Techniques

### 2.1 Document-First Knowledge Engineering

**Principle**: Knowledge is authored, not coded.

```rust
// Finelor equivalent: Skill document as source of truth
pub struct Skill {
    pub metadata: SkillMetadata,        // Parsed from YAML frontmatter
    pub content: String,                 // Markdown body
    pub path: PathBuf,                   // Source file location
}

impl Skill {
    pub fn should_activate(&self, context: &RuntimeContext) -> bool {
        // Self-describing activation logic from frontmatter
        self.metadata.matches_context(context)
    }
}
```

### 2.2 Progressive Context Disclosure

**Pattern**: Don't load what you don't need.

```rust
// Tier 1: Cheap metadata only
pub fn list_skills() -> Vec<SkillSummary> { ... }

// Tier 2: Full content on demand
pub fn load_skill(name: &str) -> Result<Skill, Error> { ... }

// Tier 3: Linked resources as needed
pub fn load_skill_file(skill: &str, path: &str) -> Result<String, Error> { ... }
```

### 2.3 Declarative Triggering

**Pattern**: Skills declare when they apply, system respects declarations.

```yaml
# Frontmatter conditions
toolsets: [terminal, file]          # Only show if user has these
tools: [execute_code]                 # Only show if available
conditions:
  toolsets: [terminal]
  min_context_length: 8000
```

### 2.4 Tool-Native Execution

**Pattern**: Skills execute through standard tool calls, not special pathways.

```rust
// Skills ARE tools in the tool-based architecture
Tool::SkillView { name: String, file_path: Option<String> }
Tool::SkillsList { category: Option<String> }
Tool::SkillManage { action: SkillAction, ... }
```

### 2.5 Self-Describing Verification

**Pattern**: Skills define their own success criteria.

```markdown
## Verification Checklist
- [ ] Output file exists at expected path
- [ ] File size > 0 bytes
- [ ] Content validates against schema
```

### 2.6 Mutable Skill Lifecycle

**Pattern**: Skills can be created, patched, and evolved by the agent.

```rust
pub enum SkillAction {
    Create { content: String, category: String },
    Patch { name: String, old: String, new: String },
    Delete { name: String },
}
```

---

## Part 3: Finelor Implementation Plan

### 3.1 Vision: Agentic Skills for Accounting & Business

**Goal**: Enable Finelor's assistant to extend its capabilities through skills for:
- Invoice generation and management
- Salary payslip generation
- Financial reporting
- Tax document preparation
- Audit trail analysis
- Multi-currency conversions
- Regulatory compliance checks

### 3.2 Proposed Architecture

```
finelor/
├── src/
│   ├── skills/                     # NEW: Skill system module
│   │   ├── mod.rs                  # Core types and traits
│   │   ├── discovery.rs            # File system scanning
│   │   ├── loader.rs               # Frontmatter parsing & content loading
│   │   ├── registry.rs             # Skill registration and lookup
│   │   ├── executor.rs             # Skill execution runtime
│   │   ├── prompt_builder.rs       # System prompt integration
│   │   └── builtin/                # Built-in Finelor skills
│   │       ├── invoice-generation/
│   │       │   └── SKILL.md
│   │       ├── payslip-generation/
│   │       │   └── SKILL.md
│   │       ├── financial-reporting/
│   │       │   └── SKILL.md
│   │       └── ...
│   └── messaging/
│       ├── tools.rs                # EXTEND: Add skill tools
│       └── prompt.rs               # EXTEND: Inject skills into prompt
└── skills/                         # NEW: User skill storage
    └── README.md
```

### 3.3 Core Types (Rust)

#### Skill Definition

```rust
// src/skills/mod.rs

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Unique skill identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillId(pub String);

/// Skill metadata from YAML frontmatter
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// When to trigger this skill (document ID patterns, intent patterns)
    #[serde(default)]
    pub triggers: TriggerConditions,
    /// Required capabilities
    #[serde(default)]
    pub requires: CapabilityRequirements,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TriggerConditions {
    /// Trigger when user mentions these document types
    #[serde(default)]
    pub document_types: Vec<String>,
    /// Trigger on these intent patterns
    #[serde(default)]
    pub intents: Vec<String>,
    /// Trigger keywords in query
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CapabilityRequirements {
    /// Required stage access (intake, processing, review, etc.)
    #[serde(default)]
    pub stages: Vec<String>,
    /// Required user permissions
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// Full skill with content
#[derive(Debug, Clone)]
pub struct Skill {
    pub id: SkillId,
    pub metadata: SkillMetadata,
    pub content: String,           // Markdown body (after frontmatter)
    pub source_path: PathBuf,
    pub linked_files: LinkedFiles, // references/, templates/, scripts/
}

#[derive(Debug, Clone, Default)]
pub struct LinkedFiles {
    pub references: Vec<PathBuf>,
    pub templates: Vec<PathBuf>,
    pub scripts: Vec<PathBuf>,
    pub assets: Vec<PathBuf>,
}

/// Skill summary for lightweight listing
#[derive(Debug, Clone, Serialize)]
pub struct SkillSummary {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub category: String,
}
```

#### Skill Registry

```rust
// src/skills/registry.rs

use std::collections::HashMap;
use tokio::sync::RwLock;

/// Thread-safe skill registry with hot-reload support
pub struct SkillRegistry {
    /// name -> skill lookup
    skills: RwLock<HashMap<String, Skill>>,
    /// category -> skills index
    by_category: RwLock<HashMap<String, Vec<String>>>,
    /// File watcher for hot-reload
    watcher: Option<notify::RecommendedWatcher>,
}

impl SkillRegistry {
    /// Scan skills directories and populate registry
    pub async fn discover(&self, paths: &[PathBuf]) -> Result<(), SkillError> {
        // Implementation: scan directories, parse SKILL.md files
    }
    
    /// Get skill by name
    pub async fn get(&self, name: &str) -> Option<Skill> {
        self.skills.read().await.get(name).cloned()
    }
    
    /// List all skills (lightweight summaries)
    pub async fn list(&self) -> Vec<SkillSummary> {
        // Implementation: return summaries only
    }
    
    /// Find skills matching context
    pub async fn find_relevant(&self, context: &RuntimeContext) -> Vec<SkillSummary> {
        // Implementation: filter by triggers, requirements
    }
}
```

### 3.4 Frontmatter Parser

```rust
// src/skills/loader.rs

use regex::Regex;
use yaml_rust::{YamlLoader, Yaml};

/// Parse SKILL.md with YAML frontmatter
/// Format:
/// ---
/// name: skill-name
/// description: "..."
/// ---
/// # Markdown content
pub fn parse_skill(content: &str) -> Result<(SkillMetadata, String), ParseError> {
    lazy_static::lazy_static! {
        static ref FRONTMATTER_RE: Regex = Regex::new(
            r"^---\s*\n(.*?)\n---\s*\n(.*)$"
        ).unwrap();
    }
    
    if let Some(caps) = FRONTMATTER_RE.captures(content) {
        let yaml_str = caps.get(1).unwrap().as_str();
        let body = caps.get(2).unwrap().as_str();
        
        let metadata: SkillMetadata = serde_yaml::from_str(yaml_str)?;
        Ok((metadata, body.to_string()))
    } else {
        Err(ParseError::NoFrontmatter)
    }
}

/// Scan directory for SKILL.md files
pub async fn scan_skills_dir(path: &Path) -> Result<Vec<PathBuf>, ScanError> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(path).await?;
    
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_dir() {
            // Recurse into subdirectories (categories)
            files.extend(Box::pin(scan_skills_dir(&path)).await?);
        } else if path.file_name() == Some("SKILL.md".as_ref()) {
            files.push(path);
        }
    }
    
    Ok(files)
}
```

### 3.5 Integration with Messaging System

#### Extend Tools

```rust
// src/messaging/tools.rs

/// Skill-related tools for the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case")]
pub enum SkillTool {
    /// List available skills
    #[serde(rename = "skills_list")]
    SkillsList { 
        category: Option<String>,
        filter: Option<String>,
    },
    
    /// View full skill content
    #[serde(rename = "skill_view")]
    SkillView { 
        name: String,
        file_path: Option<String>,
    },
    
    /// Create or update a skill
    #[serde(rename = "skill_manage")]
    SkillManage {
        action: SkillManageAction,
        name: String,
        content: Option<String>,
        category: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillManageAction {
    Create,
    Patch,
    Delete,
}

impl SkillTool {
    pub async fn execute(&self, ctx: &ToolContext) -> ToolResult {
        match self {
            SkillTool::SkillsList { category, filter } => {
                let registry = ctx.skill_registry();
                let mut skills = registry.list().await;
                
                if let Some(cat) = category {
                    skills.retain(|s| s.category == *cat);
                }
                
                // Format as JSON for LLM consumption
                Ok(json!({
                    "success": true,
                    "skills": skills,
                    "count": skills.len(),
                    "hint": "Use skill_view(name) to see full content"
                }))
            }
            
            SkillTool::SkillView { name, file_path } => {
                let registry = ctx.skill_registry();
                let skill = registry.get(name).await
                    .ok_or_else(|| ToolError::SkillNotFound(name.clone()))?;
                
                if let Some(path) = file_path {
                    // Load specific file from skill directory
                    let content = skill.load_linked_file(path).await?;
                    Ok(json!({
                        "success": true,
                        "content": content,
                        "path": path
                    }))
                } else {
                    Ok(json!({
                        "success": true,
                        "name": skill.metadata.name,
                        "content": skill.content,
                        "description": skill.metadata.description,
                        "linked_files": skill.linked_files.summarize()
                    }))
                }
            }
            
            // ... other tool implementations
        }
    }
}
```

#### Prompt Integration

```rust
// src/messaging/prompt.rs

/// Build system prompt with skills section
pub async fn assemble_system_prompt(
    soul: &str,
    skills: &SkillRegistry,
) -> String {
    let mut parts = vec![soul.to_string()];
    
    // Add skills guidance
    let skill_list = skills.list().await;
    if !skill_list.is_empty() {
        let skills_section = build_skills_prompt(&skill_list);
        parts.push(skills_section);
    }
    
    parts.join("\n\n")
}

fn build_skills_prompt(skills: &[SkillSummary]) -> String {
    let mut lines = vec![
        "## Available Skills".to_string(),
        "".to_string(),
        "Before responding, check if any skill matches your task. Skills contain".to_string(),
        "proven workflows, API patterns, and Finelor-specific procedures. Load a skill".to_string(),
        "with skill_view(name) when relevant.".to_string(),
        "".to_string(),
        "<available_skills>".to_string(),
    ];
    
    // Group by category
    let by_category: HashMap<String, Vec<&SkillSummary>> = 
        skills.iter().fold(HashMap::new(), |mut map, skill| {
            map.entry(skill.category.clone())
                .or_default()
                .push(skill);
            map
        });
    
    for (category, items) in by_category {
        lines.push(format!("  {}:", category));
        for skill in items {
            lines.push(format!("    - {}: {}", skill.name, skill.description));
        }
    }
    
    lines.push("</available_skills>".to_string());
    lines.join("\n")
}
```

### 3.6 Built-in Finelor Skills

Proposed skill catalog for Finelor:

```
skills/
├── accounting/                     # Core accounting skills
│   ├── invoice-generation/
│   │   └── SKILL.md
│   ├── payment-reconciliation/
│   │   └── SKILL.md
│   ├── ledger-posting/
│   │   └── SKILL.md
│   └── trial-balance/
│       └── SKILL.md
├── payroll/                        # Payroll skills
│   ├── payslip-generation/
│   │   └── SKILL.md
│   ├── tax-calculation/
│   │   └── SKILL.md
│   └── leave-accrual/
│       └── SKILL.md
├── reporting/                      # Reporting skills
│   ├── financial-statements/
│   │   └── SKILL.md
│   ├── management-reports/
│   │   └── SKILL.md
│   └── regulatory-filing/
│       └── SKILL.md
├── documents/                      # Document processing
│   ├── ocr-extraction/
│   │   └── SKILL.md
│   ├── document-classification/
│   │   └── SKILL.md
│   └── multi-currency/
│       └── SKILL.md
└── workflows/                      # Business workflows
    ├── approval-routing/
    │   └── SKILL.md
    ├── audit-trail/
    │   └── SKILL.md
    └── export-formats/
        └── SKILL.md
```

### 3.7 Skill Lifecycle

#### Creation Flow

```
1. User or agent calls skill_manage(action='create')
2. Validate frontmatter (name format, required fields)
3. Write to skills/<category>/<name>/SKILL.md
4. Trigger registry hot-reload
5. Available immediately in skills_list
6. Included in next system prompt rebuild
```

#### Update Flow

```
1. User calls skill_manage(action='patch', old=..., new=...)
2. Load existing skill content
3. Apply fuzzy patch (using similar algorithm to patch tool)
4. Validate resulting document
5. Write back to disk
6. Invalidate registry cache
```

### 3.8 Security Considerations

```rust
// src/skills/security.rs

/// Security scanner for skill content
pub struct SkillSecurityScanner;

impl SkillSecurityScanner {
    /// Patterns that may indicate prompt injection
    const SUSPICIOUS_PATTERNS: &[&str] = &[
        "ignore previous instructions",
        "ignore all previous",
        "you are now",
        "disregard your",
        "forget your instructions",
        "new instructions:",
        "system prompt:",
        "<system>",
        "]]>",
    ];
    
    pub fn scan(content: &str) -> SecurityScanResult {
        let lower = content.to_lowercase();
        let mut findings = Vec::new();
        
        for pattern in Self::SUSPICIOUS_PATTERNS {
            if lower.contains(pattern) {
                findings.push(SuspiciousPattern(*pattern));
            }
        }
        
        SecurityScanResult {
            passed: findings.is_empty(),
            findings,
        }
    }
}

/// Path validation for linked file loading
pub fn validate_skill_path(
    skill_dir: &Path,
    requested_path: &str,
) -> Result<PathBuf, PathError> {
    // Reject path traversal attempts
    if requested_path.contains("..") {
        return Err(PathError::PathTraversal);
    }
    
    let resolved = skill_dir.join(requested_path);
    let canonical = resolved.canonicalize()
        .map_err(|_| PathError::NotFound)?;
    
    // Ensure path stays within skill directory
    if !canonical.starts_with(skill_dir.canonicalize()?) {
        return Err(PathError::OutsideSkillDirectory);
    }
    
    Ok(canonical)
}
```

---

## Part 4: Implementation Phases

### Phase 1: Foundation (Week 1-2)

**Goal**: Core skill infrastructure

**Tasks**:
1. [ ] Create `src/skills/` module structure
2. [ ] Implement YAML frontmatter parser
3. [ ] Implement filesystem discovery (`scan_skills_dir`)
4. [ ] Define core types (`Skill`, `SkillMetadata`, `SkillRegistry`)
5. [ ] Build basic registry with in-memory caching
6. [ ] Add skill-related errors to error types

**Deliverable**: Skills can be discovered and loaded from filesystem

### Phase 2: Tool Integration (Week 2-3)

**Goal**: Agent can discover and load skills

**Tasks**:
1. [ ] Add `SkillTool` enum to messaging/tools.rs
2. [ ] Implement `skills_list` tool execution
3. [ ] Implement `skill_view` tool execution
4. [ ] Add skill tools to AccountantAgent tool schema
5. [ ] Wire tool calls through AgentGateway

**Deliverable**: Agent can list and view skills via tool calls

### Phase 3: Prompt Integration (Week 3-4)

**Goal**: Skills are available in system context

**Tasks**:
1. [ ] Extend prompt assembly to include skills section
2. [ ] Build `build_skills_prompt()` function
3. [ ] Cache invalidation on skill changes
4. [ ] Add soul.md guidance on skill usage
5. [ ] Test prompt token budget impact

**Deliverable**: Skills automatically injected into system prompt

### Phase 4: Built-in Skills (Week 4-5)

**Goal**: Initial skill library

**Tasks**:
1. [ ] Create skills/builtin/ directory structure
2. [ ] Write `invoice-generation/SKILL.md` (production-ready)
3. [ ] Include comprehensive tool examples
4. [ ] Add verification checklist
5. [ ] Document common pitfalls

**Deliverable**: Production-ready invoice generation skill

---

## Part 5: Example Skill for Finelor

### Invoice Generation Skill

```markdown
---
name: invoice-generation
description: "Use when generating, formatting, or managing customer invoices. Handles invoice creation from document data, template selection, and PDF generation."
tags: [accounting, invoicing, documents]
triggers:
  document_types: [invoice]
  intents: [generate_invoice, create_invoice, new_invoice]
  keywords: [invoice, bill, customer payment]
requires:
  stages: [processing]
  permissions: [write_documents]
---

# Invoice Generation

## Overview

This skill generates professional invoices from Finelor document data. It supports:
- Multiple invoice templates (standard, proforma, credit note)
- Automatic tax calculations
- Multi-currency support
- PDF generation with customizable branding

## When to Use

- Customer requests a new invoice
- Document is classified as "invoice" and needs generation
- User asks to "create an invoice for..."
- Converting a quote/sales order to invoice

**Don't use for:**
- Receipts (use `receipt-generation` skill)
- Purchase orders (use `purchase-order` skill)
- Internal memos

## Prerequisites

1. Document must have customer information extracted
2. Line items must be identified in the document
3. Company profile must be configured with:
   - Business name and address
   - Tax ID
   - Bank account details (if showing on invoice)

## Workflow

### Step 1: Extract Invoice Data

Use `get_document` to load the source document and identify:
- Customer details (name, address, tax ID if applicable)
- Line items (description, quantity, unit price)
- Invoice date and due date
- Payment terms

### Step 2: Determine Invoice Type

| Type | When to Use |
|------|-------------|
| standard | Regular customer invoice |
| proforma | Pre-payment invoice / quote |
| credit_note | Refund/return invoice |
| recurring | Subscription billing |

### Step 3: Generate Invoice

Call `prepare_invoice` (read-only) to:
- Validate all required fields
- Calculate subtotal, taxes, and total
- Select appropriate template
- Return preview data

### Step 4: User Confirmation

Present the invoice preview to user for confirmation.

### Step 5: Finalize

On confirmation, call `create_invoice` to:
- Save invoice to database
- Generate PDF
- Link to source document
- Return invoice number and download link

## Common Pitfalls

1. **Missing tax rates**: Always check company tax settings. Different products may have different VAT rates.

2. **Currency mismatch**: Verify customer currency matches line item currencies. Use `prepare_currency_conversion` if needed.

3. **Duplicate invoices**: Check existing documents for same customer/amount before generating new invoice.

4. **Payment terms**: Confirm payment terms with user if not clear from source document.

## Verification Checklist

- [ ] Customer name and address clearly shown
- [ ] All line items have valid quantities and prices
- [ ] Tax calculations are correct for jurisdiction
- [ ] Invoice total matches calculated sum
- [ ] Invoice number follows company sequence
- [ ] PDF renders correctly (text not cut off)
- [ ] Invoice linked to source document

## Example

**User**: "Create an invoice from document D12345"

**Process**:
```
1. get_document("D12345") → customer: "Acme Corp", items: [...]
2. prepare_invoice(document="D12345", template="standard")
   → preview: { subtotal: 1000.00, tax: 200.00, total: 1200.00 }
3. Confirm with user
4. create_invoice(confirmed=true) → invoice_id: INV-2024-0001
```

## Related Skills

- `payment-reconciliation`: Match payments to invoices
- `customer-statement`: Generate monthly customer statements
- `credit-note`: Handle refunds and adjustments
```

---

## Part 6: Migration Path

### For Existing Finelor Code

**Step 1**: Extract existing hardcoded workflows into skills
- AccountantAgent processing logic → `document-processing/SKILL.md`
- Validator validation rules → `validation/SKILL.md`
- Reviewer guidelines → `review/SKILL.md`

**Step 2**: Gradual adoption
- Start with 1-2 foundational skills
- Add new features as skills
- Refactor existing code into skills over time

**Step 3**: Skill governance
- Review skills for accuracy
- Archive outdated skills
- Promote community skills

---

## Conclusion

The Hermes skill system demonstrates that effective agent extensibility comes from treating knowledge as authored documents, not compiled code. By adopting this document-centered approach, Finelor can achieve:

1. **Self-improving agents**: The assistant learns by creating/updating skills from experience
2. **User customizability**: Users can create their own accounting workflows
3. **Community sharing**: Skill format enables sharing domain-specific accounting expertise
4. **Maintainability**: Markdown skills are easier to review than code changes
5. **Progressive disclosure**: Skills load only when needed, managing context budget

This implementation plan provides a clear pathway to bringing Hermes-grade agentic extensibility to Finelor while respecting its existing architecture and safety constraints.

---

## Appendix A: Skill-Tool Integration (ADDED)

### A.1 Skills Guide Tool Usage, Not Replace Tools

**Key Insight**: Skills don't add new tools to the system. They add **knowledge about how to use existing tools effectively**.

Think of it as:
- **Tools** = "Hands" (what the assistant *can* do)
- **Skills** = "Playbooks" (how to *use* those hands)

A skill tells the agent **which tools to call**, **when**, and **in what order**.

### A.2 How Skills Reference Tools in Practice

A skill workflow typically looks like this:

```markdown
## Workflow

### Step 1: Verify Document State
Before processing, check if the document is ready:

```json
{
  "tool": "document_status_summary",
  "params": {
    "document_ref": "<DOCUMENT_ID>"
  }
}
```

**Expected Response**: Document should be in "reviewed" or "accepted" state.

### Step 2: Load Document Data
Extract customer and line item information:

```json
{
  "tool": "get_document",
  "params": {
    "document_ref": "<DOCUMENT_ID>"
  }
}
```

### Step 3: Prepare Invoice (Validation)
Pre-validate before creating:

```json
{
  "tool": "prepare_invoice",
  "params": {
    "document_id": "<DOCUMENT_ID>",
    "template": "standard"
  }
}
```
```

### A.3 Runtime Execution Flow

```
User: "Generate an invoice for document D000123"
    │
    ▼
Assistant: "Invoice request → Check skills → Found invoice-generation!"
    │
    ▼
skill_view("invoice-generation") → Returns full workflow
    │
    ▼
Assistant follows steps:
    ├─ Step 1: Call document_status_summary(D000123)
    │           → Returns "extracted"
    │
    ├─ Step 2: Call get_document(D000123)
    │           → Returns customer data
    │
    ├─ Step 3: Call prepare_invoice(D000123)
    │           → Returns preview
    │
    └─ Step 4: Present to user, then create_invoice()
```

### A.4 Tool-Referencing Skill: Complete Example

```markdown
---
name: payment-reconciliation
description: "Use when matching incoming payments to outstanding invoices"
required_tools:                      # NEW: Explicit tool requirements
  - get_document
  - list_documents
  - document_status_summary
  - prepare_open_review
---

# Payment Reconciliation

## Required Tools Overview

| Tool Name | Used For |
|-----------|----------|
| `get_document` | Load payment document details |
| `list_documents` | Find outstanding invoices |
| `document_status_summary` | Check if payment is reconciled |
| `prepare_open_review` | Create review when match is unclear |

## Workflow

### Step 1: Load Payment Document

Call `get_document` with the payment reference:

```json
{
  "tool": "get_document",
  "params": {
    "document_ref": "{{payment_document_id}}"
  }
}
```

**Extract these fields**:
- Customer name (must match invoice customer exactly)
- Payment amount
- Payment date
- Reference number (may contain invoice number)

### Step 2: Find Outstanding Invoices

For the same customer, get pending invoices:

```json
{
  "tool": "list_documents",
  "params": {
    "document_type": "invoice",
    "customer_name": "{{customer_name}}",
    "status": ["pending", "partial"]
  }
}
```

### Step 3: Match Payment to Invoices

Check for exact matches by amount:

| Match Type | Criteria | Action |
|------------|----------|--------|
| **Exact** | Payment = Invoice total | Proceed to reconciliation |
| **Partial** | Payment < Invoice total | Create partial payment |
| **Overpayment** | Payment > Invoice total | Flag for review |
| **Multi-invoice** | References multiple invoices | Split payment |
| **No match** | No corresponding invoice | Create review |

### Step 4: If Unclear, Prepare Review

When matching isn't obvious:

```json
{
  "tool": "prepare_open_review",
  "params": {
    "document_ids": ["{{payment_id}}"],
    "reason": "payment_reconciliation",
    "note": "Payment from {{customer}} for {{amount}} doesn't clearly match any single invoice"
  }
}
```

## Common Pitfalls

1. **Auto-matching partial payments**: Always confirm partial matches with user
2. **Currency confusion**: Verify payment currency matches invoice currency before matching
3. **Misreading references**: "Payment for INV-123" might mean "Payment for INV-1234" if user mistyped

## Tool Output Interpretation

**document_status_summary returns**:
```json
{
  "document_id": "D000123",
  "status": "extracted",
  "extraction": {
    "confidence": 0.95,
    "fields": ["amount", "date", "customer"]
  }
}
```

**What this means**: Document is ready for processing.

**If status is "extracting"**: Wait or retry later.
**If status is "failed"**: Fix extraction issues first.

## Verification Checklist

- [ ] `get_document` returned all expected fields
- [ ] Customer name matches across payment and invoice
- [ ] Currency is consistent
- [ ] `prepare_reconciliation` validation passed
- [ ] User confirmed match before finalizing
```

### A.5 Conditional Skill Visibility Based on Tools

Skills can **require** specific tools to be available:

```rust
// src/skills/mod.rs - Addition to SkillMetadata

#[derive(Debug, Clone, Deserialize)]
pub struct ToolConditions {
    /// Only show if these tools are available
    pub tools: Vec<String>,
    /// Only show if these toolsets are available  
    pub toolsets: Vec<String>,
}

impl Skill {
    /// Check if this skill should be shown given available tools
    pub fn should_show(&self, available_tools: &HashSet<String>) -> bool {
        if self.metadata.conditions.tools.is_empty() {
            return true; // No tool requirements, always show
        }
        
        // ALL required tools must be present
        self.metadata.conditions.tools
            .iter()
            .all(|t| available_tools.contains(t))
    }
}
```

**Example**: Advanced analytics skill that requires `execute_code`:

```markdown
---
name: advanced-financial-analysis
description: "Use for complex financial modeling with Python"
conditions:
  tools: [execute_code]    # Only show if execute_code tool exists
  toolsets: [advanced]     # Only for 'advanced' tier
---

# Advanced Financial Analysis

## Workflow

This skill uses `execute_code` to run pandas analysis:

```json
{
  "tool": "execute_code",
  "params": {
    "code": "import pandas as pd\n# Financial analysis..."
  }
}
```
```

If `execute_code` is not available:
- Skill is **hidden** from skills list
- Assistant won't try to load it
- System prompt won't include it

### A.6 Skill-Tool Binding Component

```rust
// src/skills/tool_binding.rs

use std::collections::HashMap;

/// Maps tools to skills that use them
pub struct SkillToolBinder {
    /// Tool name -> skills that use this tool
    tool_to_skills: HashMap<String, Vec<SkillId>>,
    /// Tracks which skills are "active" (all tools available)
    active_skills: HashSet<SkillId>,
}

impl SkillToolBinder {
    /// Register a skill and map its required tools
    pub fn register_skill(&mut self, skill: &Skill) {
        for tool in &skill.metadata.required_tools {
            self.tool_to_skills
                .entry(tool.clone())
                .or_default()
                .push(skill.id.clone());
        }
    }
    
    /// Find skills that could help with a given tool call
    pub fn find_for_tool(&self, tool_name: &str) -> Vec<SkillId> {
        self.tool_to_skills
            .get(tool_name)
            .cloned()
            .unwrap_or_default()
    }
    
    /// Suggest relevant skills when tool fails
    pub fn suggest_for_error(&self, tool_error: &ToolError) -> Option<String> {
        // Example: "list_documents failed with 'timeout'"
        // → Suggest "troubleshooting" skill
        for (tool, skills) in &self.tool_to_skills {
            for skill_id in skills {
                // Check if skill has Error Handling section
                if self.has_error_guidance(skill_id, tool_error) {
                    return Some(format!(
                        "Skill '{}' may help with this error",
                        skill_id
                    ));
                }
            }
        }
        None
    }
}
```

### A.7 System Prompt Addition for Tool-Using Skills

Add this guidance section to help the assistant understand skill-tool relationships:

```markdown
## Skills and Their Tool Requirements

When a skill recommends using specific tools:
- The skill was designed assuming those tools are available
- Follow the skill's tool sequences carefully
- Skills may show exact JSON examples of tool calls to make
- If a skill step fails, check the "Common Pitfalls" section
- Skills can reference tools by their exact names in this system

**Available Tools You Can Use**:
- get_document, list_documents, document_status_summary
- prepare_invoice, create_invoice
- explain_document, explain_line_item
- prepare_open_review, prepare_retry_document
- list_pending_reviews, list_export_ready

**When a Skill Says To Use a Tool**:
1. Use the exact tool name shown in the skill
2. Follow the parameters shown in the skill's examples
3. Check the skill's "Expected Response" to interpret results
4. If results differ from expected, see "Common Pitfalls"
```

### A.8 Tool-Using Skill: Multiple Tool Chains

Some skills orchestrate complex multi-tool workflows:

```markdown
---
name: full-document-processing
description: "End-to-end document processing from intake to export"
required_tools:
  - get_document
  - document_status_summary
  - explain_document
  - prepare_open_review
  - list_pending_reviews
  - prepare_export_documents
---

# Full Document Processing

## Workflow

```
Get Document
    │
    ▼
Check Status
    │
    ├─ If "extracting" → Wait and recheck
    ├─ If "failed" → Explain failure
    ├─ If "needs_review" → Prepare review
    └─ If "extracted" → Continue
              │
              ▼
        Validate Data (explain_document)
              │
              ├─ If unclear → Prepare review
              └─ If clear → Continue
                        │
                        ▼
                  Final Processing
                        │
                        ▼
                  Queue for Export
```

## Step-by-Step

### Step 1: Get Document
```json
{"tool": "get_document", "params": {"document_ref": "{{doc_id}}"}}
```

### Step 2: Check Status
```json
{"tool": "document_status_summary", "params": {"document_ids": ["{{doc_id}}"]}}
```

### Step 3: Branch Based on Status

| Status | Tool Call | Next Step |
|--------|-----------|-----------|
| extracting | None | Wait, then retry Step 2 |
| failed | `explain_document` | Identify blocking issue |
| needs_review | `prepare_open_review` | Create review for human |
| extracted | `explain_document` | Validate extraction |

### Step 4: Validation
```json
{"tool": "explain_document", "params": {"document_ref": "{{doc_id}}", "question": "What are the total amounts and are they reasonable?"}}
```

### Step 5: Export (if validation passed)
```json
{"tool": "prepare_export_documents", "params": {"document_refs": ["{{doc_id}}"], "format": "quickbooks"}}
```
```

### A.9 Testing Tool-Using Skills

To verify a skill uses tools correctly:

```rust
#[tokio::test]
async fn test_invoice_skill_uses_correct_tools() {
    let skill = load_skill("invoice-generation").await;
    
    // Check skill mentions required tools
    assert!(skill.content.contains("document_status_summary"));
    assert!(skill.content.contains("get_document"));
    assert!(skill.content.contains("prepare_invoice"));
    
    // Verify metadata lists required tools
    assert!(skill.metadata.required_tools.contains("prepare_invoice"));
}

#[tokio::test]
async fn test_skill_conditional_on_tools() {
    let registry = SkillRegistry::new();
    
    // Without execute_code tool, advanced-analysis skill hidden
    let skills = registry.list_with_tools(&["get_document"]).await;
    assert!(!skills.iter().any(|s| s.name == "advanced-financial-analysis"));
    
    // With execute_code tool, skill is visible
    let skills = registry.list_with_tools(&["get_document", "execute_code"]).await;
    assert!(skills.iter().any(|s| s.name == "advanced-financial-analysis"));
}
```

### A.10 Skill-Tool Integration Summary

| Aspect | Implementation |
|--------|---------------|
| **Skills define workflows** | Markdown documents with step-by-step instructions |
| **Skills reference tools** | By exact tool names in workflow sections |
| **Skill metadata** | Lists `required_tools` for conditional visibility |
| **Runtime binding** | Registry checks tool availability before showing skills |
| **Tool suggestion** | binder.suggest_for_error() can recommend skills |
| **Prompt guidance** | System prompt includes skill-tool relationship guidance |

### A.11 Phase 4 Addition: Built-in Tool-Using Skills

The built-in skill library should include tool-using examples:

```
skills/builtin/
├── accounting/
│   ├── invoice-generation/
│   │   └── SKILL.md          # Uses: get_document, prepare_invoice, create_invoice
│   ├── payment-processing/
│   │   └── SKILL.md          # Uses: get_document, list_documents, prepare_open_review
│   └── reconciliation/
│       └── SKILL.md          # Uses: list_documents, explain_document
├── validation/
│   ├── document-validation/
│   │   └── SKILL.md          # Uses: document_status_summary, explain_document
│   └── tax-calculation-review/
│       └── SKILL.md          # Uses: explain_line_item, prepare_retry_document
└── export/
    └── export-preparation/
        └── SKILL.md          # Uses: list_export_ready, prepare_export_documents
```

---

## Appendix B: Complete Implementation Checklist (ADDED)

### Core Components

- [ ] `SkillMetadata` struct with frontmatter support
- [ ] YAML frontmatter parser (`parse_skill`)
- [ ] `SkillRegistry` with `get()` and `list()` methods
- [ ] `SkillDiscovery` filesystem scanner

### Tool Integration

- [ ] Add `SkillTool` enum to messaging tools
- [ ] Implement `skills_list` tool execution
- [ ] Implement `skill_view` tool execution
- [ ] Wire tools through `AgentGateway`

### Prompt Integration

- [ ] `build_skills_prompt()` function
- [ ] Extend system prompt assembly
- [ ] LRU cache for skills prompt
- [ ] Cache invalidation on changes

### Skill-Tool Binding (NEW)

- [ ] `SkillToolBinder` component
- [ ] `required_tools` frontmatter field
- [ ] Conditional skill visibility based on tools
- [ ] Tool→skill mapping for suggestions

### Built-in Skills

- [ ] Create `skills/builtin/` directory
- [ ] Invoice generation skill (with full tool examples)
- [ ] Payment reconciliation skill
- [ ] Document validation skill
- [ ] At least one skill per Finelor stage

### Management

- [ ] `skill_manage(action='create')`
- [ ] `skill_manage(action='patch')`
- [ ] `skill_manage(action='delete')`
- [ ] Content validation
- [ ] Security scanning

### Production

- [ ] File watcher for hot-reload
- [ ] Usage tracking
- [ ] Skill versioning
- [ ] External skill directories
