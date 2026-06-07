use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Full skill manifest — parsed from SKILL.md frontmatter.
///
/// Format:
/// ```markdown
/// ---
/// name: my-skill
/// description: What this skill does
/// version: "1.0"
/// triggers: [rust, refactor]
/// tags: [rust, patterns]
/// read_when: [Editing Rust files, User asks about refactoring]
/// requires: []
/// provides_tools: []
/// priority: 10
/// ---
/// # Skill content here...
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,

    /// Version string for tracking updates.
    #[serde(default = "default_version")]
    pub version: String,

    /// Keywords/phrases that auto-trigger this skill.
    #[serde(default)]
    pub triggers: Vec<String>,

    /// Tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Conditions under which this skill should be loaded.
    /// These are checked against the user's message and context.
    #[serde(default)]
    pub read_when: Vec<String>,

    /// Names of other skills this one depends on.
    #[serde(default)]
    pub requires: Vec<String>,

    /// Tool names this skill provides/registers.
    #[serde(default)]
    pub provides_tools: Vec<String>,

    /// Priority: higher = loaded first, shown higher in catalog.
    #[serde(default)]
    pub priority: u32,

    /// Path to the skill's content file (body after frontmatter).
    #[serde(default)]
    pub content_path: PathBuf,
}

fn default_version() -> String {
    "1.0".to_string()
}

impl SkillManifest {
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_markdown(&content, path)
    }

    fn from_markdown(content: &str, file_path: &std::path::Path) -> Result<Self> {
        // Split on "---" to extract frontmatter
        if !content.starts_with("---") {
            anyhow::bail!("Skill file must start with YAML frontmatter (---)");
        }

        let rest = &content[3..]; // skip first ---
        let end = rest.find("---").ok_or_else(|| {
            anyhow::anyhow!("Missing closing --- in frontmatter")
        })?;

        let frontmatter = &rest[..end].trim();
        let _body = rest[end + 3..].trim();

        let mut manifest = parse_yaml_frontmatter(frontmatter)?;

        // Content path: if not specified, the SKILL.md itself is the content
        if manifest.content_path.as_os_str().is_empty() {
            manifest.content_path = file_path.to_path_buf();
        }

        // If the manifest's content_path points to SKILL.md, read the body
        // Otherwise, the content_path points to a separate file
        if manifest.content_path == file_path {
            // Body is in the same file — store it directly (used by load_content)
        }

        Ok(manifest)
    }

    /// Check if this skill should auto-trigger for the given user message.
    pub fn matches_trigger(&self, user_message: &str) -> bool {
        let msg_lower = user_message.to_lowercase();

        // Check trigger keywords
        for trigger in &self.triggers {
            if msg_lower.contains(&trigger.to_lowercase()) {
                return true;
            }
        }

        // Check read_when conditions
        for condition in &self.read_when {
            if msg_lower.contains(&condition.to_lowercase()) {
                return true;
            }
        }

        // Check name match (user explicitly mentions the skill)
        if msg_lower.contains(&self.name.to_lowercase()) {
            return true;
        }

        false
    }

    /// Get the content body from a SKILL.md file (everything after the frontmatter).
    pub fn read_body(path: &std::path::Path) -> Result<String> {
        let content = std::fs::read_to_string(path)?;
        if !content.starts_with("---") {
            return Ok(content);
        }
        let rest = &content[3..];
        if let Some(end) = rest.find("---") {
            Ok(rest[end + 3..].trim().to_string())
        } else {
            Ok(content)
        }
    }

    /// Produce a one-line catalog entry for the agent's system prompt.
    pub fn catalog_line(&self) -> String {
        let triggers_str = if self.triggers.is_empty() {
            String::new()
        } else {
            format!(" [triggers: {}]", self.triggers.join(", "))
        };
        format!(
            "- **{}** (p{}): {}{}",
            self.name, self.priority, self.description, triggers_str
        )
    }
}

// ── YAML-like frontmatter parser (no serde_yaml dependency) ──────────

