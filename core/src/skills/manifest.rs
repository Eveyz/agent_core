use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A runnable script provided by a skill.
///
/// Declared in the skill's SKILL.md frontmatter under `scripts:`.
/// Each script becomes a registered tool named `skill.<skill_name>.<script_name>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptEntry {
    /// Short name — forms part of the tool name: `skill.<skill>.<name>`.
    pub name: String,

    /// LLM-facing description of what this script does.
    pub description: String,

    /// Path to the script relative to the skill directory (e.g. `scripts/deploy.sh`).
    /// Must be inside the skill directory — validated at load time.
    pub file: String,

    /// Per-script timeout in seconds (default: 60, capped at 600).
    #[serde(default = "default_script_timeout")]
    pub timeout_secs: u64,

    /// JSON Schema for the parameters this script accepts.
    /// Converted to CLI flags (`--key value`, `--flag`) at execution time.
    #[serde(default)]
    pub schema: Value,
}

fn default_script_timeout() -> u64 {
    60
}

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
/// scripts:
///   - name: deploy
///     description: "Deploy the app"
///     file: scripts/deploy.sh
///     timeout_secs: 120
///     schema: '{"type": "object", "properties": {"env": {"type": "string"}}}'
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

    /// Runnable scripts provided by this skill.
    /// Each becomes a tool named `skill.<skill_name>.<script_name>`.
    #[serde(default)]
    pub scripts: Vec<ScriptEntry>,
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
        let end = rest
            .find("---")
            .ok_or_else(|| anyhow::anyhow!("Missing closing --- in frontmatter"))?;

        let frontmatter = &rest[..end].trim();
        let _body = rest[end + 3..].trim();

        let mut manifest = parse_yaml_frontmatter(frontmatter)?;

        validate_identifier("skill name", &manifest.name)?;
        if manifest.description.trim().is_empty() {
            anyhow::bail!("skill description must not be empty");
        }

        let skill_dir = file_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("SKILL.md has no parent directory"))?;

        // Content path: if not specified, the SKILL.md itself is the content
        if manifest.content_path.as_os_str().is_empty() {
            let file_name = file_path
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("SKILL.md has no file name"))?;
            manifest.content_path =
                resolve_existing_path_within(skill_dir, Path::new(file_name), "content_path")?;
        } else {
            manifest.content_path =
                resolve_existing_path_within(skill_dir, &manifest.content_path, "content_path")?;
        }

        for script in &mut manifest.scripts {
            validate_identifier("script name", &script.name)?;
            if script.description.trim().is_empty() {
                anyhow::bail!("script '{}' description must not be empty", script.name);
            }
            if script.file.trim().is_empty() {
                anyhow::bail!("script '{}' file must not be empty", script.name);
            }
            if script.timeout_secs == 0 || script.timeout_secs > 600 {
                anyhow::bail!(
                    "script '{}' timeout_secs must be between 1 and 600",
                    script.name
                );
            }
            if !script.schema.is_object() {
                anyhow::bail!("script '{}' schema must be a JSON object", script.name);
            }
            let _ =
                resolve_existing_path_within(skill_dir, Path::new(&script.file), "script file")?;
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
        let mut extras = Vec::new();
        if !self.triggers.is_empty() {
            extras.push(format!("triggers: {}", self.triggers.join(", ")));
        }
        if !self.read_when.is_empty() {
            extras.push(format!("read_when: {}", self.read_when.join("; ")));
        }
        let extras_str = if extras.is_empty() {
            String::new()
        } else {
            format!(" [{}]", extras.join(" | "))
        };
        format!(
            "- **{}** (p{}): {}{}",
            self.name, self.priority, self.description, extras_str
        )
    }
}

fn validate_identifier(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("{label} must match [A-Za-z0-9_-]+");
    }
    Ok(())
}

