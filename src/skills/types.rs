use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Unique identifier for a skill
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillId(pub String);

impl SkillId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for SkillId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Skill metadata from YAML frontmatter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Unique name for the skill
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Category for grouping skills
    pub category: String,
    /// Optional version string
    pub version: Option<String>,
    /// Optional author/creator
    pub author: Option<String>,
    /// Optional list of keywords for search
    pub keywords: Option<Vec<String>>,
    /// Optional dependencies on other skills
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
    /// Optional list of file extensions this skill applies to
    pub file_extensions: Option<Vec<String>>,
    /// Optional list of globs matching files this skill applies to
    pub file_globs: Option<Vec<String>>,
    /// Additional arbitrary metadata as key-value pairs
    #[serde(flatten)]
    pub extra: HashMap<String, String>,
}

/// Represents a reference file (markdown document)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillReference {
    /// Reference name/filename (e.g., "browser-based-agent-pattern.md")
    pub name: String,
    /// Full path to the reference file
    pub path: PathBuf,
    /// Content of the reference file
    pub content: String,
}

/// Represents a template file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillTemplate {
    /// Template name/filename
    pub name: String,
    /// Full path to the template file
    pub path: PathBuf,
    /// Template content (may contain placeholders)
    pub content: String,
}

/// Complete skill data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Unique identifier derived from skill directory name
    pub id: SkillId,
    /// Metadata from YAML frontmatter
    pub metadata: SkillMetadata,
    /// Full content of SKILL.md (excluding frontmatter)
    pub content: String,
    /// Path to the skill directory
    pub path: PathBuf,
    /// Reference files in the references/ subdirectory
    pub references: Vec<SkillReference>,
    /// Template files in the templates/ subdirectory
    pub templates: Vec<SkillTemplate>,
    /// When the skill was loaded
    pub loaded_at: chrono::DateTime<chrono::Utc>,
    /// Last modified time of the skill directory
    pub modified_at: Option<std::time::SystemTime>,
}

impl Skill {
    /// Returns the skill name from metadata
    pub fn name(&self) -> &str {
        &self.metadata.name
    }

    /// Returns the skill description
    pub fn description(&self) -> &str {
        &self.metadata.description
    }

    /// Returns the skill category
    pub fn category(&self) -> &str {
        &self.metadata.category
    }

    /// Get all keywords as a slice
    pub fn keywords(&self) -> &[String] {
        self.metadata.keywords.as_deref().unwrap_or_default()
    }

    /// Check if skill has a specific reference file
    pub fn get_reference(&self, name: &str) -> Option<&SkillReference> {
        self.references.iter().find(|r| r.name == name)
    }

    /// Get a specific template by name
    pub fn get_template(&self, name: &str) -> Option<&SkillTemplate> {
        self.templates.iter().find(|t| t.name == name)
    }

    /// Returns true if skill applies to given file extension
    pub fn applies_to_extension(&self, ext: &str) -> bool {
        self.metadata
            .file_extensions
            .as_ref()
            .map(|exts| exts.iter().any(|e| e.eq_ignore_ascii_case(ext)))
            .unwrap_or(false)
    }

    /// Returns true if skill applies to given file path (based on globs)
    pub fn applies_to_file(&self, file_path: &std::path::Path) -> bool {
        self.metadata
            .file_globs
            .as_ref()
            .map(|globs| {
                let filename = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                globs.iter().any(|pattern| {
                    // Simple glob matching - can be enhanced with glob crate
                    filename.contains(pattern.trim_matches('*'))
                })
            })
            .unwrap_or(false)
    }
}

/// Filter criteria for skill queries
#[derive(Debug, Clone, Default)]
pub struct SkillFilter {
    /// Filter by category
    pub category: Option<String>,
    /// Filter by keyword (any skill containing this keyword)
    pub keyword: Option<String>,
    /// Filter by file extension applicability
    pub file_extension: Option<String>,
    /// Filter by name pattern (substring match)
    pub name_pattern: Option<String>,
}

