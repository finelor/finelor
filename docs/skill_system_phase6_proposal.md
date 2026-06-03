# Finelor Skills System - Phase 6 Advanced Features
## Future Implementation Proposal

**Status**: Deferred (not part of initial implementation)  
**Priority**: High (for production maturity)  
**Estimated Timeline**: Weeks 7-10 (after initial 5-phase rollout)

---

## Objective

Extend the foundational skill system (Phases 1-5) with production-grade features enabling enterprise deployment, team collaboration, and long-term maintainability.

---

## Phase 6 Features

### 6.1 Hot-Reload via File Watcher

**Current State**: Skills loaded once at startup, restart required for updates  
**Desired State**: Skills update live without restart

#### Implementation

```rust
// src/skills/watcher.rs

use notify::{Watcher, RecursiveMode, watcher};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct SkillFileWatcher {
    watcher: notify::RecommendedWatcher,
    change_tx: mpsc::Sender<PathBuf>,
}

impl SkillFileWatcher {
    pub fn new(registry: Arc<SkillRegistry>) -> Result<Self, Error> {
        let (tx, mut rx) = mpsc::channel(100);
        
        let watcher = watcher(
            move |res: notify::Result<notify::Event>| {
                match res {
                    Ok(event) => {
                        for path in event.paths {
                            if path.ends_with("SKILL.md") {
                                let _ = tx.try_send(path);
                            }
                        }
                    }
                    Err(e) => log::error!("File watch error: {:?}", e),
                }
            },
            Duration::from_secs(2), // Debounce
        )?;
        
        // Spawn async handler
        tokio::spawn(async move {
            while let Some(path) = rx.recv().await {
                log::info!("Skill file changed: {}", path.display());
                registry.reload_skill(&path).await;
            }
        });
        
        Ok(Self { watcher, change_tx: tx })
    }
    
    pub fn watch(&mut self, path: &Path) -> Result<(), Error> {
        self.watcher.watch(path, RecursiveMode::Recursive)?;
        Ok(())
    }
}
```

#### User Experience

```bash
# User edits skill file
vim ~/.config/finelor/skills/accounting/invoice-generation/SKILL.md
# Save file
# → Skill automatically reloaded (no restart)

# In logs:
2024-01-15 14:32:01 INFO  finelor::skills::watcher - Skill file changed: invoice-generation/SKILL.md
2024-01-15 14:32:01 INFO  finelor::skills::registry - Reloaded skill: invoice-generation
```

#### Benefits

- **Developer Experience**: Test skill changes instantly
- **Production**: Deploy skill updates without downtime
- **Collaboration**: Team skill edits reflected immediately

---

### 6.2 Skill Versioning

**Current State**: `version` field in frontmatter is informational only  
**Desired State**: Full semantic versioning with upgrade paths

#### Implementation

```rust
// src/skills/version.rs

use semver::{Version, VersionReq};

#[derive(Debug, Clone)]
pub struct SkillVersion {
    pub current: Version,
    pub compatible_range: VersionReq,
    pub changelog: Option<String>,
}

impl Skill {
    /// Check if this skill version is compatible with finelor version
    pub fn check_compatibility(&self, finelor_version: &Version) -> Compatibility {
        // Allow skills to declare: "requires_finelor: ">=2.1.0"
        if let Some(ref req) = self.metadata.requires_finelor {
            if !req.matches(finelor_version) {
                return Compatibility::Incompatible {
                    reason: format!("Requires Finelor {}", req),
                };
            }
        }
        
        Compatibility::Compatible
    }
    
    /// Suggest upgrade if skill has newer version available
    pub fn check_upgrade_available(&self, registry: &SkillRegistry) -> Option<Upgrade> {
        if let Some(latest) = registry.get_latest_version(&self.metadata.name) {
            if latest.metadata.version > self.metadata.version {
                return Some(Upgrade {
                    from: self.metadata.version.clone(),
                    to: latest.metadata.version.clone(),
                    breaking_changes: self.detect_breaking_changes(&latest),
                });
            }
        }
        None
    }
}
```

