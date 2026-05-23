pub mod manifest;

pub use manifest::SkillManifest;

use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct SkillLoader {
    search_dirs: Vec<PathBuf>,
    manifests: Vec<LoadedSkill>,
}

pub struct LoadedSkill {
    pub manifest: SkillManifest,
    pub source_dir: PathBuf,
}

impl SkillLoader {
    pub fn new(search_dir: PathBuf) -> Self {
        Self {
            search_dirs: vec![search_dir],
            manifests: Vec::new(),
        }
    }

    pub fn with_dirs(dirs: Vec<PathBuf>) -> Self {
        Self {
            search_dirs: dirs,
            manifests: Vec::new(),
        }
    }

    pub fn with_defaults() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let dirs = vec![
            cwd.join(".agent").join("skills"),
            cwd.join(".claude").join("skills"),
            cwd.join("skills"),
            PathBuf::from(&home).join(".agent").join("skills"),
            PathBuf::from(&home).join(".claude").join("skills"),
        ];

        Self {
            search_dirs: dirs,
            manifests: Vec::new(),
        }
    }

    pub fn add_search_dir(&mut self, dir: PathBuf) {
        self.search_dirs.push(dir);
    }

    pub fn scan(&mut self) -> Result<usize> {
        self.manifests.clear();
        let mut seen_names = std::collections::HashSet::new();

        for dir in &self.search_dirs {
            if !dir.exists() {
                continue;
            }

            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();

                if !path.is_dir() {
                    continue;
                }

                let manifest_path = path.join("SKILL.md");
                if manifest_path.exists() {
                    if let Ok(mut manifest) = SkillManifest::from_file(&manifest_path) {
                        if manifest.content_path.as_os_str().is_empty() {
                            manifest.content_path = manifest_path;
                        }
                        if seen_names.insert(manifest.name.clone()) {
                            self.manifests.push(LoadedSkill {
                                manifest,
                                source_dir: dir.clone(),
                            });
                        }
                    }
                }
            }
        }

        Ok(self.manifests.len())
    }

    pub fn list(&self) -> Vec<&SkillManifest> {
        self.manifests.iter().map(|s| &s.manifest).collect()
    }

    pub fn list_with_sources(&self) -> Vec<(&SkillManifest, &Path)> {
        self.manifests
            .iter()
            .map(|s| (&s.manifest, s.source_dir.as_path()))
            .collect()
    }

    pub fn find_by_name(&self, name: &str) -> Option<&SkillManifest> {
        self.manifests.iter().find(|s| s.manifest.name == name).map(|s| &s.manifest)
    }

    pub fn find_by_trigger(&self, query: &str) -> Vec<&SkillManifest> {
        let query_lower = query.to_lowercase();
        self.manifests
            .iter()
            .filter(|s| {
                s.manifest
                    .triggers
                    .iter()
                    .any(|t| query_lower.contains(&t.to_lowercase()))
                    || query_lower.contains(&s.manifest.name.to_lowercase())
            })
            .map(|s| &s.manifest)
            .collect()
    }

    pub fn load_content(&self, manifest: &SkillManifest) -> Result<String> {
        let skill = self
            .manifests
            .iter()
            .find(|s| s.manifest.name == manifest.name);

        let content_path = if manifest.content_path.is_absolute() {
            manifest.content_path.clone()
        } else if let Some(skill) = skill {
            skill.source_dir.join(&manifest.content_path)
        } else {
            manifest.content_path.clone()
        };

        Ok(std::fs::read_to_string(content_path)?)
    }

    pub fn load_skill_context(&self, name: &str) -> Result<Option<String>> {
        if let Some(manifest) = self.find_by_name(name) {
            let content = self.load_content(manifest)?;
            Ok(Some(format!(
                "== Skill: {} ==\n{}\n== End Skill: {} ==\n",
                manifest.name, content, manifest.name
            )))
        } else {
            Ok(None)
        }
    }

    pub fn search_dirs(&self) -> &[PathBuf] {
        &self.search_dirs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_skill(dir: &Path, name: &str, display_name: &str) {
        fs::create_dir_all(dir.join(name)).unwrap();
        fs::write(
            dir.join(name).join("SKILL.md"),
            format!(
                r#"---
name: {}
description: A test skill
triggers:
  - test
---
Test content"#,
                display_name
            ),
        )
        .unwrap();
    }

    #[test]
    fn test_single_dir_scan() {
        let dir = PathBuf::from("/tmp/test_skills_single");
        let _ = fs::remove_dir_all(&dir);
        write_skill(&dir, "skill_a", "skill-a");

        let mut loader = SkillLoader::new(dir.clone());
        let count = loader.scan().unwrap();
        assert_eq!(count, 1);
        assert_eq!(loader.list()[0].name, "skill-a");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_multi_dir_dedup() {
        let dir1 = PathBuf::from("/tmp/test_skills_multi1");
        let dir2 = PathBuf::from("/tmp/test_skills_multi2");
        let _ = fs::remove_dir_all(&dir1);
        let _ = fs::remove_dir_all(&dir2);

        write_skill(&dir1, "s1", "shared-skill");
        write_skill(&dir2, "s1", "shared-skill");
        write_skill(&dir2, "s2", "unique-skill");

        let mut loader = SkillLoader::with_dirs(vec![dir1.clone(), dir2.clone()]);
        let count = loader.scan().unwrap();
        assert_eq!(count, 2);

        let _ = fs::remove_dir_all(&dir1);
        let _ = fs::remove_dir_all(&dir2);
    }

    #[test]
    fn test_first_dir_wins_on_duplicate() {
        let dir1 = PathBuf::from("/tmp/test_skills_dup1");
        let dir2 = PathBuf::from("/tmp/test_skills_dup2");
        let _ = fs::remove_dir_all(&dir1);
        let _ = fs::remove_dir_all(&dir2);

        fs::create_dir_all(dir1.join("dup")).unwrap();
        fs::write(
            dir1.join("dup/SKILL.md"),
            "---\nname: dup\ndescription: from dir1\n---\ndir1 content",
        )
        .unwrap();

        fs::create_dir_all(dir2.join("dup")).unwrap();
        fs::write(
            dir2.join("dup/SKILL.md"),
            "---\nname: dup\ndescription: from dir2\n---\ndir2 content",
        )
        .unwrap();

        let mut loader = SkillLoader::with_dirs(vec![dir1.clone(), dir2.clone()]);
        loader.scan().unwrap();

        let content = loader.load_content(loader.find_by_name("dup").unwrap()).unwrap();
        assert!(content.contains("dir1 content"));

        let _ = fs::remove_dir_all(&dir1);
        let _ = fs::remove_dir_all(&dir2);
    }

    #[test]
    fn test_skips_missing_dirs() {
        let loader = SkillLoader::with_dirs(vec![
            PathBuf::from("/tmp/nonexistent_dir_abc123"),
            PathBuf::from("/tmp/another_nonexistent_dir_xyz789"),
        ]);
        // Should not panic, just skip
        assert_eq!(loader.list().len(), 0);
    }
}