impl SkillFilter {
    /// Create a new filter for a specific category
    pub fn by_category(category: impl Into<String>) -> Self {
        Self {
            category: Some(category.into()),
            ..Default::default()
        }
    }

    /// Create a new filter for skills matching a keyword
    pub fn by_keyword(keyword: impl Into<String>) -> Self {
        Self {
            keyword: Some(keyword.into()),
            ..Default::default()
        }
    }

    /// Create a new filter for skills applicable to a file extension
    pub fn by_extension(ext: impl Into<String>) -> Self {
        Self {
            file_extension: Some(ext.into()),
            ..Default::default()
        }
    }

    /// Check if a skill matches this filter
    pub fn matches(&self, skill: &Skill) -> bool {
        if let Some(cat) = &self.category
            && !skill.category().eq_ignore_ascii_case(cat)
        {
            return false;
        }

        if let Some(keyword) = &self.keyword {
            let keyword_lower = keyword.to_lowercase();
            if !skill
                .metadata
                .keywords
                .as_ref()
                .is_some_and(|kw| kw.iter().any(|k| k.to_lowercase().contains(&keyword_lower)))
                && !skill.name().to_lowercase().contains(&keyword_lower)
                && !skill.description().to_lowercase().contains(&keyword_lower)
            {
                return false;
            }
        }

        if let Some(ext) = &self.file_extension
            && !skill.applies_to_extension(ext)
        {
            return false;
        }

        if let Some(pattern) = &self.name_pattern
            && !skill
                .name()
                .to_lowercase()
                .contains(&pattern.to_lowercase())
        {
            return false;
        }

        true
    }
}

/// Error types for skill operations
#[derive(Debug, Clone, thiserror::Error)]
pub enum SkillError {
    #[error("skill not found: {0}")]
    NotFound(String),
    #[error(
        "duplicate skill display name '{name}' for skill ids '{first_id}' and '{second_id}' at '{first_path}' and '{second_path}'"
    )]
    DuplicateSkillName {
        name: String,
        first_id: String,
        second_id: String,
        first_path: String,
        second_path: String,
    },
    #[error("invalid YAML frontmatter: {0}")]
    InvalidFrontmatter(String),
    #[error("invalid markdown content: {0}")]
    InvalidContent(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("parse error: {0}")]
    ParseError(String),
    #[error("reference not found: {0}")]
    ReferenceNotFound(String),
    #[error("template not found: {0}")]
    TemplateNotFound(String),
}

pub type SkillResult<T> = Result<T, SkillError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_skill(name: &str, category: &str) -> Skill {
        Skill {
            id: SkillId::new(name),
            metadata: SkillMetadata {
                name: name.to_string(),
                description: format!("Description for {}", name),
                category: category.to_string(),
                version: None,
                author: None,
                keywords: Some(vec!["test".to_string(), "example".to_string()]),
                depends_on: None,
                file_extensions: Some(vec!["rs".to_string(), "md".to_string()]),
                file_globs: None,
                extra: HashMap::new(),
            },
            content: String::new(),
            path: PathBuf::from("/test"),
            references: vec![],
            templates: vec![],
            loaded_at: chrono::Utc::now(),
            modified_at: None,
        }
    }

    #[test]
    fn skill_filter_by_category() {
        let skill = create_test_skill("test-skill", "software-development");
        let filter = SkillFilter::by_category("software-development");
        assert!(filter.matches(&skill));

        let filter = SkillFilter::by_category("devops");
        assert!(!filter.matches(&skill));
    }

    #[test]
    fn skill_filter_by_extension() {
        let skill = create_test_skill("test-skill", "dev");
        let filter = SkillFilter::by_extension("rs");
        assert!(filter.matches(&skill));

        let filter = SkillFilter::by_extension("py");
        assert!(!filter.matches(&skill));
    }

    #[test]
    fn skill_filter_by_keyword() {
        let skill = create_test_skill("test-skill", "dev");
        let filter = SkillFilter::by_keyword("test");
        assert!(filter.matches(&skill));

        let filter = SkillFilter::by_keyword("foo");
        assert!(!filter.matches(&skill));
    }
}
