use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::skills::types::{
    Skill, SkillError, SkillId, SkillMetadata, SkillReference, SkillResult, SkillSections,
    SkillTemplate,
};

/// Default path for skills directory
pub const SKILLS_DIR: &str = "./assets/skills";

/// Loader for skill files from disk
#[derive(Debug)]
pub struct SkillLoader {
    /// Root directory for loading skills
    skills_dir: PathBuf,
}

impl SkillLoader {
    /// Create a new skill loader with the default skills directory
    pub fn new() -> Self {
        Self {
            skills_dir: shellexpand::tilde(SKILLS_DIR).into_owned().into(),
        }
    }

    /// Create a skill loader with a custom skills directory
    pub fn with_dir(path: impl Into<PathBuf>) -> Self {
        Self {
            skills_dir: path.into(),
        }
    }

    /// Get the skills directory path
    pub fn skills_dir(&self) -> &Path {
        &self.skills_dir
    }

    /// Load all skills from the skills directory
    pub async fn load_all(&self) -> SkillResult<Vec<Skill>> {
        let mut skills = Vec::new();

        if !self.skills_dir.exists() {
            tracing::info!(
                "Skills directory does not exist, creating: {:?}",
                self.skills_dir
            );
            fs::create_dir_all(&self.skills_dir).map_err(|e| SkillError::Io(e.to_string()))?;
            return Ok(skills);
        }

        let entries = fs::read_dir(&self.skills_dir).map_err(|e| SkillError::Io(e.to_string()))?;

        for entry in entries {
            let entry = entry.map_err(|e| SkillError::Io(e.to_string()))?;
            let path = entry.path();

            if path.is_dir() {
                let skill_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| SkillError::ParseError("Invalid directory name".to_string()))?;

                match self.load_skill(skill_name).await {
                    Ok(skill) => {
                        tracing::info!("Loaded skill: {} from {:?}", skill_name, path);
                        skills.push(skill);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load skill {}: {:?}", skill_name, e);
                        // Continue loading other skills
                    }
                }
            }
        }