#### Frontmatter Extension

```yaml
---
name: invoice-generation
description: "Use when generating customer invoices"
version: 2.1.0
requires_finelor: ">=2.0.0"
upgrades:
  from_version: "1.x"
  breaking_changes:
    - "Changed parameter 'client' to 'customer'"
    - "Removed support for legacy format v1"
---
```

#### Benefits

- **Breaking Changes**: Clear upgrade paths
- **Team Coordination**: Version-pinned skills in production
- **Rollback**: Revert to previous skill version if issues

---

### 6.3 Skill Dependencies

**Current State**: `related_skills` is informational only  
**Desired State**: Automatic dependency resolution and loading

#### Implementation

```rust
// src/skills/dependencies.rs

use petgraph::graph::Graph;

#[derive(Debug, Clone)]
pub struct SkillDependencyGraph {
    graph: Graph<SkillId, DependencyType>,
}

impl SkillDependencyGraph {
    /// Build dependency graph from all skills
    pub fn build(skills: &[Skill]) -> Result<Self, DependencyError> {
        let mut graph = Graph::new();
        let mut node_map: HashMap<String, NodeIndex> = HashMap::new();
        
        // Add all skills as nodes
        for skill in skills {
            let idx = graph.add_node(skill.id.clone());
            node_map.insert(skill.metadata.name.clone(), idx);
        }
        
        // Add edges for dependencies
        for skill in skills {
            if let Some(deps) = &skill.metadata.depends_on {
                for dep in deps {
                    if let Some(&dep_idx) = node_map.get(dep) {
                        let skill_idx = node_map[&skill.metadata.name];
                        graph.add_edge(skill_idx, dep_idx, DependencyType::Requires);
                    } else {
                        return Err(DependencyError::MissingDependency {
                            skill: skill.metadata.name.clone(),
                            dependency: dep.clone(),
                        });
                    }
                }
            }
        }
        
        // Check for cycles
        if petgraph::algo::is_cyclic_directed(&graph) {
            return Err(DependencyError::CircularDependency);
        }
        
        Ok(Self { graph })
    }
    
    /// Get load order (dependencies first)
    pub fn load_order(&self) -> Vec<SkillId> {
        petgraph::algo::toposort(&self.graph, None)
            .map(|idx| self.graph[idx].clone())
            .collect()
    }
}
```

#### Frontmatter Extension

```yaml
---
name: invoice-generation
description: "Use when generating customer invoices"
depends_on:
  - document-extraction    # Load document-extraction skill first
  - customer-lookup        # Then customer-lookup
related_skills:
  - payment-reconciliation  # Suggests but doesn't require
  - credit-note            # Suggests but doesn't require
---
```

#### Benefits

- **Modularity**: Compose skills from smaller, reusable parts
- **Consistency**: Shared "customer-lookup" skill used by multiple workflows
- **Maintainability**: Update shared dependency once, all skills benefit

---

### 6.4 Template Variable Substitution

**Current State**: Static skill content  
**Desired State**: Dynamic content with variables

#### Implementation

```rust
// src/skills/templating.rs

use tera::{Tera, Context};

pub struct SkillTemplating {
    engine: Tera,
}

impl SkillTemplating {
    /// Render skill content with context
    pub fn render(&self, content: &str, context: &SkillContext) -> Result<String, Error> {
        let mut ctx = Context::new();
        
        // Inject context variables
        ctx.insert("customer_name", &context.customer_name);
        ctx.insert("document_id", &context.document_id);
        ctx.insert("company_name", &context.company_name);
        ctx.insert("current_date", &context.current_date);
        ctx.insert("user_preferences", &context.user_prefs);
        
        // Render
        self.engine.render_str(content, &ctx)
    }
}

/// Context available when loading a skill
#[derive(Debug, Clone)]
pub struct SkillContext {
    pub session_id: String,
    pub customer_name: Option<String>,
    pub document_id: Option<String>,
    pub company_name: String,
    pub current_date: chrono::NaiveDate,
    pub user_prefs: HashMap<String, String>,
}
```

