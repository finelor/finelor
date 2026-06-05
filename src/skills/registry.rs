use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::skills::loader::SkillLoader;
use crate::skills::types::{Skill, SkillError, SkillFilter, SkillId, SkillResult};

type SkillCache = HashMap<SkillId, Arc<Skill>>;
type SkillNameIndex = HashMap<String, SkillId>;
type SkillListIndex = HashMap<String, Vec<SkillId>>;

fn supporting_file_names<'a, T: 'a>(
    items: impl Iterator<Item = &'a T>,
    get_name: impl Fn(&'a T) -> String,
) -> Vec<String> {
    items.map(get_name).collect()
}

/// In-memory skill registry with caching
#[derive(Debug)]
pub struct SkillRegistry {
    /// The skill loader used to load skills from disk
    loader: SkillLoader,
    /// Cached skills by ID
    skills: RwLock<HashMap<SkillId, Arc<Skill>>>,
    /// Index of skills by normalized display name
    name_index: RwLock<HashMap<String, SkillId>>,
    /// Index of skills by category
    category_index: RwLock<HashMap<String, Vec<SkillId>>>,
    /// Index of skills by keyword
    keyword_index: RwLock<HashMap<String, Vec<SkillId>>>,
    /// Index of skills by file extension
    extension_index: RwLock<HashMap<String, Vec<SkillId>>>,
}

impl SkillRegistry {
    /// Create a new skill registry with the default skills directory
    pub fn new() -> Self {
        Self {
            loader: SkillLoader::new(),
            skills: RwLock::new(HashMap::new()),
            name_index: RwLock::new(HashMap::new()),
            category_index: RwLock::new(HashMap::new()),
            keyword_index: RwLock::new(HashMap::new()),
            extension_index: RwLock::new(HashMap::new()),
        }
    }

    /// Create a skill registry with a custom skills directory
    pub fn with_dir(path: &Path) -> Self {
        Self {
            loader: SkillLoader::with_dir(path),
            skills: RwLock::new(HashMap::new()),
            name_index: RwLock::new(HashMap::new()),
            category_index: RwLock::new(HashMap::new()),
            keyword_index: RwLock::new(HashMap::new()),
            extension_index: RwLock::new(HashMap::new()),
        }
    }

    fn normalize_skill_name(name: &str) -> String {
        name.trim().to_lowercase()
    }

    fn duplicate_skill_name_error(
        existing_skill: &Skill,
        incoming_skill: &Skill,
        incoming_path: &str,
    ) -> SkillError {
        SkillError::DuplicateSkillName {
            name: incoming_skill.name().to_string(),
            first_id: existing_skill.id.0.clone(),
            second_id: incoming_skill.id.0.clone(),
            first_path: existing_skill.path.display().to_string(),
            second_path: incoming_path.to_string(),
        }
    }

    fn log_duplicate_skill_name(
        existing_skill: &Skill,
        incoming_skill: &Skill,
        incoming_path: &str,
    ) {
        tracing::error!(
            duplicate_skill_name = incoming_skill.name(),
            first_skill_id = %existing_skill.id,
            second_skill_id = %incoming_skill.id,
            first_skill_path = %existing_skill.path.display(),
            second_skill_path = %incoming_path,
            "Duplicate skill display name detected while rebuilding registry indexes"
        );
    }

    fn add_skill_to_indexes(
        skills: &mut SkillCache,
        name_index: &mut SkillNameIndex,
        category_index: &mut SkillListIndex,
        keyword_index: &mut SkillListIndex,
        extension_index: &mut SkillListIndex,
        skill: Arc<Skill>,
    ) -> SkillResult<()> {
        let normalized_name = Self::normalize_skill_name(skill.name());

        if let Some(existing_id) = name_index.get(&normalized_name).cloned()
            && let Some(existing_skill) = skills.get(&existing_id)
            && existing_skill.id != skill.id
        {
            let incoming_path = skill.path.display().to_string();
            Self::log_duplicate_skill_name(existing_skill, skill.as_ref(), &incoming_path);
            return Err(Self::duplicate_skill_name_error(
                existing_skill,
                skill.as_ref(),
                &incoming_path,
            ));
        }

        let skill_id = skill.id.clone();
        name_index.insert(normalized_name, skill_id.clone());
        category_index
            .entry(skill.category().to_string())
            .or_default()
            .push(skill_id.clone());

        if let Some(keywords) = skill.metadata.keywords.as_ref() {
            for keyword in keywords {
                keyword_index
                    .entry(keyword.to_lowercase())
                    .or_default()
                    .push(skill_id.clone());
            }
        }

        if let Some(exts) = skill.metadata.file_extensions.as_ref() {
            for ext in exts {
                extension_index
                    .entry(ext.to_lowercase())
                    .or_default()
                    .push(skill_id.clone());
            }
        }

        skills.insert(skill_id, skill);
        Ok(())
    }

    fn remove_skill_from_indexes(
        name_index: &mut SkillNameIndex,
        category_index: &mut SkillListIndex,
        keyword_index: &mut SkillListIndex,
        extension_index: &mut SkillListIndex,
        skill: &Skill,
    ) {
        name_index.remove(&Self::normalize_skill_name(skill.name()));

        if let Some(ids) = category_index.get_mut(skill.category()) {
            ids.retain(|sid| sid != &skill.id);
            if ids.is_empty() {
                category_index.remove(skill.category());
            }
        }

        if let Some(keywords) = skill.metadata.keywords.as_ref() {
            for keyword in keywords {
                let keyword_lower = keyword.to_lowercase();
                if let Some(ids) = keyword_index.get_mut(&keyword_lower) {
                    ids.retain(|sid| sid != &skill.id);
                    if ids.is_empty() {
                        keyword_index.remove(&keyword_lower);
                    }
                }
            }
        }

        if let Some(exts) = skill.metadata.file_extensions.as_ref() {
            for ext in exts {
                let ext_lower = ext.to_lowercase();
                if let Some(ids) = extension_index.get_mut(&ext_lower) {
                    ids.retain(|sid| sid != &skill.id);
                    if ids.is_empty() {
                        extension_index.remove(&ext_lower);
                    }
                }
            }
        }
    }

    /// Initialize the registry by loading all skills from disk
    pub async fn initialize(&self) -> SkillResult<()> {
        self.refresh().await
    }

    /// Refresh all skills from disk, rebuilding indexes
    pub async fn refresh(&self) -> SkillResult<()> {
        tracing::info!(
            skills_dir = %self.loader.skills_dir().display(),
            "Refreshing skills registry from disk"
        );

        // Load all skills from disk
        let skills = self.loader.load_all().await?;
        tracing::debug!(
            loaded_skill_count = skills.len(),
            "Rebuilding skills registry cache from loaded skill packages"
        );

        let mut next_skills = HashMap::new();
        let mut next_name_index = HashMap::new();
        let mut next_category_index = HashMap::new();
        let mut next_keyword_index = HashMap::new();
        let mut next_extension_index = HashMap::new();

        for skill in skills {
            tracing::debug!(
                skill_id = %skill.id,
                skill_name = skill.name(),
                category = skill.category(),
                keyword_count = skill.metadata.keywords.as_ref().map(|k| k.len()).unwrap_or(0),
                extension_count = skill.metadata.file_extensions.as_ref().map(|e| e.len()).unwrap_or(0),
                reference_names = ?supporting_file_names(
                    skill.references.iter(),
                    |reference| reference.name.clone()
                ),
                template_names = ?supporting_file_names(
                    skill.templates.iter(),
                    |template| template.name.clone()
                ),
                "Caching loaded skill package in staged registry rebuild"
            );

            Self::add_skill_to_indexes(
                &mut next_skills,
                &mut next_name_index,
                &mut next_category_index,
                &mut next_keyword_index,
                &mut next_extension_index,
                Arc::new(skill),
            )?;
        }

        {
            let mut skills_cache = self.skills.write().await;
            let mut name_index = self.name_index.write().await;
            let mut cat_index = self.category_index.write().await;
            let mut kw_index = self.keyword_index.write().await;
            let mut ext_index = self.extension_index.write().await;

            *skills_cache = next_skills;
            *name_index = next_name_index;
            *cat_index = next_category_index;
            *kw_index = next_keyword_index;
            *ext_index = next_extension_index;
        }

        let count = self.skills.read().await.len();
        let category_count = self.category_index.read().await.len();
        let keyword_count = self.keyword_index.read().await.len();
        let extension_count = self.extension_index.read().await.len();
        tracing::info!(
            skill_count = count,
            category_count,
            keyword_count,
            extension_count,
            "Skill registry cache refreshed successfully"
        );

        Ok(())
    }

    /// Get a skill by ID
    pub async fn get_skill(&self, id: &SkillId) -> SkillResult<Arc<Skill>> {
        let skills = self.skills.read().await;
        let skill = skills
            .get(id)
            .cloned()
            .ok_or_else(|| SkillError::NotFound(id.to_string()))?;
        tracing::debug!(
            skill_id = %id,
            skill_name = skill.name(),
            "Resolved skill from registry by id"
        );
        Ok(skill)
    }

    /// Get a skill by name (case-insensitive)
    pub async fn get_skill_by_name(&self, name: &str) -> SkillResult<Arc<Skill>> {
        let normalized_name = Self::normalize_skill_name(name);
        let skill_id = {
            let name_index = self.name_index.read().await;
            name_index
                .get(&normalized_name)
                .cloned()
                .ok_or_else(|| SkillError::NotFound(format!("Skill with name: {}", name)))?
        };
        let skills = self.skills.read().await;

        let skill = skills
            .get(&skill_id)
            .cloned()
            .ok_or_else(|| SkillError::NotFound(format!("Skill with name: {}", name)))?;
        tracing::debug!(
            requested_skill_name = name,
            skill_id = %skill.id,
            skill_name = skill.name(),
            "Resolved skill from registry by name"
        );
        Ok(skill)
    }

    /// Get all skills
    pub async fn get_all_skills(&self) -> Vec<Arc<Skill>> {
        let skills = self.skills.read().await;
        let mut all = skills.values().cloned().collect::<Vec<_>>();
        all.sort_by(|left, right| {
            left.name()
                .cmp(right.name())
                .then_with(|| left.id.0.cmp(&right.id.0))
        });
        tracing::debug!(
            skill_count = all.len(),
            skill_names = ?all.iter().map(|skill| skill.name().to_string()).collect::<Vec<_>>(),
            "Enumerated all skills from registry cache"
        );
        all
    }

    /// Get skills by category
    pub async fn get_skills_by_category(&self, category: &str) -> Vec<Arc<Skill>> {
        let cat_index = self.category_index.read().await;
        let mut result = Vec::new();

        if let Some(ids) = cat_index.get(category) {
            let skills = self.skills.read().await;
            for id in ids {
                if let Some(skill) = skills.get(id) {
                    result.push(skill.clone());
                }
            }
        }

        result
    }

    /// Get skills by keyword
    pub async fn get_skills_by_keyword(&self, keyword: &str) -> Vec<Arc<Skill>> {
        let kw_index = self.keyword_index.read().await;
        let keyword_lower = keyword.to_lowercase();
        let mut result = Vec::new();

        if let Some(ids) = kw_index.get(&keyword_lower) {
            let skills = self.skills.read().await;
            for id in ids {
                if let Some(skill) = skills.get(id) {
                    result.push(skill.clone());
                }
            }
        }

        result
    }

    /// Get skills applicable to a file extension
    pub async fn get_skills_for_extension(&self, extension: &str) -> Vec<Arc<Skill>> {
        let ext_index = self.extension_index.read().await;
        let ext_lower = extension.to_lowercase().trim_start_matches('.').to_string();
        let mut result = Vec::new();

        if let Some(ids) = ext_index.get(&ext_lower) {
            let skills = self.skills.read().await;
            for id in ids {
                if let Some(skill) = skills.get(id) {
                    result.push(skill.clone());
                }
            }
        }

        result
    }

    /// Get all unique categories
    pub async fn get_categories(&self) -> Vec<String> {
        let cat_index = self.category_index.read().await;
        cat_index.keys().cloned().collect()
    }

    /// Get all unique keywords
    pub async fn get_keywords(&self) -> Vec<String> {
        let kw_index = self.keyword_index.read().await;
        kw_index.keys().cloned().collect()
    }

    /// Filter skills using a SkillFilter
    pub async fn filter_skills(&self, filter: &SkillFilter) -> Vec<Arc<Skill>> {
        let skills = self.skills.read().await;
        skills
            .values()
            .filter(|s| filter.matches(s))
            .cloned()
            .collect()
    }

    /// Search skills by name or description
    pub async fn search_skills(&self, query: &str) -> Vec<Arc<Skill>> {
        let skills = self.skills.read().await;
        let query_lower = query.to_lowercase();

        skills
            .values()
            .filter(|s| {
                s.name().to_lowercase().contains(&query_lower)
                    || s.description().to_lowercase().contains(&query_lower)
                    || s.metadata.keywords.as_ref().is_some_and(|kw| {
                        kw.iter().any(|k| k.to_lowercase().contains(&query_lower))
                    })
            })
            .cloned()
            .collect()
    }

    /// Load a single skill by name (useful for reloading)
    pub async fn load_skill(&self, name: &str) -> SkillResult<Arc<Skill>> {
        let skill = self.loader.load_skill(name).await?;
        let skill_id = skill.id.clone();
        let skill_arc = Arc::new(skill);

        {
            let mut skills = self.skills.write().await;
            let mut name_index = self.name_index.write().await;
            let mut cat_index = self.category_index.write().await;
            let mut kw_index = self.keyword_index.write().await;
            let mut ext_index = self.extension_index.write().await;

            let mut next_skills = skills.clone();
            let mut next_name_index = name_index.clone();
            let mut next_cat_index = cat_index.clone();
            let mut next_kw_index = kw_index.clone();
            let mut next_ext_index = ext_index.clone();

            if let Some(previous_skill) = next_skills.remove(&skill_id) {
                Self::remove_skill_from_indexes(
                    &mut next_name_index,
                    &mut next_cat_index,
                    &mut next_kw_index,
                    &mut next_ext_index,
                    previous_skill.as_ref(),
                );
            }

            Self::add_skill_to_indexes(
                &mut next_skills,
                &mut next_name_index,
                &mut next_cat_index,
                &mut next_kw_index,
                &mut next_ext_index,
                skill_arc.clone(),
            )?;

            *skills = next_skills;
            *name_index = next_name_index;
            *cat_index = next_cat_index;
            *kw_index = next_kw_index;
            *ext_index = next_ext_index;
        }

        tracing::info!(
            skill_id = %skill_id,
            skill_name = skill_arc.name(),
            category = skill_arc.category(),
            reference_count = skill_arc.references.len(),
            template_count = skill_arc.templates.len(),
            "Loaded skill package into registry cache"
        );

        Ok(skill_arc)
    }

    /// Remove a skill from the registry
    pub async fn remove_skill(&self, id: &SkillId) -> SkillResult<()> {
        let mut skills = self.skills.write().await;
        let mut name_index = self.name_index.write().await;
        let mut cat_index = self.category_index.write().await;
        let mut kw_index = self.keyword_index.write().await;
        let mut ext_index = self.extension_index.write().await;

        let skill = skills
            .remove(id)
            .ok_or_else(|| SkillError::NotFound(id.to_string()))?;

        Self::remove_skill_from_indexes(
            &mut name_index,
            &mut cat_index,
            &mut kw_index,
            &mut ext_index,
            skill.as_ref(),
        );

        tracing::info!(
            skill_id = %id,
            skill_name = skill.name(),
            "Removed skill package from registry cache"
        );

        Ok(())
    }

    /// Get the number of skills in the registry
    pub async fn len(&self) -> usize {
        self.skills.read().await.len()
    }

    /// Check if the registry is empty
    pub async fn is_empty(&self) -> bool {
        self.skills.read().await.is_empty()
    }

    /// Get the skills directory path
    pub fn skills_dir(&self) -> &Path {
        self.loader.skills_dir()
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared skill registry type alias for use in application state
pub type SharedSkillRegistry = Arc<SkillRegistry>;

/// Create a shared skill registry and initialize it
pub async fn create_skill_registry() -> SkillResult<SharedSkillRegistry> {
    let registry = Arc::new(SkillRegistry::new());
    registry.initialize().await?;
    Ok(registry)
}

/// Create a shared skill registry with a custom directory
pub async fn create_skill_registry_with_path(path: &Path) -> SkillResult<SharedSkillRegistry> {
    let registry = Arc::new(SkillRegistry::with_dir(path));
    registry.initialize().await?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::types::SkillMetadata;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_skill(id: &str, category: &str) -> Skill {
        Skill {
            id: SkillId::new(id),
            metadata: SkillMetadata {
                name: id.to_string(),
                description: format!("Description for {}", id),
                category: category.to_string(),
                version: None,
                author: None,
                keywords: Some(vec!["test".to_string(), "example".to_string()]),
                depends_on: None,
                file_extensions: Some(vec!["rs".to_string()]),
                file_globs: None,
                extra: HashMap::new(),
            },
            content: String::new(),
            path: std::path::PathBuf::from(format!("/test/{}", id)),
            references: vec![],
            templates: vec![],
            loaded_at: chrono::Utc::now(),
            modified_at: None,
        }
    }

    fn write_skill(root: &TempDir, id: &str, name: &str, category: &str, extra_body: &str) {
        let skill_dir = root.path().join(id);
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                r#"---
name: {name}
description: Description for {id}
category: {category}
keywords:
  - test
  - example
file_extensions:
  - rs
---
# {name}

{extra_body}
"#
            ),
        )
        .expect("write skill");
    }

    #[tokio::test]
    async fn test_skill_registry_basic_operations() {
        let registry = SkillRegistry::new();

        // Manually insert a skill for testing
        let skill = create_test_skill("test-skill", "dev");
        let skill_id = skill.id.clone();

        {
            let mut skills = registry.skills.write().await;
            let mut name_index = registry.name_index.write().await;
            let mut cat_index = registry.category_index.write().await;
            let mut kw_index = registry.keyword_index.write().await;
            let mut ext_index = registry.extension_index.write().await;

            skills.insert(skill_id.clone(), Arc::new(skill));
            name_index.insert("test-skill".to_string(), skill_id.clone());
            cat_index
                .entry("dev".to_string())
                .or_default()
                .push(skill_id.clone());
            kw_index
                .entry("test".to_string())
                .or_default()
                .push(skill_id.clone());
            ext_index
                .entry("rs".to_string())
                .or_default()
                .push(skill_id.clone());
        }

        // Test get_skill
        let retrieved = registry.get_skill(&skill_id).await;
        assert!(retrieved.is_ok());
        assert_eq!(retrieved.unwrap().name(), "test-skill");

        // Test get_skill_by_name
        let by_name = registry.get_skill_by_name("test-skill").await;
        assert!(by_name.is_ok());

        // Test categories
        let categories = registry.get_categories().await;
        assert!(categories.contains(&"dev".to_string()));

        // Test get_skills_by_category
        let dev_skills = registry.get_skills_by_category("dev").await;
        assert_eq!(dev_skills.len(), 1);

        // Test get_skills_by_keyword
        let test_skills = registry.get_skills_by_keyword("test").await;
        assert_eq!(test_skills.len(), 1);

        // Test get_skills_for_extension
        let rs_skills = registry.get_skills_for_extension("rs").await;
        assert_eq!(rs_skills.len(), 1);

        // Test search
        let search_results = registry.search_skills("test").await;
        assert!(!search_results.is_empty());

        // Test filter
        let filter = SkillFilter::by_category("dev");
        let filtered = registry.filter_skills(&filter).await;
        assert_eq!(filtered.len(), 1);

        // Test remove
        registry.remove_skill(&skill_id).await.unwrap();
        assert!(registry.get_skill(&skill_id).await.is_err());
    }

    #[tokio::test]
    async fn test_skill_registry_empty() {
        let registry = SkillRegistry::new();
        assert!(registry.is_empty().await);
        assert_eq!(registry.len().await, 0);

        let all = registry.get_all_skills().await;
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_get_all_skills_returns_stable_sorted_order() {
        let registry = SkillRegistry::new();
        let earlier_alpha = Skill {
            id: SkillId::new("a-id"),
            metadata: SkillMetadata {
                name: "Alpha".to_string(),
                description: "Description for Alpha".to_string(),
                category: "dev".to_string(),
                version: None,
                author: None,
                keywords: None,
                depends_on: None,
                file_extensions: None,
                file_globs: None,
                extra: HashMap::new(),
            },
            content: String::new(),
            path: std::path::PathBuf::from("/test/a-id"),
            references: vec![],
            templates: vec![],
            loaded_at: chrono::Utc::now(),
            modified_at: None,
        };
        let beta = Skill {
            id: SkillId::new("b-id"),
            metadata: SkillMetadata {
                name: "Beta".to_string(),
                description: "Description for Beta".to_string(),
                category: "dev".to_string(),
                version: None,
                author: None,
                keywords: None,
                depends_on: None,
                file_extensions: None,
                file_globs: None,
                extra: HashMap::new(),
            },
            content: String::new(),
            path: std::path::PathBuf::from("/test/b-id"),
            references: vec![],
            templates: vec![],
            loaded_at: chrono::Utc::now(),
            modified_at: None,
        };
        let duplicate_alpha = Skill {
            id: SkillId::new("z-id"),
            metadata: SkillMetadata {
                name: "Alpha".to_string(),
                description: "Description for duplicate Alpha".to_string(),
                category: "dev".to_string(),
                version: None,
                author: None,
                keywords: None,
                depends_on: None,
                file_extensions: None,
                file_globs: None,
                extra: HashMap::new(),
            },
            content: String::new(),
            path: std::path::PathBuf::from("/test/z-id"),
            references: vec![],
            templates: vec![],
            loaded_at: chrono::Utc::now(),
            modified_at: None,
        };

        {
            let mut skills = registry.skills.write().await;
            skills.insert(duplicate_alpha.id.clone(), Arc::new(duplicate_alpha));
            skills.insert(beta.id.clone(), Arc::new(beta));
            skills.insert(earlier_alpha.id.clone(), Arc::new(earlier_alpha));
        }

        let all = registry.get_all_skills().await;
        let ordered = all
            .iter()
            .map(|skill| (skill.name().to_string(), skill.id.0.clone()))
            .collect::<Vec<_>>();

        assert_eq!(
            ordered,
            vec![
                ("Alpha".to_string(), "a-id".to_string()),
                ("Alpha".to_string(), "z-id".to_string()),
                ("Beta".to_string(), "b-id".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn test_registry_initialize_fails_on_duplicate_display_names() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(root.path().join("skill-one")).expect("skill one dir");
        std::fs::create_dir_all(root.path().join("skill-two")).expect("skill two dir");
        std::fs::write(
            root.path().join("skill-one").join("SKILL.md"),
            r#"---
name: Shared Name
description: First skill
category: ops
---
# Skill One
"#,
        )
        .expect("write first skill");
        std::fs::write(
            root.path().join("skill-two").join("SKILL.md"),
            r#"---
name: Shared Name
description: Second skill
category: ops
---
# Skill Two
"#,
        )
        .expect("write second skill");

        let registry = SkillRegistry::with_dir(root.path());
        let err = registry
            .initialize()
            .await
            .expect_err("duplicate display names should fail");

        match err {
            SkillError::DuplicateSkillName {
                name,
                first_id,
                second_id,
                first_path,
                second_path,
            } => {
                assert_eq!(name, "Shared Name");
                assert!([first_id.as_str(), second_id.as_str()].contains(&"skill-one"));
                assert!([first_id.as_str(), second_id.as_str()].contains(&"skill-two"));
                assert!(first_path.contains("skill-"));
                assert!(second_path.contains("skill-"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn test_registry_initialize_fails_on_case_only_duplicate_display_names() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(root.path().join("skill-one")).expect("skill one dir");
        std::fs::create_dir_all(root.path().join("skill-two")).expect("skill two dir");
        std::fs::write(
            root.path().join("skill-one").join("SKILL.md"),
            r#"---
name: Shared Name
description: First skill
category: ops
---
# Skill One
"#,
        )
        .expect("write first skill");
        std::fs::write(
            root.path().join("skill-two").join("SKILL.md"),
            r#"---
name: shared name
description: Second skill
category: ops
---
# Skill Two
"#,
        )
        .expect("write second skill");

        let registry = SkillRegistry::with_dir(root.path());
        let err = registry
            .refresh()
            .await
            .expect_err("case-insensitive duplicate display names should fail");

        assert!(matches!(err, SkillError::DuplicateSkillName { .. }));
    }

    #[tokio::test]
    async fn test_refresh_is_atomic_on_duplicate_name_failure() {
        let root = tempfile::tempdir().expect("tempdir");
        write_skill(&root, "alpha-skill", "Alpha", "ops", "Initial alpha skill");
        let registry = SkillRegistry::with_dir(root.path());
        registry.initialize().await.expect("initial registry load");

        write_skill(
            &root,
            "beta-skill",
            "Alpha",
            "ops",
            "Conflicting duplicate skill",
        );

        let err = registry
            .refresh()
            .await
            .expect_err("duplicate refresh should fail");
        assert!(matches!(err, SkillError::DuplicateSkillName { .. }));

        let all_skills = registry.get_all_skills().await;
        assert_eq!(all_skills.len(), 1);
        assert_eq!(all_skills[0].id.0, "alpha-skill");

        let by_name = registry
            .get_skill_by_name("Alpha")
            .await
            .expect("old registry state should remain available");
        assert_eq!(by_name.id.0, "alpha-skill");
    }

    #[tokio::test]
    async fn test_load_skill_populates_name_index() {
        let root = tempfile::tempdir().expect("tempdir");
        write_skill(&root, "alpha-skill", "Alpha Skill", "ops", "Alpha");

        let registry = SkillRegistry::with_dir(root.path());
        let loaded = registry
            .load_skill("alpha-skill")
            .await
            .expect("load single skill");

        assert_eq!(loaded.id.0, "alpha-skill");

        let by_name = registry
            .get_skill_by_name(" alpha skill ")
            .await
            .expect("skill should resolve through name index");
        assert_eq!(by_name.id.0, "alpha-skill");
    }

    #[tokio::test]
    async fn test_load_skill_replaces_all_indexes_for_same_id() {
        let root = tempfile::tempdir().expect("tempdir");
        write_skill(&root, "alpha-skill", "Alpha Skill", "ops", "Alpha");

        let registry = SkillRegistry::with_dir(root.path());
        registry
            .load_skill("alpha-skill")
            .await
            .expect("initial load");

        std::fs::write(
            root.path().join("alpha-skill").join("SKILL.md"),
            r#"---
name: Renamed Skill
description: Description for alpha-skill
category: finance
keywords:
  - refreshed
file_extensions:
  - pdf
---
# Renamed Skill

Updated skill body.
"#,
        )
        .expect("rewrite skill");

        let reloaded = registry
            .load_skill("alpha-skill")
            .await
            .expect("reload same skill id");
        assert_eq!(reloaded.name(), "Renamed Skill");

        assert!(registry.get_skill_by_name("Alpha Skill").await.is_err());
        let renamed = registry
            .get_skill_by_name("Renamed Skill")
            .await
            .expect("new name should resolve");
        assert_eq!(renamed.id.0, "alpha-skill");

        assert!(registry.get_skills_by_category("ops").await.is_empty());
        assert_eq!(registry.get_skills_by_category("finance").await.len(), 1);
        assert!(registry.get_skills_by_keyword("test").await.is_empty());
        assert_eq!(registry.get_skills_by_keyword("refreshed").await.len(), 1);
        assert!(registry.get_skills_for_extension("rs").await.is_empty());
        assert_eq!(registry.get_skills_for_extension("pdf").await.len(), 1);
    }

    #[tokio::test]
    async fn test_load_skill_rejects_duplicate_display_name_from_different_id() {
        let root = tempfile::tempdir().expect("tempdir");
        write_skill(&root, "alpha-skill", "Shared Name", "ops", "Alpha");
        write_skill(&root, "beta-skill", "Shared Name", "ops", "Beta");

        let registry = SkillRegistry::with_dir(root.path());
        registry
            .load_skill("alpha-skill")
            .await
            .expect("load first skill");

        let err = registry
            .load_skill("beta-skill")
            .await
            .expect_err("duplicate display name should fail");
        assert!(matches!(err, SkillError::DuplicateSkillName { .. }));

        let all_skills = registry.get_all_skills().await;
        assert_eq!(all_skills.len(), 1);
        assert_eq!(all_skills[0].id.0, "alpha-skill");
    }

    #[tokio::test]
    async fn test_remove_skill_cleans_all_indexes() {
        let root = tempfile::tempdir().expect("tempdir");
        write_skill(&root, "alpha-skill", "Alpha Skill", "ops", "Alpha");

        let registry = SkillRegistry::with_dir(root.path());
        let skill = registry
            .load_skill("alpha-skill")
            .await
            .expect("load skill");

        registry
            .remove_skill(&skill.id)
            .await
            .expect("remove loaded skill");

        assert!(registry.get_skill(&skill.id).await.is_err());
        assert!(registry.get_skill_by_name("Alpha Skill").await.is_err());
        assert!(registry.get_skills_by_category("ops").await.is_empty());
        assert!(registry.get_skills_by_keyword("test").await.is_empty());
        assert!(registry.get_skills_for_extension("rs").await.is_empty());
    }
}
