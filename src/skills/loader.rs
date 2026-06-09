use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use yaml_rust2::{Yaml, YamlEmitter};

use crate::skills::types::{
    Skill, SkillError, SkillId, SkillMetadata, SkillReference, SkillResult, SkillTemplate,
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

        tracing::info!(
            skills_dir = %self.skills_dir.display(),
            "Scanning skills directory for skill packages"
        );

        if !self.skills_dir.exists() {
            tracing::info!(
                skills_dir = %self.skills_dir.display(),
                "Skills directory does not exist; creating it"
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
                        tracing::info!(
                            skill_id = %skill.id,
                            skill_name = skill.name(),
                            category = skill.category(),
                            reference_count = skill.references.len(),
                            template_count = skill.templates.len(),
                            skill_path = %path.display(),
                            "Loaded skill package from disk"
                        );
                        tracing::debug!(
                            skill_id = %skill.id,
                            skill_name = skill.name(),
                            reference_names = ?skill
                                .references
                                .iter()
                                .map(|reference| reference.name.clone())
                                .collect::<Vec<_>>(),
                            template_names = ?skill
                                .templates
                                .iter()
                                .map(|template| template.name.clone())
                                .collect::<Vec<_>>(),
                            "Loaded supporting files for skill package"
                        );
                        skills.push(skill);
                    }
                    Err(e) => {
                        tracing::warn!(
                            skill_name,
                            skill_path = %path.display(),
                            error = %e,
                            "Failed to load skill package from disk"
                        );
                        // Continue loading other skills
                    }
                }
            }
        }

        tracing::info!(
            skills_dir = %self.skills_dir.display(),
            loaded_skill_count = skills.len(),
            "Finished scanning skills directory"
        );

        Ok(skills)
    }

    /// Load a specific skill by name
    pub async fn load_skill(&self, skill_name: &str) -> SkillResult<Skill> {
        let skill_dir = self.skills_dir.join(skill_name);
        tracing::debug!(
            skill_name,
            skill_dir = %skill_dir.display(),
            "Loading skill package from disk"
        );
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

        // Load references
        let references = self.load_references(&skill_dir).await?;

        // Load templates
        let templates = self.load_templates(&skill_dir).await?;

        // Get modification time
        let modified_at = fs::metadata(&skill_dir)
            .ok()
            .and_then(|m| m.modified().ok());

        let skill = Skill {
            id: SkillId::new(skill_name),
            metadata,
            content: markdown_content.to_string(),
            path: skill_dir,
            references,
            templates,
            loaded_at: chrono::Utc::now(),
            modified_at,
        };

        tracing::debug!(
            skill_id = %skill.id,
            skill_name = skill.name(),
            reference_count = skill.references.len(),
            template_count = skill.templates.len(),
            "Finished loading skill package from disk"
        );

        Ok(skill)
    }

    /// Split content into YAML frontmatter and markdown body
    /// Frontmatter is delimited by --- at the start and end
    fn split_frontmatter(content: &str) -> SkillResult<(&str, &str)> {
        let trimmed = content.trim_start();

        if !trimmed.starts_with("---") {
            // No frontmatter - return empty frontmatter and full content as markdown
            return Ok(("", trimmed));
        }

        let mut lines = trimmed.split_inclusive('\n');
        let opening_line = lines.next().unwrap_or(trimmed);
        if opening_line.trim_end_matches(['\r', '\n']).trim() != "---" {
            return Ok(("", trimmed));
        }

        let frontmatter_start = opening_line.len();
        let mut cursor = frontmatter_start;

        for line in lines {
            let line_len = line.len();
            let normalized = line.trim_end_matches(['\r', '\n']).trim();
            if normalized == "---" {
                let frontmatter = trimmed[frontmatter_start..cursor].trim();
                let markdown = trimmed[cursor + line_len..].trim_start();
                return Ok((frontmatter, markdown));
            }
            cursor += line_len;
        }

        Err(SkillError::InvalidFrontmatter(
            "Missing closing --- for YAML frontmatter".to_string(),
        ))
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
                    let yaml_str = Self::emit_yaml_value(value).map_err(|e| {
                        SkillError::InvalidFrontmatter(format!(
                            "Failed to preserve extra field `{key_str}` as raw YAML: {e}"
                        ))
                    })?;
                    extra.insert(key_str.to_string(), yaml_str);
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

    fn emit_yaml_value(value: &Yaml) -> SkillResult<String> {
        let mut output = String::new();
        YamlEmitter::new(&mut output)
            .dump(value)
            .map_err(|e| SkillError::InvalidFrontmatter(e.to_string()))?;

        Ok(output
            .strip_prefix("---\n")
            .or_else(|| output.strip_prefix("---\r\n"))
            .unwrap_or(&output)
            .trim_end_matches(['\r', '\n'])
            .to_string())
    }

    /// Load reference files from the references/ subdirectory
    async fn load_references(&self, skill_dir: &Path) -> SkillResult<Vec<SkillReference>> {
        let references_dir = skill_dir.join("references");
        let mut references = Vec::new();

        if !references_dir.exists() {
            tracing::debug!(
                skill_dir = %skill_dir.display(),
                references_dir = %references_dir.display(),
                "Skill package has no references directory"
            );
            return Ok(references);
        }

        let entries = fs::read_dir(&references_dir).map_err(|e| SkillError::Io(e.to_string()))?;

        for entry in entries {
            let entry = entry.map_err(|e| SkillError::Io(e.to_string()))?;
            let path = entry.path();

            if path.is_file() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| SkillError::ParseError("Invalid filename".to_string()))?
                    .to_string();

                // Only load markdown files
                if name.ends_with(".md") {
                    let content =
                        fs::read_to_string(&path).map_err(|e| SkillError::Io(e.to_string()))?;

                    tracing::debug!(
                        skill_dir = %skill_dir.display(),
                        reference_name = name,
                        reference_path = %path.display(),
                        content_length = content.len(),
                        "Loaded reference file from skill package"
                    );
                    references.push(SkillReference {
                        name,
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
            tracing::debug!(
                skill_dir = %skill_dir.display(),
                templates_dir = %templates_dir.display(),
                "Skill package has no templates directory"
            );
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

                tracing::debug!(
                    skill_dir = %skill_dir.display(),
                    template_name = name,
                    template_path = %path.display(),
                    content_length = content.len(),
                    "Loaded template file from skill package"
                );
                templates.push(SkillTemplate {
                    name,
                    path,
                    content,
                });
            }
        }

        Ok(templates)
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
    use std::fs;
    use tempfile::TempDir;

    fn write_skill_fixture(
        root: &std::path::Path,
        skill_dir_name: &str,
        skill_md: &str,
        references: &[(&str, &str)],
        templates: &[(&str, &str)],
    ) {
        let skill_dir = root.join(skill_dir_name);
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(skill_dir.join("SKILL.md"), skill_md).expect("write SKILL.md");

        if !references.is_empty() {
            let references_dir = skill_dir.join("references");
            fs::create_dir_all(&references_dir).expect("create references dir");
            for (name, content) in references {
                fs::write(references_dir.join(name), content).expect("write reference");
            }
        }

        if !templates.is_empty() {
            let templates_dir = skill_dir.join("templates");
            fs::create_dir_all(&templates_dir).expect("create templates dir");
            for (name, content) in templates {
                fs::write(templates_dir.join(name), content).expect("write template");
            }
        }
    }

    fn create_temp_skill_root() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

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
    fn test_split_frontmatter_ignores_embedded_dashes_inside_yaml_values() {
        let content = r#"---
name: test
description: "uses --- inside metadata"
category: accounting
---
# Content

Hello world.
"#;

        let (frontmatter, markdown) = SkillLoader::split_frontmatter(content).unwrap();
        assert!(frontmatter.contains(r#"description: "uses --- inside metadata""#));
        assert!(markdown.contains("Hello world."));
    }

    #[test]
    fn test_split_frontmatter_requires_closing_delimiter_line() {
        let content = r#"---
name: test
description: "uses --- inside metadata""#;

        let err = SkillLoader::split_frontmatter(content).expect_err("missing closing delimiter");
        assert!(matches!(err, SkillError::InvalidFrontmatter(_)));
        assert!(err.to_string().contains("Missing closing ---"));
    }

    #[test]
    fn parse_frontmatter_preserves_unknown_extra_fields_as_raw_yaml() {
        let metadata = SkillLoader::parse_frontmatter(
            r#"name: test
description: test description
category: accounting
custom_scalar: plain-text
custom_sequence:
  - one
  - two
custom_mapping:
  nested: value
  enabled: true
"#,
        )
        .expect("frontmatter should parse");

        assert_eq!(
            metadata.extra.get("custom_scalar").map(String::as_str),
            Some("plain-text")
        );
        assert_eq!(
            metadata.extra.get("custom_sequence").map(String::as_str),
            Some("- one\n- two")
        );
        assert_eq!(
            metadata.extra.get("custom_mapping").map(String::as_str),
            Some("nested: value\nenabled: true")
        );
    }

    #[tokio::test]
    async fn load_skill_reads_complete_skill_with_references_and_templates() {
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "invoice-helper",
            r#"---
name: Invoice Helper
description: Helps prepare invoices
category: accounting
version: "1.0.0"
author: Finelor
keywords:
  - invoices
  - billing
depends_on:
  - core-skill
file_extensions:
  - pdf
file_globs:
  - "*.pdf"
custom_scalar: plain-text
custom_sequence:
  - review
  - export
custom_mapping:
  region: eu
  priority: high
---
# Invoice Helper

## Overview

Create invoices carefully.

## Workflow

1. Ask questions
2. Confirm the result

## Examples

Example content.
"#,
            &[("usage.md", "# Usage\n\nUse responsibly.")],
            &[("invoice.html", "<html></html>")],
        );

        let loader = SkillLoader::with_dir(root.path());
        let skill = loader.load_skill("invoice-helper").await.expect("skill");

        assert_eq!(skill.id.0, "invoice-helper");
        assert_eq!(skill.metadata.name, "Invoice Helper");
        assert_eq!(skill.metadata.description, "Helps prepare invoices");
        assert_eq!(skill.metadata.category, "accounting");
        assert_eq!(skill.metadata.version.as_deref(), Some("1.0.0"));
        assert_eq!(skill.metadata.author.as_deref(), Some("Finelor"));
        assert_eq!(
            skill.metadata.keywords.as_ref().expect("keywords"),
            &vec!["invoices".to_string(), "billing".to_string()]
        );
        assert_eq!(
            skill.metadata.depends_on.as_ref().expect("depends_on"),
            &vec!["core-skill".to_string()]
        );
        assert_eq!(
            skill.metadata.file_extensions.as_ref().expect("extensions"),
            &vec!["pdf".to_string()]
        );
        assert_eq!(
            skill.metadata.file_globs.as_ref().expect("globs"),
            &vec!["*.pdf".to_string()]
        );
        assert_eq!(
            skill
                .metadata
                .extra
                .get("custom_scalar")
                .map(String::as_str),
            Some("plain-text")
        );
        assert_eq!(
            skill
                .metadata
                .extra
                .get("custom_sequence")
                .map(String::as_str),
            Some("- review\n- export")
        );
        assert_eq!(
            skill
                .metadata
                .extra
                .get("custom_mapping")
                .map(String::as_str),
            Some("region: eu\npriority: high")
        );
        assert!(skill.content.contains("Create invoices carefully."));
        assert!(skill.content.contains("Ask questions"));
        assert_eq!(skill.references.len(), 1);
        assert_eq!(skill.references[0].name, "usage.md");
        assert!(skill.references[0].content.contains("Use responsibly"));
        assert_eq!(skill.templates.len(), 1);
        assert_eq!(skill.templates[0].name, "invoice.html");
        assert!(skill.templates[0].content.contains("<html>"));
    }

    #[tokio::test]
    async fn load_skill_fails_when_skill_md_missing() {
        let root = create_temp_skill_root();
        fs::create_dir_all(root.path().join("missing-skill")).expect("create skill dir");

        let loader = SkillLoader::with_dir(root.path());
        let err = loader
            .load_skill("missing-skill")
            .await
            .expect_err("missing SKILL.md should fail");

        assert!(matches!(err, SkillError::NotFound(_)));
        assert!(err.to_string().contains("SKILL.md not found"));
    }

    #[tokio::test]
    async fn load_skill_fails_on_empty_frontmatter() {
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "empty-frontmatter",
            "---\n---\n# Empty\n",
            &[],
            &[],
        );

        let loader = SkillLoader::with_dir(root.path());
        let err = loader
            .load_skill("empty-frontmatter")
            .await
            .expect_err("empty frontmatter should fail");

        assert!(matches!(err, SkillError::InvalidFrontmatter(_)));
        assert!(err.to_string().contains("Empty YAML frontmatter"));
    }

    #[tokio::test]
    async fn load_skill_fails_on_missing_required_frontmatter_fields() {
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "missing-fields",
            r#"---
name: Missing Fields
description: Missing category
---
# Missing fields
"#,
            &[],
            &[],
        );

        let loader = SkillLoader::with_dir(root.path());
        let err = loader
            .load_skill("missing-fields")
            .await
            .expect_err("missing category should fail");

        assert!(matches!(err, SkillError::InvalidFrontmatter(_)));
        assert!(err.to_string().contains("Missing required field: category"));
    }

    #[tokio::test]
    async fn load_skill_fails_on_malformed_yaml() {
        let root = create_temp_skill_root();
        write_skill_fixture(
            root.path(),
            "bad-yaml",
            r#"---
name: Broken
description: Broken YAML
category: [oops
---
# Broken
"#,
            &[],
            &[],
        );

        let loader = SkillLoader::with_dir(root.path());
        let err = loader
            .load_skill("bad-yaml")
            .await
            .expect_err("malformed YAML should fail");

        assert!(matches!(err, SkillError::InvalidFrontmatter(_)));
    }

    #[tokio::test]
    async fn load_all_creates_missing_dir_and_returns_empty() {
        let root = create_temp_skill_root();
        let missing = root.path().join("nested-skills-dir");
        let loader = SkillLoader::with_dir(&missing);

        let skills = loader.load_all().await.expect("load all");

        assert!(skills.is_empty());
        assert!(missing.exists());
        assert!(missing.is_dir());
    }

    #[tokio::test]
    async fn load_all_returns_only_valid_skills_from_mixed_directory() {
        let root = create_temp_skill_root();

        write_skill_fixture(
            root.path(),
            "valid-skill",
            r#"---
name: Valid Skill
description: This one should load
category: generic
---
# Valid

## Overview

Loaded successfully.
"#,
            &[],
            &[],
        );

        fs::create_dir_all(root.path().join("missing-markdown")).expect("create invalid dir");
        write_skill_fixture(
            root.path(),
            "bad-frontmatter",
            r#"---
name: Invalid Skill
description: Missing category
---
# Invalid
"#,
            &[],
            &[],
        );

        let loader = SkillLoader::with_dir(root.path());
        let skills = loader.load_all().await.expect("load all");

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id.0, "valid-skill");
        assert_eq!(skills[0].metadata.name, "Valid Skill");
    }
}
