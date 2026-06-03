use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::skills::loader::SkillLoader;
use crate::skills::types::{Skill, SkillError, SkillFilter, SkillId, SkillResult};

/// In-memory skill registry with caching
#[derive(Debug)]
pub struct SkillRegistry {
    /// The skill loader used to load skills from disk
    loader: SkillLoader,
    /// Cached skills by ID
    skills: RwLock<HashMap<SkillId, Arc<Skill>>>,
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
            category_index: RwLock::new(HashMap::new()),
            keyword_index: RwLock::new(HashMap::new()),
            extension_index: RwLock::new(HashMap::new()),
        }
    }

    /// Initialize the registry by loading all skills from disk
    pub async fn initialize(&self) -> SkillResult<()> {
        self.refresh().await
    }

    /// Refresh all skills from disk, rebuilding indexes
    pub async fn refresh(&self) -> SkillResult<()> {
        tracing::info!(
            "Refreshing skill registry from {:?}",
            self.loader.skills_dir()
        );

        // Load all skills from disk
        let skills = self.loader.load_all().await?;

        // Clear existing caches
        {
            let mut skills_cache = self.skills.write().await;
            skills_cache.clear();

            let mut cat_index = self.category_index.write().await;
            cat_index.clear();

            let mut kw_index = self.keyword_index.write().await;
            kw_index.clear();

            let mut ext_index = self.extension_index.write().await;
            ext_index.clear();

            // Build new caches and indexes
            for skill in skills {
                let skill_id = skill.id.clone();
                let category = skill.category().to_string();

                // Add to skills cache
                skills_cache.insert(skill_id.clone(), Arc::new(skill));

                // Index by category
                cat_index
                    .entry(category)
                    .or_default()
                    .push(skill_id.clone());

                // Get skills cache entry for indexing
                if let Some(arc_skill) = skills_cache.get(&skill_id) {
                    // Index by keywords
                    if let Some(keywords) = arc_skill.metadata.keywords.as_ref() {
                        for keyword in keywords {
                            kw_index
                                .entry(keyword.to_lowercase())
                                .or_default()
                                .push(skill_id.clone());
                        }
                    }

                    // Index by file extensions
                    if let Some(exts) = arc_skill.metadata.file_extensions.as_ref() {
                        for ext in exts {
                            ext_index
                                .entry(ext.to_lowercase())
                                .or_default()
                                .push(skill_id.clone());
                        }
                    }
                }
            }
        }

        let count = self.skills.read().await.len();
        tracing::info!("Skill registry refreshed with {} skills", count);

        Ok(())
    }

    /// Get a skill by ID
    pub async fn get_skill(&self, id: &SkillId) -> SkillResult<Arc<Skill>> {
        let skills = self.skills.read().await;
        skills
            .get(id)
            .cloned()
            .ok_or_else(|| SkillError::NotFound(id.to_string()))
    }

    /// Get a skill by name (case-insensitive)
    pub async fn get_skill_by_name(&self, name: &str) -> SkillResult<Arc<Skill>> {
        let skills = self.skills.read().await;
        let name_lower = name.to_lowercase();

        skills
            .values()
            .find(|s| s.name().to_lowercase() == name_lower)
            .cloned()
            .ok_or_else(|| SkillError::NotFound(format!("Skill with name: {}", name)))
    }

    /// Get all skills
    pub async fn get_all_skills(&self) -> Vec<Arc<Skill>> {
        let skills = self.skills.read().await;
        skills.values().cloned().collect()
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
                    || s.metadata.keywords.as_ref().map_or(false, |kw| {
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

        // Update caches
        {
            let mut skills = self.skills.write().await;
            skills.insert(skill_id.clone(), skill_arc.clone());

            // Update category index
            let mut cat_index = self.category_index.write().await;
            let entries = cat_index
                .entry(skill_arc.category().to_string())
                .or_default();
            if !entries.contains(&skill_id) {
                entries.push(skill_id.clone());
            }

            // Update keyword index
            let mut kw_index = self.keyword_index.write().await;
            if let Some(keywords) = skill_arc.metadata.keywords.as_ref() {
                for keyword in keywords {
                    let entries = kw_index.entry(keyword.to_lowercase()).or_default();
                    if !entries.contains(&skill_id) {
                        entries.push(skill_id.clone());
                    }
                }
            }

            // Update extension index
            let mut ext_index = self.extension_index.write().await;
            if let Some(exts) = skill_arc.metadata.file_extensions.as_ref() {
                for ext in exts {
                    let entries = ext_index.entry(ext.to_lowercase()).or_default();
                    if !entries.contains(&skill_id) {
                        entries.push(skill_id.clone());
                    }
                }
            }
        }

        Ok(skill_arc)
    }

    /// Remove a skill from the registry
    pub async fn remove_skill(&self, id: &SkillId) -> SkillResult<()> {
        let mut skills = self.skills.write().await;

        let skill = skills
            .remove(id)
            .ok_or_else(|| SkillError::NotFound(id.to_string()))?;

        // Remove from indexes
        let mut cat_index = self.category_index.write().await;
        if let Some(ids) = cat_index.get_mut(skill.category()) {
            ids.retain(|sid| sid != id);
            if ids.is_empty() {
                cat_index.remove(skill.category());
            }
        }

        let mut kw_index = self.keyword_index.write().await;
        if let Some(keywords) = skill.metadata.keywords.as_ref() {
            for keyword in keywords {
                if let Some(ids) = kw_index.get_mut(&keyword.to_lowercase()) {
                    ids.retain(|sid| sid != id);
                    if ids.is_empty() {
                        kw_index.remove(&keyword.to_lowercase());
                    }
                }
            }
        }

        let mut ext_index = self.extension_index.write().await;
        if let Some(exts) = skill.metadata.file_extensions.as_ref() {
            for ext in exts {
                let ext_lower = ext.to_lowercase();
                if let Some(ids) = ext_index.get_mut(&ext_lower) {
                    ids.retain(|sid| sid != id);
                    if ids.is_empty() {
                        ext_index.remove(&ext_lower);
                    }
                }
            }
        }

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
            sections: Default::default(),
            raw_content: String::new(),
            path: std::path::PathBuf::from(format!("/test/{}", id)),
            references: vec![],
            templates: vec![],
            loaded_at: chrono::Utc::now(),
            modified_at: None,
        }
    }

    #[tokio::test]
    async fn test_skill_registry_basic_operations() {
        let registry = SkillRegistry::new();

        // Manually insert a skill for testing
        let skill = create_test_skill("test-skill", "dev");
        let skill_id = skill.id.clone();

        {
            let mut skills = registry.skills.write().await;
            let mut cat_index = registry.category_index.write().await;
            let mut kw_index = registry.keyword_index.write().await;
            let mut ext_index = registry.extension_index.write().await;

            skills.insert(skill_id.clone(), Arc::new(skill));
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
}