        Ok(skills)
    }

    /// Load a specific skill by name
    pub async fn load_skill(&self, skill_name: &str) -> SkillResult<Skill> {
        let skill_dir = self.skills_dir.join(skill_name);
        if !skill_dir.exists() {
            return Err(SkillError::NotFound(skill_name.to_string()));
        }

        let skill_md_path = skill_dir.join("SKILL.md");
        if !skill_md_path.exists() {
            return Err(SkillError::NotFound(format!(
                "SKILL.md not found for skill: {}",
                skill_name
            )));
        }

        // Read and parse SKILL.md
        let content =
            fs::read_to_string(&skill_md_path).map_err(|e| SkillError::Io(e.to_string()))?;

        let (metadata_str, markdown_content) = Self::split_frontmatter(&content)?;

        // Parse YAML frontmatter
        let metadata = Self::parse_frontmatter(metadata_str)?;

        // Parse markdown sections
        let sections = Self::parse_sections(markdown_content);

        // Load references
        let references = self.load_references(&skill_dir).await?;

        // Load templates
        let templates = self.load_templates(&skill_dir).await?;

        // Get modification time
        let modified_at = fs::metadata(&skill_dir)
            .ok()
            .and_then(|m| m.modified().ok());

        Ok(Skill {
            id: SkillId::new(skill_name),
            metadata,
            sections,
            raw_content: markdown_content.to_string(),
            path: skill_dir,
            references,
            templates,
            loaded_at: chrono::Utc::now(),
            modified_at,
        })
    }

    /// Split content into YAML frontmatter and markdown body
    /// Frontmatter is delimited by --- at the start and end
    fn split_frontmatter(content: &str) -> SkillResult<(&str, &str)> {
        let trimmed = content.trim_start();

        if !trimmed.starts_with("---") {
            // No frontmatter - return empty frontmatter and full content as markdown
            return Ok(("", trimmed));
        }

        // Find the end delimiter (--- after the start)
        let after_start = &trimmed[3..]; // Skip first ---
        if let Some(end_pos) = after_start.find("---") {
            let frontmatter = after_start[..end_pos].trim();
            let markdown = after_start[end_pos + 3..].trim_start();
            Ok((frontmatter, markdown))
        } else {
            Err(SkillError::InvalidFrontmatter(
                "Missing closing --- for YAML frontmatter".to_string(),
            ))
        }
    }

    /// Parse YAML frontmatter into SkillMetadata
    fn parse_frontmatter(yaml_str: &str) -> SkillResult<SkillMetadata> {
        if yaml_str.is_empty() {
            return Err(SkillError::InvalidFrontmatter(
                "Empty YAML frontmatter".to_string(),
            ));
        }

        let yaml = yaml_rust2::YamlLoader::load_from_str(yaml_str)
            .map_err(|e| SkillError::InvalidFrontmatter(e.to_string()))?;

        if yaml.is_empty() || yaml[0].is_badvalue() {
            return Err(SkillError::InvalidFrontmatter(
                "Failed to parse YAML frontmatter".to_string(),
            ));
        }

        let doc = &yaml[0];

        // Extract required fields
        let name = doc["name"]
            .as_str()
            .ok_or_else(|| {
                SkillError::InvalidFrontmatter("Missing required field: name".to_string())
            })?
            .to_string();

        let description = doc["description"]
            .as_str()
            .ok_or_else(|| {
                SkillError::InvalidFrontmatter("Missing required field: description".to_string())
            })?
            .to_string();

        let category = doc["category"]
            .as_str()
            .ok_or_else(|| {
                SkillError::InvalidFrontmatter("Missing required field: category".to_string())
            })?
            .to_string();

        // Extract optional fields
        let version = doc["version"].as_str().map(|s| s.to_string());
        let author = doc["author"].as_str().map(|s| s.to_string());

        let keywords = doc["keywords"].as_vec().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });

        let depends_on = doc["depends_on"].as_vec().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });

        let file_extensions = doc["file_extensions"].as_vec().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });

        let file_globs = doc["file_globs"].as_vec().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });

        // Extract extra fields
        let mut extra = HashMap::new();
        if let Some(hash) = doc.as_hash() {
            for (key, value) in hash {
                let key_str = key.as_str().unwrap_or_default();
                // Skip already extracted fields
                let known_fields = [
                    "name",
                    "description",
                    "category",
                    "version",
                    "author",
                    "keywords",
                    "depends_on",
                    "file_extensions",
                    "file_globs",
                ];
                if !known_fields.contains(&key_str) {
                    // Convert yaml_rust2::Yaml to serde_yaml::Value
                    // First convert to string representation, then parse
                    let yaml_str = format!("{:?}", value);
                    if let Ok(yaml_val) = serde_yaml::from_str(&yaml_str) {
                        extra.insert(key_str.to_string(), yaml_val);
                    }
                }
            }
        }

        Ok(SkillMetadata {
            name,
            description,
            category,
            version,
            author,
            keywords,
            depends_on,
            file_extensions,
            file_globs,
            extra,
        })
    }

    /// Parse markdown content into sections
    fn parse_sections(markdown: &str) -> SkillSections {
        let mut sections = SkillSections::default();
        let mut extra: HashMap<String, String> = HashMap::new();

        let mut current_section: Option<String> = None;
        let mut current_content = String::new();

        for line in markdown.lines() {
            // Check for heading
            if line.starts_with("## ") {
                // Save previous section
                if let Some(section_name) = current_section {
                    let content = current_content.trim().to_string();
                    Self::store_section(&mut sections, &mut extra, &section_name, &content);
                }

                // Start new section
                current_section = Some(line[3..].trim().to_string());
                current_content.clear();
            } else if line.starts_with("# ") {
                // Main title - skip
                continue;
            } else {
                // Add to current section content
                current_content.push_str(line);
                current_content.push('\n');
            }
        }

        // Save last section
        if let Some(section_name) = current_section {
            let content = current_content.trim().to_string();
            Self::store_section(&mut sections, &mut extra, &section_name, &content);
        }

        sections.extra = extra;
        sections
    }

    /// Store section content in the appropriate field
    fn store_section(
        sections: &mut SkillSections,
        extra: &mut HashMap<String, String>,
        name: &str,
        content: &str,
    ) {
        let name_lower = name.to_lowercase();

        match name_lower.as_str() {
            "overview" | "introduction" | "about" => {
                sections.overview = content.to_string();
            }
            "when to use" | "when_to_use" | "when-to-use" | "usage" => {
                sections.when_to_use = content.to_string();
            }
            "when not to use" | "when_not_to_use" | "when-not-to-use" | "limitations"
            | "caveats" => {
                sections.when_not_to_use = content.to_string();
            }
            "workflow" | "steps" | "process" | "how to" => {
                sections.workflow = content.to_string();
            }
            "examples" | "example" | "sample" | "demos" => {
                sections.examples = content.to_string();
            }
            "references" | "reference" | "resources" | "links" => {
                sections.references = content.to_string();
            }
            _ => {
                extra.insert(name.to_string(), content.to_string());
            }
        }
    }

    /// Load reference files from the references/ subdirectory
    async fn load_references(&self, skill_dir: &Path) -> SkillResult<Vec<SkillReference>> {
        let references_dir = skill_dir.join("references");
        let mut references = Vec::new();

        if !references_dir.exists() {
            return Ok(references);
        }

        let entries = fs::read_dir(&references_dir).map_err(|e| SkillError::Io(e.to_string()))?;

        for entry in entries {
            let entry = entry.map_err(|e| SkillError::Io(e.to_string()))?;
            let path = entry.path();

            if path.is_file() {
                let filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| SkillError::ParseError("Invalid filename".to_string()))?
                    .to_string();

                // Only load markdown files
                if filename.ends_with(".md") {
                    let content =
                        fs::read_to_string(&path).map_err(|e| SkillError::Io(e.to_string()))?;

                    references.push(SkillReference {
                        filename,
                        path,
                        content,
                    });
                }
            }
        }

        Ok(references)
    }

    /// Load template files from the templates/ subdirectory
    async fn load_templates(&self, skill_dir: &Path) -> SkillResult<Vec<SkillTemplate>> {
        let templates_dir = skill_dir.join("templates");
        let mut templates = Vec::new();

        if !templates_dir.exists() {
            return Ok(templates);
        }

        let entries = fs::read_dir(&templates_dir).map_err(|e| SkillError::Io(e.to_string()))?;

        for entry in entries {
            let entry = entry.map_err(|e| SkillError::Io(e.to_string()))?;
            let path = entry.path();

            if path.is_file() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| SkillError::ParseError("Invalid filename".to_string()))?
                    .to_string();

                let content =
                    fs::read_to_string(&path).map_err(|e| SkillError::Io(e.to_string()))?;

                // Extract description from first comment or frontmatter
                let description = Self::extract_template_description(&content);

                templates.push(SkillTemplate {
                    name,
                    path,
                    content,
                    description,
                });
            }
        }

        Ok(templates)
    }

    /// Extract description from template file
    fn extract_template_description(content: &str) -> Option<String> {
        // Look for description in HTML comment <!-- description: ... -->
        if let Some(start) = content.find("<!--") {
            if let Some(end) = content.find("-->") {
                let comment = &content[start + 4..end];
                if let Some(desc_start) = comment.find("description:") {
                    return Some(comment[desc_start + 12..].trim().to_string());
                }
            }
        }

        // Look for first line comment
        for line in content.lines().take(5) {
            let trimmed = line.trim();
            if trimmed.starts_with("# ") || trimmed.starts_with("// ") {
                return Some(trimmed[3..].to_string());
            }
        }

        None
    }
}