/// Resolve an absolute or skill-relative path and prove that it stays inside
/// the canonical skill directory. This also rejects missing files and symlink
/// escapes before a skill becomes activatable.
pub(crate) fn resolve_existing_path_within(
    skill_dir: &Path,
    requested: &Path,
    label: &str,
) -> Result<PathBuf> {
    let canonical_root = std::fs::canonicalize(skill_dir)
        .with_context(|| format!("failed to resolve skill directory: {}", skill_dir.display()))?;
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        skill_dir.join(requested)
    };
    let canonical = std::fs::canonicalize(&candidate)
        .with_context(|| format!("{label} does not exist: {}", candidate.display()))?;
    if !canonical.starts_with(&canonical_root) {
        anyhow::bail!("{label} escapes skill directory: {}", requested.display());
    }
    if !canonical.is_file() {
        anyhow::bail!("{label} is not a file: {}", candidate.display());
    }
    Ok(canonical)
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
    let mut scripts: Vec<ScriptEntry> = Vec::new();

    // Track which list we're currently parsing
    enum ListMode {
        None,
        Triggers,
        Tags,
        ReadWhen,
        Requires,
        ProvidesTools,
        /// Parsing `scripts:` section — accumulating fields for one ScriptEntry.
        Scripts {
            current: ScriptEntry,
        },
    }
    let mut list_mode = ListMode::None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // ── Scripts section: nested key-value entries ──────────────────────
        // Unlike simple string lists, each script entry has sub-fields.
        // `- name: foo` starts a new entry; indented lines populate fields.
        if matches!(list_mode, ListMode::Scripts { .. }) {
            // Start of a new script entry: flush previous, begin next.
            if line.starts_with("- name:") {
                if let ListMode::Scripts { current } =
                    std::mem::replace(&mut list_mode, ListMode::None)
                {
                    if !current.name.is_empty() {
                        scripts.push(current);
                    }
                }
                let script_name = line
                    .strip_prefix("- name:")
                    .unwrap()
                    .trim()
                    .trim_matches('"')
                    .to_string();
                list_mode = ListMode::Scripts {
                    current: ScriptEntry {
                        name: script_name,
                        description: String::new(),
                        file: String::new(),
                        timeout_secs: default_script_timeout(),
                        schema: Value::Object(serde_json::Map::new()),
                    },
                };
                continue;
            }

            // Sub-field of the current script entry.
            if let ListMode::Scripts { ref mut current } = list_mode {
                if let Some(val) = line.strip_prefix("description:") {
                    current.description = val.trim().trim_matches('"').to_string();
                } else if let Some(val) = line.strip_prefix("file:") {
                    current.file = val.trim().trim_matches('"').to_string();
                } else if let Some(val) = line.strip_prefix("timeout_secs:") {
                    current.timeout_secs = val.trim().parse().unwrap_or(default_script_timeout());
                } else if let Some(val) = line.strip_prefix("schema:") {
                    let schema_str = val.trim();
                    // Support both quoted JSON string and inline YAML-as-JSON.
                    let json_str = schema_str.trim_matches('"').trim_matches('\'');
                    if let Ok(v) = serde_json::from_str(json_str) {
                        current.schema = v;
                    } else {
                        // Fallback: wrap as a simple JSON string for manual fix.
                        current.schema = Value::String(json_str.to_string());
                    }
                }
            }
            continue;
        }

        // ── Simple list items ──────────────────────────────────────────────
        if line.starts_with("- ") {
            let val = line.strip_prefix("- ").unwrap().trim();
            let val = val.trim_matches('"').trim_matches('\'');
            match list_mode {
                ListMode::Triggers => triggers.push(val.to_string()),
                ListMode::Tags => tags.push(val.to_string()),
                ListMode::ReadWhen => read_when.push(val.to_string()),
                ListMode::Requires => requires.push(val.to_string()),
                ListMode::ProvidesTools => provides_tools.push(val.to_string()),
                ListMode::None => {} // standalone list item, ignore
                ListMode::Scripts { .. } => unreachable!(),
            }
            continue;
        }

        // Detect list mode changes
        if line.starts_with("scripts:") {
            // Check inline list — if "scripts: []" just skip to next field.
            if let Some(inline) = line.strip_prefix("scripts:") {
                let trimmed = inline.trim();
                if trimmed == "[]" {
                    list_mode = ListMode::None;
                    continue;
                }
            }
            list_mode = ListMode::Scripts {
                current: ScriptEntry {
                    name: String::new(),
                    description: String::new(),
                    file: String::new(),
                    timeout_secs: default_script_timeout(),
                    schema: Value::Object(serde_json::Map::new()),
                },
            };
            continue;
        }

        if line.starts_with("triggers:") {
            list_mode = ListMode::Triggers;
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

    // Flush final script entry if one was being accumulated.
    if let ListMode::Scripts { current } = list_mode {
        if !current.name.is_empty() {
            scripts.push(current);
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
        scripts,
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
        assert!(
            manifest
                .read_when
                .contains(&"Editing Rust source files".to_string())
        );

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
            triggers: vec![
                "git".to_string(),
                "commit".to_string(),
                "branch".to_string(),
            ],
            tags: vec![],
            read_when: vec!["working with git".to_string()],
            requires: vec![],
            provides_tools: vec![],
            priority: 0,
            content_path: PathBuf::new(),
            scripts: vec![],
        };

        assert!(manifest.matches_trigger("帮我创建一个 git branch"));
        assert!(manifest.matches_trigger("how to git commit properly"));
        assert!(manifest.matches_trigger("working with git today"));
        assert!(!manifest.matches_trigger("how to write Python code"));

        let line = manifest.catalog_line();
        assert!(line.contains("git-workflow"));
        assert!(line.contains("triggers:"));
        assert!(line.contains("read_when:"));
        assert!(line.contains("working with git"));
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
            scripts: vec![],
        };

        // Name match should still work even without triggers
        assert!(manifest.matches_trigger("use docker-deploy skill"));
        assert!(!manifest.matches_trigger("write some code"));
    }

    #[test]
    fn test_parse_scripts_section() {
        let content = r#"---
name: deploy-skill
description: A skill with scripts
scripts:
  - name: deploy
    description: "Deploy the app"
    file: scripts/deploy.sh
    timeout_secs: 120
    schema: '{"type": "object", "properties": {"env": {"type": "string", "enum": ["staging", "production"]}, "dry_run": {"type": "boolean"}}, "required": ["env"]}'
  - name: health_check
    description: "Run health check"
    file: scripts/check.py
    timeout_secs: 30
    schema: '{"type": "object", "properties": {}}'
---
Some content"#;

        let dir = PathBuf::from("/tmp/test_skill_scripts");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(dir.join("scripts/deploy.sh"), "#!/bin/sh\n").unwrap();
        fs::write(dir.join("scripts/check.py"), "print('ok')\n").unwrap();
        fs::write(dir.join("SKILL.md"), content).unwrap();

        let manifest = SkillManifest::from_file(&dir.join("SKILL.md")).unwrap();
        assert_eq!(manifest.name, "deploy-skill");
        assert_eq!(manifest.scripts.len(), 2);

        let deploy = &manifest.scripts[0];
        assert_eq!(deploy.name, "deploy");
        assert_eq!(deploy.description, "Deploy the app");
        assert_eq!(deploy.file, "scripts/deploy.sh");
        assert_eq!(deploy.timeout_secs, 120);
        assert!(deploy.schema.get("properties").is_some());

        let hc = &manifest.scripts[1];
        assert_eq!(hc.name, "health_check");
        assert_eq!(hc.timeout_secs, 30);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_scripts_empty_list() {
        let content = r#"---
name: no-scripts
description: No scripts
scripts: []
---
content"#;

        let dir = PathBuf::from("/tmp/test_skill_no_scripts");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), content).unwrap();

        let manifest = SkillManifest::from_file(&dir.join("SKILL.md")).unwrap();
        assert!(manifest.scripts.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn relative_content_path_is_resolved_from_skill_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("references")).unwrap();
        fs::write(dir.path().join("references/main.md"), "Reference body").unwrap();
        fs::write(
            dir.path().join("SKILL.md"),
            "---\nname: external-content\ndescription: d\ncontent_path: references/main.md\n---\nIgnored\n",
        )
        .unwrap();
        let manifest = SkillManifest::from_file(&dir.path().join("SKILL.md")).unwrap();
        assert!(manifest.content_path.is_absolute());
        assert_eq!(
            SkillManifest::read_body(&manifest.content_path).unwrap(),
            "Reference body"
        );
    }

    #[test]
    fn content_and_script_paths_cannot_escape_skill_directory() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(root.path().join("outside.md"), "outside").unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: escape\ndescription: d\ncontent_path: ../outside.md\n---\nBody\n",
        )
        .unwrap();
        let error = SkillManifest::from_file(&skill.join("SKILL.md")).unwrap_err();
        assert!(error.to_string().contains("escapes skill directory"));
    }
}
