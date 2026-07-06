pub mod manifest;

pub use manifest::{ScriptEntry, SkillManifest};

use anyhow::Result;
use serde_json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Backward-compatible alias.
pub type SkillLoader = SkillManager;

// ── Helpers ──────────────────────────────────────────────────────────

/// Resolve the home directory using standard cross-platform dirs library (matches ~/.agverse and User profile on Windows).
fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Recursively walk a directory tree and collect directories whose
/// immediate children contain SKILL.md files. These become search dirs
/// for the skill scanner.
///
/// This handles both layouts found under Agverse plugins:
///   plugin/skills/skill-name/SKILL.md   → adds `plugin/skills/`
///   external_plugins/skill-name/SKILL.md → adds `external_plugins/`
fn collect_skills_dirs(root: &Path, dirs: &mut Vec<PathBuf>) {
    if !root.exists() {
        return;
    }
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut has_skill_child = false;
    let mut subdirs: Vec<PathBuf> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.join("SKILL.md").exists() {
                // root/<name>/SKILL.md exists → root is a search dir
                has_skill_child = true;
            } else {
                subdirs.push(path);
            }
        }
    }

    if has_skill_child {
        dirs.push(root.to_path_buf());
    }

    for subdir in subdirs {
        collect_skills_dirs(&subdir, dirs);
    }
}

/// Manages skill loading, auto-triggering, and lifecycle.
///
/// Skills are SKILL.md files in search directories. They can be:
/// - Auto-triggered: when user message matches triggers/read_when conditions
/// - Manually loaded: via skill_load tool
/// - Deactivated: via skill_deactivate tool
/// - Hot-reloaded: via skill_reload tool (rescans directories)
pub struct SkillManager {
    search_dirs: Vec<PathBuf>,
    manifests: Vec<LoadedSkill>,
    /// Currently active skill names (loaded into context).
    active_skills: HashSet<String>,
}

pub struct LoadedSkill {
    pub manifest: SkillManifest,
    pub source_dir: PathBuf,
}

impl SkillManager {
    pub fn new(search_dir: PathBuf) -> Self {
        Self {
            search_dirs: vec![search_dir],
            manifests: Vec::new(),
            active_skills: HashSet::new(),
        }
    }

    pub fn with_dirs(dirs: Vec<PathBuf>) -> Self {
        Self {
            search_dirs: dirs,
            manifests: Vec::new(),
            active_skills: HashSet::new(),
        }
    }

    pub fn with_defaults() -> Self {
        let home = home_dir();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let mut dirs = vec![
            // Standard agent skill dirs
            cwd.join(".agent").join("skills"),
            cwd.join(".claude").join("skills"),
            cwd.join("skills"),
            home.join(".agent").join("skills"),
            home.join(".agents").join("skills"),
            home.join(".claude").join("skills"),
            // agverse
            crate::paths::get_skills_dir(),
        ];

        // Collect all skills/ directories under agverse plugins
        let plugins_root = crate::paths::get_agverse_dir().join("plugins");
        if plugins_root.exists() {
            collect_skills_dirs(&plugins_root, &mut dirs);
        }

        // Built-in skills shipped with the app (env var or auto-detect)
        if let Ok(builtin) = std::env::var("AGVERSE_BUILTIN_SKILLS") {
            dirs.push(PathBuf::from(builtin));
        } else if let Ok(app_data) = std::env::var("AGVERSE_APP_RESOURCES") {
            dirs.push(PathBuf::from(app_data).join("builtin-skills"));
        }

        Self {
            search_dirs: dirs,
            manifests: Vec::new(),
            active_skills: HashSet::new(),
        }
    }

    pub fn add_search_dir(&mut self, dir: PathBuf) {
        self.search_dirs.push(dir);
    }

    // ── Scanning ────────────────────────────────────────────────────