#### Skill Template Example

```markdown
---
name: invoice-generation
description: "Use when generating invoices for {{ customer_name }}"
---

# Invoice Generation

## Overview

This skill generates invoices for **{{ company_name }}**.
Current date: {{ current_date | date(format="%Y-%m-%d") }}.

## Templates

Use template from `{{ skill_dir }}/templates/invoice-{{ user_preferences.invoice_style }}.html`

## Workflow

### Step 1: Load Document {{ document_id }}

Call `get_document`:
```json
{
  "tool": "get_document",
  "params": {
    "document_ref": "{{ document_id }}"
  }
}
```
```

#### Benefits

- **Personalization**: Skill content adapts to company/customer
- **Consistency**: {{ company_name }} always correct
- **Flexibility**: Different invoice templates per user preference

---

### 6.5 External Skill Directories

**Current State**: Skills only in `~/.config/finelor/skills/`  
**Desired State**: Multiple skill sources

#### Implementation

```rust
// src/skills/external.rs

pub struct ExternalSkillManager {
    sources: Vec<ExternalSource>,
}

#[derive(Debug, Clone)]
pub struct ExternalSource {
    pub name: String,
    pub path: PathBuf,
    pub auto_update: bool,
    pub git_url: Option<String>,
    pub trust_level: TrustLevel,
}

impl ExternalSkillManager {
    /// Load skills from external directories
    pub async fn load_external(&self, registry: &SkillRegistry) -> Result<(), Error> {
        for source in &self.sources {
            // If git_url, sync first
            if let Some(ref url) = source.git_url {
                if source.auto_update {
                    self.git_sync(&source.path, url).await?;
                }
            }
            
            // Discover and load skills
            let skills = discover_skills(&source.path).await?;
            
            for skill in skills {
                // Tag with source
                let mut skill = skill;
                skill.metadata.source = Some(source.name.clone());
                skill.metadata.trust_level = source.trust_level;
                
                registry.register(skill).await?;
            }
        }
        
        Ok(())
    }
    
    /// Sync git repository
    async fn git_sync(&self, path: &Path, url: &str) -> Result<(), Error> {
        if !path.exists() {
            // Clone
            Command::new("git")
                .args(&["clone", url, path.to_str().unwrap()])
                .output()?;
        } else {
            // Pull
            Command::new("git")
                .current_dir(path)
                .args(&["pull", "origin", "main"])
                .output()?;
        }
        Ok(())
    }
}
```

#### Configuration

```yaml
# ~/.config/finelor/config.yaml

skills:
  builtin:
    enabled: true
    path: "/usr/share/finelor/skills"
  
  user:
    path: "~/.config/finelor/skills"
  
  external:
    - name: "acme-accounting-standards"
      path: "~/repos/acme-accounting-skills"
      git_url: "https://github.com/acme-corp/accounting-skills.git"
      auto_update: true
      trust_level: "trusted"
    
    - name: "industry-templates"
      path: "/shared/industry-skills"
      trust_level: "community"
      # No git_url = manual updates
```

#### Benefits

- **Team Sharing**: Shared skill repository for accounting team
- **Industry Standards**: Subscribe to industry-specific skill packs
- **Vendor Skills**: Software vendors provide skills for integrations

---

### 6.6 Usage Tracking

**Current State**: No visibility into which skills are used  
**Desired State**: Analytics on skill effectiveness

#### Implementation

