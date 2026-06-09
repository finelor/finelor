//! Skill system for Finelor
//!
//! This module provides skill loading and management for the Finelor assistant agent.
//! Skills are loaded from `assets/skills/` and follow the Hermes skill format.
//!
//! ## Directory Structure
//!
//! ```text
//! assets/skills/
//! └── <skill-name>/
//!     ├── SKILL.md          # Main skill file with YAML frontmatter
//!     ├── references/       # Optional reference documents
//!     │   └── *.md
//!     └── templates/        # Optional templates
//!         └── *.md
//! ```
//!
//! ## SKILL.md Format
//!
//! ```markdown
//! ---
//! name: skill-name
//! description: Description of what this skill does
//! category: software-development
//! keywords:
//!   - rust
//!   - example
//! ---
//!
//! # Skill Title
//!
//! ## Overview
//!
//! Description of the skill...
//!
//! ## Workflow
//!
//! 1. Step one
//! 2. Step two
//! ```

mod loader;
pub mod registry;
pub mod types;

// Re-export main types
pub use registry::{
    SharedSkillRegistry, SkillRegistry, create_skill_registry, create_skill_registry_with_path,
};
pub use types::{
    Skill, SkillError, SkillFilter, SkillId, SkillMetadata, SkillReference, SkillResult,
    SkillTemplate,
};

/// Default skills directory path
pub const SKILLS_DIR: &str = "./assets/skills";

/// Initialize a skill registry with the default path
pub async fn init_registry() -> types::SkillResult<registry::SharedSkillRegistry> {
    registry::create_skill_registry().await
}