    /// Scan all search directories for SKILL.md files.
    /// Deduplicates by skill name (first directory wins).
    pub fn scan(&mut self) -> Result<usize> {
        self.manifests.clear();
        let mut seen_names = HashSet::new();

        for dir in &self.search_dirs {
            if !dir.exists() {
                continue;
            }

            let entries = match std::fs::read_dir(dir) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let path = entry.path();

                if !path.is_dir() {
                    continue;
                }

                let manifest_path = path.join("SKILL.md");
                if manifest_path.exists()
                    && let Ok(manifest) = SkillManifest::from_file(&manifest_path)
                {
                    if seen_names.insert(manifest.name.clone()) {
                        self.manifests.push(LoadedSkill {
                            manifest,
                            source_dir: path.clone(),
                        });
                    }
                }
            }
        }

        // Sort by priority (descending)
        self.manifests
            .sort_by(|a, b| b.manifest.priority.cmp(&a.manifest.priority));

        Ok(self.manifests.len())
    }

    // ── Auto-trigger ────────────────────────────────────────────────

    /// Check user message against all skill triggers.
    /// Returns list of skill names that should be auto-loaded.
    /// Does NOT modify state — caller should call `activate()` for matched skills.
    pub fn check_triggers(&self, user_message: &str) -> Vec<&SkillManifest> {
        let mut matched: Vec<&SkillManifest> = Vec::new();

        for skill in &self.manifests {
            // Skip already active skills
            if self.active_skills.contains(&skill.manifest.name) {
                continue;
            }
            if skill.manifest.matches_trigger(user_message) {
                matched.push(&skill.manifest);
            }
        }

        // Sort by priority
        matched.sort_by(|a, b| b.priority.cmp(&a.priority));
        matched
    }

    /// Activate a skill (mark as loaded into context).
    pub fn activate(&mut self, name: &str) -> bool {
        if self.find_by_name(name).is_some() {
            self.active_skills.insert(name.to_string());
            true
        } else {
            false
        }
    }

    /// Deactivate a skill (remove from context).
    pub fn deactivate(&mut self, name: &str) -> bool {
        self.active_skills.remove(name)
    }

    /// Check if a skill is currently active.
    pub fn is_active(&self, name: &str) -> bool {
        self.active_skills.contains(name)
    }

    /// Get names of all currently active skills.
    pub fn active_skill_names(&self) -> Vec<&str> {
        self.active_skills.iter().map(|s| s.as_str()).collect()
    }

    /// Deactivate all skills.
    pub fn deactivate_all(&mut self) {
        self.active_skills.clear();
    }

    // ── Content loading ──────────────────────────────────────────────

    /// Load the full content of a skill (body after frontmatter).
    pub fn load_content(&self, manifest: &SkillManifest) -> Result<String> {
        // Try reading body from the SKILL.md file
        if manifest.content_path.as_os_str().is_empty() {
            anyhow::bail!("No content path for skill '{}'", manifest.name);
        }
        let body = SkillManifest::read_body(&manifest.content_path)?;

        // Find the source_dir for this manifest in self.manifests to get the skill_dir
        let skill_dir = self.manifests
            .iter()
            .find(|s| s.manifest.name == manifest.name)
            .map(|s| &s.source_dir);

        if let Some(dir) = skill_dir {
            let dir_str = dir.to_string_lossy();
            let re = regex::Regex::new(r"(?m)(^|[^a-zA-Z0-9_/\.-])(?:\./)?scripts/").unwrap();
            let rewritten = re.replace_all(&body, |caps: &regex::Captures| {
                format!("{}{}/scripts/", &caps[1], dir_str)
            });
            Ok(rewritten.into_owned())
        } else {
            Ok(body)
        }
    }

    /// Build context string for all active skills (for Segment 6).
    pub fn build_active_context(&self) -> String {
        if self.active_skills.is_empty() {
            return String::new();
        }

        let mut parts: Vec<String> = Vec::new();
        for skill in &self.manifests {
            if self.active_skills.contains(&skill.manifest.name) {
                if let Ok(content) = self.load_content(&skill.manifest) {
                    parts.push(format!(
                        "## Skill: {} (v{})\nSkill directory: {}\n{}\n",
                        skill.manifest.name,
                        skill.manifest.version,
                        skill.source_dir.display(),
                        content,
                    ));
                }
            }
        }

        if parts.is_empty() {
            return String::new();
        }

        let mut result = format!(
            "The following skills are ACTIVE and loaded into your context. \
             Use their knowledge to guide your responses:\n\n{}",
            parts.join("\n")
        );

        // Append script catalog for all active skills.
        let active_scripts = self.get_active_scripts();
        if !active_scripts.is_empty() {
            result.push_str(&Self::scripts_context(&active_scripts));
        }

        result
    }

    /// Build a concise skill catalog for the system prompt.
    /// Lists ALL available skills (not just active ones) so the agent
    /// knows what's available and can use skill_load to pull one in.
    pub fn build_catalog(&self) -> String {
        if self.manifests.is_empty() {
            return String::new();
        }

        let mut lines: Vec<String> = Vec::new();
        lines.push("Available skills (to use a skill, call the `skill_load` tool with its name to load and activate it. DO NOT try to read the SKILL.md file or files inside the skill directory directly using file tools):".to_string());

        for skill in &self.manifests {
            let active_marker = if self.active_skills.contains(&skill.manifest.name) {
                " [ACTIVE]"
            } else {
                ""
            };
            lines.push(format!(
                "{}{}",
                skill.manifest.catalog_line(),
                active_marker
            ));
        }

        lines.join("\n")
    }

    // ── Lookup ──────────────────────────────────────────────────────

    /// Get the directory containing a skill's files (SKILL.md, scripts/, etc.).
    pub fn source_dir_of(&self, name: &str) -> Option<&Path> {
        self.manifests
            .iter()
            .find(|s| s.manifest.name == name)
            .map(|s| s.source_dir.as_path())
    }

    // ── Scripts ─────────────────────────────────────────────────────

    /// Discover all scripts for a skill, combining manifest-declared entries
    /// with auto-discovered files in the `scripts/` directory.
    ///
    /// Manifest entries take priority — if a script with the same name appears
    /// in both, the manifest wins. Auto-discovery only fills in scripts not
    /// already declared.
    pub fn discover_scripts(&self, name: &str) -> Vec<ScriptEntry> {
        let mut scripts: Vec<ScriptEntry> = Vec::new();
        let mut seen = HashSet::new();

        // 1. Manifest-declared scripts (highest priority)
        if let Some(skill) = self.manifests.iter().find(|s| s.manifest.name == name) {
            for entry in &skill.manifest.scripts {
                if !seen.insert(entry.name.clone()) {
                    continue;
                }
                scripts.push(entry.clone());
            }
        }

        // 2. Auto-discover scripts in <skill_dir>/scripts/
        if let Some(source_dir) = self.source_dir_of(name) {
            let scripts_dir = source_dir.join("scripts");
            if scripts_dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&scripts_dir) {
                    let script_extensions: &[&str] =
                        &["sh", "bash", "py", "js", "rb", "ts", "go", "rs"];
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if !path.is_file() {
                            continue;
                        }
                        let script_name = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown");
                        // Sanitize: replace dots/hyphens with underscores for tool name.
                        let safe_name = script_name.replace(['.', '-'], "_");
                        if seen.contains(&safe_name) {
                            continue;
                        }
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if !script_extensions.contains(&ext) {
                            continue;
                        }
                        seen.insert(safe_name.clone());
                        scripts.push(ScriptEntry {
                            name: safe_name.clone(),
                            description: format!(
                                "Run `{}` script from the '{name}' skill",
                                path.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(script_name)
                            ),
                            file: path
                                .strip_prefix(source_dir)
                                .ok()
                                .and_then(|p| p.to_str().map(|s| s.to_string()))
                                .unwrap_or_else(|| {
                                    format!("scripts/{}", path.file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or(script_name))
                                }),
                            timeout_secs: 60,
                            schema: serde_json::Value::Object(serde_json::Map::new()),
                        });
                    }
                }
            }
        }

        scripts
    }

    /// Get script entries for a specific skill (manifest + auto-discovered).
    pub fn get_scripts(&self, name: &str) -> Vec<ScriptEntry> {
        self.discover_scripts(name)
    }

    /// Get all active skill scripts as (skill_name, ScriptEntry) pairs.
    pub fn get_active_scripts(&self) -> Vec<(String, ScriptEntry)> {
        let mut result = Vec::new();
        for name in &self.active_skills {
            for script in self.discover_scripts(name) {
                result.push((name.clone(), script));
            }
        }
        result
    }

    /// Format a single script entry as a catalog line for the context prompt.
    pub fn script_line(skill_name: &str, script: &ScriptEntry) -> String {
        format!(
            "- **skill.{}.{}**: {} [timeout: {}s]",
            skill_name, script.name, script.description, script.timeout_secs
        )
    }

    /// Format a list of scripts for injection into the context prompt.
    pub fn scripts_context(scripts: &[(String, ScriptEntry)]) -> String {
        if scripts.is_empty() {
            return String::new();
        }
        let mut lines = vec!["\n### Active Scripts\n".to_string()];
        for (skill_name, script) in scripts {
            lines.push(Self::script_line(skill_name, script));
        }
        lines.join("\n")
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
        self.manifests
            .iter()
            .find(|s| s.manifest.name == name)
            .map(|s| &s.manifest)
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

    // ── Backward compat ─────────────────────────────────────────────

    pub fn load_skill_context(&self, name: &str) -> Result<Option<String>> {
        if let Some(skill) = self.manifests.iter().find(|s| s.manifest.name == name) {
            let content = self.load_content(&skill.manifest)?;
            let mut result = format!(
                "== Skill: {} (v{}) ==\nSkill directory: {}\n{}\n== End Skill: {} ==\n",
                skill.manifest.name,
                skill.manifest.version,
                skill.source_dir.display(),
                content,
                skill.manifest.name
            );
            // Append available scripts.
            let scripts = self.discover_scripts(&skill.manifest.name);
            if !scripts.is_empty() {
                result.push_str("\n### Available Scripts\n");
                for script in &scripts {
                    result.push_str(&format!(
                        "- skill.{}.{}: {} (scripts/{})\n",
                        skill.manifest.name, script.name, script.description, script.file
                    ));
                }
            }
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    pub fn search_dirs(&self) -> &[PathBuf] {
        &self.search_dirs
    }

    pub fn count(&self) -> usize {
        self.manifests.len()
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
version: "1.0"
triggers:
  - test
  - {0}
---
Test content for {0}"#,
                display_name
            ),
        )
        .unwrap();
    }

    fn write_skill_full(
        dir: &Path,
        name: &str,
        description: &str,
        triggers: &[&str],
        read_when: &[&str],
        priority: u32,
    ) {
        fs::create_dir_all(dir.join(name)).unwrap();
        let triggers_yaml: String = triggers
            .iter()
            .map(|t| format!("  - {}", t))
            .collect::<Vec<_>>()
            .join("\n");
        let read_when_yaml: String = read_when
            .iter()
            .map(|t| format!("  - {}", t))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(
            dir.join(name).join("SKILL.md"),
            format!(
                r#"---
name: {}
description: {}
version: "1.0"
triggers:
{}
read_when:
{}
priority: {}
---
Skill body for {}"#,
                name, description, triggers_yaml, read_when_yaml, priority, name
            ),
        )
        .unwrap();
    }

    #[test]
    fn test_single_dir_scan() {
        let dir = PathBuf::from("/tmp/test_skills_single2");
        let _ = fs::remove_dir_all(&dir);
        write_skill(&dir, "skill_a", "skill-a");

        let mut mgr = SkillManager::new(dir.clone());
        let count = mgr.scan().unwrap();
        assert_eq!(count, 1);
        assert_eq!(mgr.list()[0].name, "skill-a");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_multi_dir_dedup() {
        let dir1 = PathBuf::from("/tmp/test_skills_multi1b");
        let dir2 = PathBuf::from("/tmp/test_skills_multi2b");
        let _ = fs::remove_dir_all(&dir1);
        let _ = fs::remove_dir_all(&dir2);

        write_skill(&dir1, "s1", "shared-skill");
        write_skill(&dir2, "s1", "shared-skill");
        write_skill(&dir2, "s2", "unique-skill");

        let mut mgr = SkillManager::with_dirs(vec![dir1.clone(), dir2.clone()]);
        let count = mgr.scan().unwrap();
        assert_eq!(count, 2);

        let _ = fs::remove_dir_all(&dir1);
        let _ = fs::remove_dir_all(&dir2);
    }

    #[test]
    fn test_first_dir_wins_on_duplicate() {
        let dir1 = PathBuf::from("/tmp/test_skills_dup1b");
        let dir2 = PathBuf::from("/tmp/test_skills_dup2b");
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

        let mut mgr = SkillManager::with_dirs(vec![dir1.clone(), dir2.clone()]);
        mgr.scan().unwrap();

        let content = mgr.load_content(mgr.find_by_name("dup").unwrap()).unwrap();
        assert!(content.contains("dir1 content"));

        let _ = fs::remove_dir_all(&dir1);
        let _ = fs::remove_dir_all(&dir2);
    }

    #[test]
    fn test_skips_missing_dirs() {
        let mgr = SkillManager::with_dirs(vec![
            PathBuf::from("/tmp/nonexistent_dir_abc456"),
            PathBuf::from("/tmp/another_nonexistent_dir_xyz789"),
        ]);
        assert_eq!(mgr.list().len(), 0);
    }

    #[test]
    fn test_auto_trigger() {
        let dir = PathBuf::from("/tmp/test_skills_trigger");
        let _ = fs::remove_dir_all(&dir);
        write_skill_full(
            &dir,
            "rust-refactor",
            "Rust refactoring",
            &["rust", "refactor"],
            &["Editing Rust source"],
            10,
        );
        write_skill_full(
            &dir,
            "python-testing",
            "Python testing",
            &["python", "pytest"],
            &[],
            5,
        );

        let mut mgr = SkillManager::new(dir.clone());
        mgr.scan().unwrap();

        let matched = mgr.check_triggers("帮我重构 Rust 代码");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "rust-refactor");

        let matched2 = mgr.check_triggers("写一个 pytest 测试");
        assert_eq!(matched2.len(), 1);
        assert_eq!(matched2[0].name, "python-testing");

        let matched3 = mgr.check_triggers("hello world");
        assert_eq!(matched3.len(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_activate_and_deactivate() {
        let dir = PathBuf::from("/tmp/test_skills_activate");
        let _ = fs::remove_dir_all(&dir);
        write_skill(&dir, "s1", "skill-one");

        let mut mgr = SkillManager::new(dir.clone());
        mgr.scan().unwrap();

        assert!(!mgr.is_active("skill-one"));
        assert!(mgr.activate("skill-one"));
        assert!(mgr.is_active("skill-one"));

        // Activating again is a no-op but returns true
        assert!(mgr.activate("skill-one"));

        assert!(mgr.deactivate("skill-one"));
        assert!(!mgr.is_active("skill-one"));

        // Deactivating non-existent returns false
        assert!(!mgr.deactivate("nope"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_skip_already_active_in_trigger_check() {
        let dir = PathBuf::from("/tmp/test_skills_skip");
        let _ = fs::remove_dir_all(&dir);
        write_skill_full(&dir, "rust", "Rust", &["rust"], &[], 10);

        let mut mgr = SkillManager::new(dir.clone());
        mgr.scan().unwrap();

        // First check: should match
        let matched = mgr.check_triggers("rust code");
        assert_eq!(matched.len(), 1);

        // Activate it
        mgr.activate("rust");

        // Second check: should skip (already active)
        let matched2 = mgr.check_triggers("rust code");
        assert_eq!(matched2.len(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_catalog() {
        let dir = PathBuf::from("/tmp/test_skills_catalog");
        let _ = fs::remove_dir_all(&dir);
        write_skill_full(&dir, "high", "High priority", &[], &[], 100);
        write_skill_full(&dir, "low", "Low priority", &[], &[], 1);

        let mut mgr = SkillManager::new(dir.clone());
        mgr.scan().unwrap();

        let catalog = mgr.build_catalog();
        assert!(catalog.contains("high"));
        assert!(catalog.contains("low"));
        // High priority should come first
        let high_pos = catalog.find("high").unwrap();
        let low_pos = catalog.find("low").unwrap();
        assert!(high_pos < low_pos);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_active_context() {
        let dir = PathBuf::from("/tmp/test_skills_ctx");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("my-skill")).unwrap();
        fs::write(
            dir.join("my-skill/SKILL.md"),
            "---\nname: my-skill\ndescription: My skill\n---\nThis is the skill body.",
        )
        .unwrap();

        let mut mgr = SkillManager::new(dir.clone());
        mgr.scan().unwrap();
        mgr.activate("my-skill");

        let ctx = mgr.build_active_context();
        assert!(ctx.contains("my-skill"));
        assert!(ctx.contains("This is the skill body"));
        assert!(ctx.contains("ACTIVE"));
        assert!(ctx.contains("Skill directory:"));
        // Must expose the skill's own dir, not the search dir.
        assert!(ctx.contains(&dir.join("my-skill").display().to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_content_rewrites_paths() {
        let dir = PathBuf::from("/tmp/test_skills_rewrite");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("rewrite-skill")).unwrap();

        let body = "\
---
name: rewrite-skill
description: Skill for testing relative path rewriting
---
Run scripts/make.sh directly.
Or run ./scripts/make.sh like this.
But do not rewrite /absolute/path/to/scripts/make.sh.
Do not rewrite javascriptscripts/make.sh.
Do not rewrite some_scripts/make.sh.
Do not rewrite ../scripts/make.sh.
Do not rewrite my-scripts/make.sh.
Also check markdown: [script](scripts/make.sh) and [script2](./scripts/make.sh).
";

        fs::write(dir.join("rewrite-skill/SKILL.md"), body).unwrap();

        let mut mgr = SkillManager::new(dir.clone());
        mgr.scan().unwrap();

        let skill_manifest = mgr.find_by_name("rewrite-skill").unwrap();
        let content = mgr.load_content(skill_manifest).unwrap();

        let expected_dir = dir.join("rewrite-skill").to_string_lossy().into_owned();

        // Should rewrite relative paths
        assert!(content.contains(&format!("Run {}/scripts/make.sh directly.", expected_dir)));
        assert!(content.contains(&format!("Or run {}/scripts/make.sh like this.", expected_dir)));
        assert!(content.contains(&format!("Also check markdown: [script]({}/scripts/make.sh) and [script2]({}/scripts/make.sh).", expected_dir, expected_dir)));

        // Should NOT rewrite absolute or invalid matches
        assert!(content.contains("But do not rewrite /absolute/path/to/scripts/make.sh."));
        assert!(content.contains("Do not rewrite javascriptscripts/make.sh."));
        assert!(content.contains("Do not rewrite some_scripts/make.sh."));
        assert!(content.contains("Do not rewrite ../scripts/make.sh."));
        assert!(content.contains("Do not rewrite my-scripts/make.sh."));

        let _ = fs::remove_dir_all(&dir);
    }
}