```rust
// src/skills/usage.rs

use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillUsageEvent {
    pub skill_id: SkillId,
    pub event_type: UsageEventType,
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub context: Option<String>,  // e.g., document type that triggered
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UsageEventType {
    Listed,      // Skill appeared in skills_list
    Viewed,      // skill_view() called
    Applied,     // Tool calls made following skill workflow
    Completed,   // Skill workflow finished successfully
    Failed,      // Skill workflow encountered error
}

pub struct UsageTracker {
    storage: Arc<dyn UsageStorage>,
}

impl UsageTracker {
    pub async fn record(&self, event: SkillUsageEvent) {
        self.storage.store(event).await;
    }
    
    pub async fn get_skill_stats(&self, skill_id: &SkillId, days: i64) -> SkillStats {
        let events = self.storage.query(skill_id, days).await;
        
        SkillStats {
            total_views: events.iter().filter(|e| matches!(e.event_type, Viewed)).count(),
            total_applied: events.iter().filter(|e| matches!(e.event_type, Applied)).count(),
            success_rate: self.calculate_success_rate(&events),
            last_used: events.last().map(|e| e.timestamp),
        }
    }
}
```

#### Curator Integration

```rust
// Background curator process (Phase 7+)

impl Curator {
    async fn review_skills(&self) {
        let skills = self.registry.list().await;
        
        for skill in skills {
            let stats = self.usage_tracker.get_skill_stats(&skill.id, 90).await;
            
            // Unused for 90 days → Suggest archive
            if stats.last_used.is_none() || 
               stats.last_used.unwrap() < Utc::now() - Duration::days(90) {
                self.suggest_archive(&skill).await;
            }
            
            // High failure rate → Flag for review
            if stats.success_rate < 0.5 && stats.total_applied > 10 {
                self.flag_for_review(&skill, "High failure rate").await;
            }
        }
    }
}
```

#### Dashboard Metrics

```bash
$ finelor skills stats

Skill Usage (Last 30 Days):
┌─────────────────────────┬────────┬──────────┬────────┬────────┐
│ Skill                   │ Views  │ Applied  │ Success│ Status │
├─────────────────────────┼────────┼──────────┼────────┼────────┤
│ invoice-generation      │   342  │    298   │  97%   │ ✅     │
│ payment-reconciliation  │    89  │     76   │  94%   │ ✅     │
│ tax-calculation         │    45  │     32   │  72%   │ ⚠️     │
│ advanced-reporting    │    12  │      2   │  50%   │ 🔍     │
└─────────────────────────┴────────┴──────────┴────────┴────────┘

🔍 = Under review, ⚠️ = Needs attention, ✅ = Healthy
```

#### Benefits

- **Insight**: Know which skills are actually helpful
- **Maintenance**: Archive unused skills
- **Quality**: Identify skills with high failure rates
- **ROI**: Measure skill system effectiveness

---

## Implementation Priority

### Tier 1: Essential for Production (Weeks 7-8)

1. **Hot-Reload** - Immediate developer experience win
2. **Usage Tracking** - Critical for understanding adoption

### Tier 2: Team/Enterprise Features (Weeks 9-10)

3. **External Directories** - Teams sharing skills
4. **Template Variables** - Personalization

### Tier 3: Scale Features (Future)

5. **Skill Versioning** - Breaking changes management
6. **Dependencies** - Complex skill ecosystems

---

## Dependencies

### Phase 6 Requires (Already Implemented in Phases 1-5)

- `SkillRegistry` with hot-reload support
- `SkillTool` enum and execution
- System prompt integration
- File discovery and parsing

### New Dependencies for Phase 6

```toml
# Cargo.toml additions

[dependencies]
# Hot-reload
notify = "6.1"

# Versioning
semver = "1.0"

# Templating
tara = "1.20"  # or askama, handlebars

# Graph (dependencies)
petgraph = "0.6"

# Time tracking
chrono = "0.4"

# Git operations (optional)
git2 = "0.18"
```

---

## Testing Strategy

### Unit Tests