impl Default for SkillLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_frontmatter() {
        let content = r#"---
name: test
---
# Content

Hello world.
"#;

        let (frontmatter, markdown) = SkillLoader::split_frontmatter(content).unwrap();
        assert!(!frontmatter.is_empty());
        assert!(markdown.contains("Hello world"));
    }

    #[test]
    fn test_split_frontmatter_no_frontmatter() {
        let content = "# Just markdown\n\nNo frontmatter here.";

        let (frontmatter, markdown) = SkillLoader::split_frontmatter(content).unwrap();
        assert!(frontmatter.is_empty());
        assert_eq!(markdown, content.trim_start());
    }

    #[test]
    fn test_parse_sections() {
        let markdown = r#"# Title

## Overview

This is overview.

## When to Use

Use this when...

## Custom Section

Custom content."#;

        let sections = SkillLoader::parse_sections(markdown);

        assert_eq!(sections.overview, "This is overview.");
        assert_eq!(sections.when_to_use, "Use this when...");
        assert!(sections.extra.contains_key("Custom Section"));
        assert_eq!(
            sections.extra.get("Custom Section").unwrap(),
            "Custom content."
        );
    }

    #[test]
    fn test_store_section_variations() {
        let mut sections = SkillSections::default();
        let mut extra = HashMap::new();

        SkillLoader::store_section(&mut sections, &mut extra, "When to Use", "content1");
        SkillLoader::store_section(&mut sections, &mut extra, "when-not-to-use", "content2");
        SkillLoader::store_section(&mut sections, &mut extra, "Unknown Section", "content3");

        assert_eq!(sections.when_to_use, "content1");
        assert_eq!(sections.when_not_to_use, "content2");
        assert_eq!(extra.get("Unknown Section").unwrap(), "content3");
    }
}