fn parse_yaml_frontmatter(content: &str) -> Result<SkillManifest> {
    let mut name = String::new();
    let mut description = String::new();
    let mut version = "1.0".to_string();
    let mut triggers = Vec::new();
    let mut tags = Vec::new();
    let mut read_when = Vec::new();
    let mut requires = Vec::new();
    let mut provides_tools = Vec::new();
    let mut priority: u32 = 0;
    let mut content_path = PathBuf::new();

    // Track which list we're currently parsing
    enum ListMode {
        None,
        Triggers,
        Tags,
        ReadWhen,
        Requires,
        ProvidesTools,
    }
    let mut list_mode = ListMode::None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // List items
        if line.starts_with("- ") {
            let val = line.strip_prefix("- ").unwrap().trim();
            // Strip quotes if present
            let val = val.trim_matches('"').trim_matches('\'');
            match list_mode {
                ListMode::Triggers => triggers.push(val.to_string()),
                ListMode::Tags => tags.push(val.to_string()),
                ListMode::ReadWhen => read_when.push(val.to_string()),
                ListMode::Requires => requires.push(val.to_string()),
                ListMode::ProvidesTools => provides_tools.push(val.to_string()),
                ListMode::None => {} // standalone list item, ignore
            }
            continue;
        }

        // Detect list mode changes
        if line.starts_with("triggers:") {
            list_mode = ListMode::Triggers;
            // Check inline list: "triggers: [a, b, c]"
            if let Some(inline) = line.strip_prefix("triggers:") {
                if let Some(arr) = parse_inline_list(inline.trim()) {
                    triggers = arr;
                    list_mode = ListMode::None;
                }
            }
            continue;
        }
        if line.starts_with("tags:") {
            list_mode = ListMode::Tags;
            if let Some(inline) = line.strip_prefix("tags:") {
                if let Some(arr) = parse_inline_list(inline.trim()) {
                    tags = arr;
                    list_mode = ListMode::None;
                }
            }
            continue;
        }
        if line.starts_with("read_when:") {
            list_mode = ListMode::ReadWhen;
            if let Some(inline) = line.strip_prefix("read_when:") {
                if let Some(arr) = parse_inline_list(inline.trim()) {
                    read_when = arr;
                    list_mode = ListMode::None;
                }
            }
            continue;
        }
        if line.starts_with("requires:") {
            list_mode = ListMode::Requires;
            if let Some(inline) = line.strip_prefix("requires:") {
                if let Some(arr) = parse_inline_list(inline.trim()) {
                    requires = arr;
                    list_mode = ListMode::None;
                }
            }
            continue;
        }
        if line.starts_with("provides_tools:") {
            list_mode = ListMode::ProvidesTools;
            if let Some(inline) = line.strip_prefix("provides_tools:") {
                if let Some(arr) = parse_inline_list(inline.trim()) {
                    provides_tools = arr;
                    list_mode = ListMode::None;
                }
            }
            continue;
        }

        // Scalar fields
        list_mode = ListMode::None;
        if let Some(val) = line.strip_prefix("name:") {
            name = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("description:") {
            description = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("version:") {
            version = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("priority:") {
            priority = val.trim().parse().unwrap_or(0);
        } else if let Some(val) = line.strip_prefix("content_path:") {
            content_path = PathBuf::from(val.trim().trim_matches('"'));
        } else if let Some(val) = line.strip_prefix("content:") {
            content_path = PathBuf::from(val.trim().trim_matches('"'));
        }
    }

    Ok(SkillManifest {
        name,
        description,
        version,
        triggers,
        tags,
        read_when,
        requires,
        provides_tools,
        priority,
        content_path,
    })
}

/// Parse inline list like `[a, b, c]` or `[a,b,c]`.
fn parse_inline_list(s: &str) -> Option<Vec<String>> {
    let s = s.trim();
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        let items: Vec<String> = inner
            .split(',')
            .map(|item| item.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|item| !item.is_empty())
            .collect();
        Some(items)
    } else {
        None
    }
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
version: "2.0"
triggers:
  - rust
  - ownership
  - borrow
tags:
  - rust
  - patterns
read_when:
  - Editing Rust source files
requires: []
provides_tools: []
priority: 10
---
This is the skill content"#;

        let dir = PathBuf::from("/tmp/test_skill_parse2");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), content).unwrap();

        let manifest = SkillManifest::from_file(&dir.join("SKILL.md")).unwrap();
        assert_eq!(manifest.name, "rust-patterns");
        assert_eq!(manifest.version, "2.0");
        assert_eq!(manifest.priority, 10);
        assert_eq!(manifest.triggers.len(), 3);
        assert_eq!(manifest.read_when.len(), 1);
        assert!(manifest.triggers.contains(&"rust".to_string()));
        assert!(manifest.read_when.contains(&"Editing Rust source files".to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_inline_list_parsing() {
        let content = r#"---
name: inline-test
description: Test inline arrays
triggers: [rust, refactor, "multi word"]
tags: [lang, tool]
priority: 5
---
content"#;

        let dir = PathBuf::from("/tmp/test_skill_inline");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), content).unwrap();

        let manifest = SkillManifest::from_file(&dir.join("SKILL.md")).unwrap();
        assert_eq!(manifest.triggers.len(), 3);
        assert!(manifest.triggers.contains(&"multi word".to_string()));
        assert_eq!(manifest.tags.len(), 2);
        assert_eq!(manifest.priority, 5);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_trigger_matching() {
        let manifest = SkillManifest {
            name: "git-workflow".to_string(),
            description: "Git workflow patterns".to_string(),
            version: "1.0".to_string(),
            triggers: vec!["git".to_string(), "commit".to_string(), "branch".to_string()],
            tags: vec![],
            read_when: vec!["working with git".to_string()],
            requires: vec![],
            provides_tools: vec![],
            priority: 0,
            content_path: PathBuf::new(),
        };

        assert!(manifest.matches_trigger("帮我创建一个 git branch"));
        assert!(manifest.matches_trigger("how to git commit properly"));
        assert!(manifest.matches_trigger("working with git today"));
        assert!(!manifest.matches_trigger("how to write Python code"));
    }

    #[test]
    fn test_trigger_matches_name() {
        let manifest = SkillManifest {
            name: "docker-deploy".to_string(),
            description: "Docker deployment".to_string(),
            version: "1.0".to_string(),
            triggers: vec![],
            tags: vec![],
            read_when: vec![],
            requires: vec![],
            provides_tools: vec![],
            priority: 0,
            content_path: PathBuf::new(),
        };

        // Name match should still work even without triggers
        assert!(manifest.matches_trigger("use docker-deploy skill"));
        assert!(!manifest.matches_trigger("write some code"));
    }
}