```rust
#[tokio::test]
async fn test_hot_reload_updates_skill() {
    let registry = Arc::new(SkillRegistry::new());
    let watcher = SkillFileWatcher::new(registry.clone()).unwrap();
    
    // Create skill file
    fs::write("test/TEMP_SKILL.md", "name: temp").await.unwrap();
    
    // Modify file
    fs::write("test/TEMP_SKILL.md", "name: temp\ndescription: updated").await.unwrap();
    
    // Wait for reload
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Verify updated
    let skill = registry.get("temp").await.unwrap();
    assert_eq!(skill.metadata.description, "updated");
}

#[test]
fn test_dependency_graph_resolution() {
    let skills = vec![
        skill_with_deps("a", &["b", "c"]),
        skill_with_deps("b", &["c"]),
        skill_with_deps("c", &[]),
    ];
    
    let graph = SkillDependencyGraph::build(&skills).unwrap();
    let order = graph.load_order();
    
    // c must come before b, b before a
    assert_eq!(order, vec!["c", "b", "a"]);
}

#[test]
fn test_circular_dependency_detection() {
    let skills = vec![
        skill_with_deps("a", &["b"]),
        skill_with_deps("b", &["a"]),
    ];
    
    let result = SkillDependencyGraph::build(&skills);
    assert!(matches!(result, Err(DependencyError::CircularDependency)));
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_external_skill_loading() {
    // Set up external directory
    let external = tempdir().unwrap();
    create_skill_file(&external, "external-skill", "External skill").await;
    
    // Configure and load
    let config = Config {
        skills: SkillConfig {
            external_dirs: vec![external.path().to_path_buf()],
            ..Default::default()
        },
    };
    
    let manager = ExternalSkillManager::from_config(&config);
    let registry = SkillRegistry::new();
    manager.load_external(&registry).await.unwrap();
    
    // Verify loaded
    let skill = registry.get("external-skill").await.unwrap();
    assert_eq!(skill.metadata.source, Some("external".to_string()));
}
```

---

## Migration Path from Phase 5

### Step 1: Enable Hot-Reload (Day 1)

```bash
# Add to config
skills:
  hot_reload: true

# Restart Finelor
# All skill edits now auto-reload
```

### Step 2: Add Usage Tracking (Week 1)

```rust
// Add to registry
registry.enable_usage_tracking(UsageTracker::new());

// Usage data appears in admin dashboard
```

### Step 3: Team External Directories (Week 2)

```bash
# Accounting team shares skills via git
mkdir /shared/acme-skills
git clone https://github.com/acme/accounting-skills /shared/acme-skills

# Config update
skills:
  external:
    - name: "acme-standards"
      path: "/shared/acme-skills"
```

### Step 4: Template Variables (Week 3)

```rust
// Skills start using {{ variables }}
// No code changes needed - just content updates
```

### Step 5: Versioning (Week 4+)

```yaml
# Skills add version fields
# Breaking changes require version bumps
```

---

## Success Metrics

| Metric | Phase 5 (Baseline) | Phase 6 Target |
|--------|-------------------|----------------|
| Skill change deployment time | Minutes (restart) | Seconds (hot-reload) |
| Unused skill cleanup | Manual | Automated (curator) |
| Team skill sharing | None | Git-based sync |
| Personalization | None | Template variables |
| Breaking change awareness | None | Version warnings |

---

## Blockers and Risks

| Risk | Mitigation |
|------|-----------|
| File watcher performance on large directories | Configurable debounce, exclude patterns |
| Git sync conflicts | Manual merge required, conflict markers in skill |
| Template injection | Same security scanning as skill content |
| Dependency cycles | Detect and fail fast at load time |
| Usage tracking overhead | Async batching, configurable retention |

---

## Open Questions

1. **External Skills**: Should untrusted external skills run in sandbox?
2. **Template Language**: Tera vs Handlebars vs custom syntax?
3. **Version Resolution**: Latest compatible vs explicit versions?
4. **Usage Retention**: How long to keep usage data? (GDPR compliance)
5. **Skill Marketplace**: Future integration with skill registry service?

---

This proposal is **ready for review** once Phases 1-5 are complete and stable in production.
