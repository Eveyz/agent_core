use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub content_path: PathBuf,
}

impl SkillManifest {
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;

        if content.contains("---") {
            Self::from_markdown(&content, path)
        } else {
            let manifest: SkillManifest = serde_json::from_str(&content)?;
            Ok(manifest)
        }
    }

    fn from_markdown(content: &str, file_path: &std::path::Path) -> Result<Self> {
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() < 3 {
            anyhow::bail!("Invalid skill markdown format: missing frontmatter");
        }

        let frontmatter = parts[1].trim();
        let manifest: SkillManifest = serde_yaml_like(frontmatter)?;

        let content_path = if manifest.content_path.as_os_str().is_empty() {
            file_path.to_path_buf()
        } else {
            manifest.content_path
        };

        Ok(SkillManifest {
            content_path,
            ..manifest
        })
    }
}

fn serde_yaml_like(content: &str) -> Result<SkillManifest> {
    let mut name = String::new();
    let mut description = String::new();
    let mut triggers = Vec::new();
    let mut tags = Vec::new();
    let mut content_path = PathBuf::new();
    let mut in_triggers = false;
    let mut in_tags = false;

    for line in content.lines() {
        let line = line.trim();

        if line.starts_with("name:") {
            name = line.strip_prefix("name:").unwrap().trim().to_string();
            in_triggers = false;
            in_tags = false;
        } else if line.starts_with("description:") {
            description = line
                .strip_prefix("description:")
                .unwrap()
                .trim()
                .to_string();
            in_triggers = false;
            in_tags = false;
        } else if line.starts_with("content_path:") || line.starts_with("content:") {
            let val = line.split_once(':').map(|(_, v)| v.trim()).unwrap_or("");
            content_path = PathBuf::from(val);
            in_triggers = false;
            in_tags = false;
        } else if line == "triggers:" {
            in_triggers = true;
            in_tags = false;
        } else if line == "tags:" {
            in_triggers = false;
            in_tags = true;
        } else if line.starts_with("- ") {
            let val = line.strip_prefix("- ").unwrap().trim().to_string();
            if in_triggers {
                triggers.push(val);
            } else if in_tags {
                tags.push(val);
            }
        }
    }

    Ok(SkillManifest {
        name,
        description,
        triggers,
        tags,
        content_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_markdown_frontmatter() {
        let content = r#"---
name: rust-patterns
description: Idiomatic Rust patterns
triggers:
  - rust
  - ownership
  - borrow
tags:
  - rust
  - patterns
---
This is the skill content"#;

        let dir = PathBuf::from("/tmp/test_skill_parse");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), content).unwrap();

        let manifest = SkillManifest::from_file(&dir.join("SKILL.md")).unwrap();
        assert_eq!(manifest.name, "rust-patterns");
        assert_eq!(manifest.triggers.len(), 3);
        assert!(manifest.triggers.contains(&"rust".to_string()));

        let _ = fs::remove_dir_all(&dir);
    }
}
