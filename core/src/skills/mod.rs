pub mod manifest;

pub use manifest::{ScriptEntry, SkillManifest};

use anyhow::Result;
use parking_lot::Mutex;
use regex::Regex;
use serde_json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// Backward-compatible alias.
pub type SkillLoader = SkillManager;

/// Parse `@skill:<name>` mentions from free text (mid-sentence / punctuation-safe).
pub fn parse_skill_mentions(text: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re =
        RE.get_or_init(|| Regex::new(r"@skill:([A-Za-z0-9_-]+)").expect("skill mention regex"));
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for cap in re.captures_iter(text) {
        let name = cap[1].to_string();
        if seen.insert(name.clone()) {
            names.push(name);
        }
    }
    names
}

const ASSET_DIRS: &[&str] = &["templates", "references", "assets", "subskills"];
const ASSET_MAX_FILES: usize = 40;
const ASSET_MAX_DEPTH: usize = 2;
const RESOURCE_MAX_FILES: usize = 1_000;
const RESOURCE_MAX_DEPTH: usize = 16;

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

fn default_search_dirs(include_process_workspace: bool) -> Vec<PathBuf> {
    let home = home_dir();
    let mut dirs = Vec::new();
    if include_process_workspace {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        dirs.extend([
            cwd.join(".agent").join("skills"),
            cwd.join(".agents").join("skills"),
            cwd.join(".claude").join("skills"),
            cwd.join(".codex").join("skills"),
            cwd.join("skills"),
        ]);
    }
    dirs.extend([
        home.join(".agent").join("skills"),
        home.join(".agents").join("skills"),
        home.join(".claude").join("skills"),
        crate::paths::get_skills_dir(),
    ]);

    let plugins_root = crate::paths::get_agverse_dir().join("plugins");
    if plugins_root.exists() {
        collect_skills_dirs(&plugins_root, &mut dirs);
    }
    if let Ok(builtin) = std::env::var("AGVERSE_BUILTIN_SKILLS") {
        dirs.push(PathBuf::from(builtin));
    } else if let Ok(app_data) = std::env::var("AGVERSE_APP_RESOURCES") {
        dirs.push(PathBuf::from(app_data).join("builtin-skills"));
    }
    dirs
}

fn direct_skill_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("SKILL.md").is_file())
        .collect();
    dirs.sort();
    dirs
}

/// Manages skill loading, auto-triggering, and lifecycle.
///
/// Skills are SKILL.md files in search directories. They can be:
/// - Auto-triggered: when user message matches triggers/read_when conditions
/// - Manually loaded: via skill_load tool
/// - Deactivated: via skill_deactivate tool
/// - Hot-reloaded: via skill_reload tool (rescans directories)
#[derive(Clone)]
pub struct SkillManager {
    search_dirs: Vec<PathBuf>,
    manifests: Vec<LoadedSkill>,
    /// Active skill names keyed by session scope (`""` = anonymous / CLI).
    /// Keeps chat sessions from leaking loaded skills into each other.
    active_by_scope: HashMap<String, HashSet<String>>,
    /// Skills explicitly requested by the user/agent. `active_by_scope` is the
    /// dependency closure derived from these roots.
    requested_by_scope: HashMap<String, HashSet<String>>,
    /// One-shot notes (e.g. unknown `@skill:`) drained into Segment 6 each turn.
    pending_notes_by_scope: HashMap<String, Vec<String>>,
    diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDiagnostic {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone)]
pub struct LoadedSkill {
    pub manifest: SkillManifest,
    pub source_dir: PathBuf,
}

impl SkillManager {
    pub fn new(search_dir: PathBuf) -> Self {
        Self {
            search_dirs: vec![search_dir],
            manifests: Vec::new(),
            active_by_scope: HashMap::new(),
            requested_by_scope: HashMap::new(),
            pending_notes_by_scope: HashMap::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn with_dirs(dirs: Vec<PathBuf>) -> Self {
        Self {
            search_dirs: dirs,
            manifests: Vec::new(),
            active_by_scope: HashMap::new(),
            requested_by_scope: HashMap::new(),
            pending_notes_by_scope: HashMap::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn with_defaults() -> Self {
        Self::with_dirs(default_search_dirs(true))
    }

    /// Global/plugin/builtin roots only. Explicit Runs add exactly their own
    /// workspace roots so process-startup project skills cannot leak across.
    pub fn with_global_defaults() -> Self {
        Self::with_dirs(default_search_dirs(false))
    }

    pub fn add_search_dir(&mut self, dir: PathBuf) {
        if !self.search_dirs.contains(&dir) {
            self.search_dirs.push(dir);
        }
    }

    /// Add the standard project-local skill roots for an explicit Run
    /// workspace. Returns true when the search path changed.
    pub fn add_workspace_root(&mut self, root: &Path) -> bool {
        let before = self.search_dirs.len();
        let dirs = [
            root.join(".agent").join("skills"),
            root.join(".agents").join("skills"),
            root.join(".claude").join("skills"),
            root.join(".codex").join("skills"),
            root.join("skills"),
        ];
        // Project-local definitions override global/plugin definitions.
        for dir in dirs.into_iter().rev() {
            if !self.search_dirs.contains(&dir) {
                self.search_dirs.insert(0, dir);
            }
        }
        self.search_dirs.len() != before
    }

    // ── Scanning ────────────────────────────────────────────────────

    /// Scan all search directories for SKILL.md files.
    /// Deduplicates by skill name (first directory wins).
    pub fn scan(&mut self) -> Result<usize> {
        self.manifests.clear();
        self.diagnostics.clear();
        let mut seen_names = HashSet::new();

        for dir in &self.search_dirs {
            if !dir.exists() {
                continue;
            }

            let mut skill_dirs = direct_skill_dirs(dir);
            let mut index = 0;
            while index < skill_dirs.len() {
                let path = skill_dirs[index].clone();
                index += 1;
                let manifest_path = path.join("SKILL.md");
                match SkillManifest::from_file(&manifest_path) {
                    Ok(manifest) if seen_names.insert(manifest.name.clone()) => {
                        self.manifests.push(LoadedSkill {
                            manifest,
                            source_dir: std::fs::canonicalize(&path).unwrap_or(path.clone()),
                        });
                    }
                    Ok(manifest) => self.diagnostics.push(SkillDiagnostic {
                        path: manifest_path,
                        message: format!(
                            "duplicate skill '{}' shadowed by an earlier root",
                            manifest.name
                        ),
                    }),
                    Err(error) => self.diagnostics.push(SkillDiagnostic {
                        path: manifest_path,
                        message: error.to_string(),
                    }),
                }

                // Explicit physical subskills are discoverable without making
                // every references/assets directory recursively executable.
                let nested = path.join("subskills");
                for child in direct_skill_dirs(&nested) {
                    if !skill_dirs.contains(&child) {
                        skill_dirs.push(child);
                    }
                }
            }
        }

        // Sort by priority (descending)
        self.manifests
            .sort_by(|a, b| b.manifest.priority.cmp(&a.manifest.priority));

        Ok(self.manifests.len())
    }

    pub fn diagnostics(&self) -> &[SkillDiagnostic] {
        &self.diagnostics
    }

    // ── Auto-trigger ────────────────────────────────────────────────

    fn scope_key(session_id: Option<&str>) -> String {
        session_id.unwrap_or("").to_string()
    }

    fn active_set(&self, session_id: Option<&str>) -> Option<&HashSet<String>> {
        self.active_by_scope.get(&Self::scope_key(session_id))
    }

    fn active_set_mut(&mut self, session_id: Option<&str>) -> &mut HashSet<String> {
        let key = Self::scope_key(session_id);
        self.active_by_scope.entry(key).or_default()
    }

    /// Check user message against all skill triggers.
    /// Returns list of skill names that should be auto-loaded.
    /// Does NOT modify state — caller should call `activate()` for matched skills.
    pub fn check_triggers(&self, user_message: &str) -> Vec<&SkillManifest> {
        self.check_triggers_for(None, user_message)
    }

    /// Session-scoped trigger check (skips skills already active in that session).
    pub fn check_triggers_for(
        &self,
        session_id: Option<&str>,
        user_message: &str,
    ) -> Vec<&SkillManifest> {
        let active = self.active_set(session_id);
        let mut matched: Vec<&SkillManifest> = Vec::new();

        for skill in &self.manifests {
            if active.is_some_and(|a| a.contains(&skill.manifest.name)) {
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

    /// Activate a skill in the anonymous/default scope (CLI / tests).
    pub fn activate(&mut self, name: &str) -> bool {
        self.activate_for(None, name)
    }

    /// Activate a skill for a specific session scope.
    pub fn activate_for(&mut self, session_id: Option<&str>, name: &str) -> bool {
        match self.activation_plan(name) {
            Ok(plan) => {
                self.requested_by_scope
                    .entry(Self::scope_key(session_id))
                    .or_default()
                    .insert(name.to_string());
                self.active_set_mut(session_id).extend(plan);
                true
            }
            Err(error) => {
                self.push_note(session_id, error.to_string());
                false
            }
        }
    }

    /// Resolve `requires` into dependency-first activation order and reject
    /// missing dependencies or cycles before mutating session state.
    pub fn activation_plan(&self, name: &str) -> Result<Vec<String>> {
        fn visit(
            manager: &SkillManager,
            name: &str,
            visiting: &mut HashSet<String>,
            visited: &mut HashSet<String>,
            out: &mut Vec<String>,
        ) -> Result<()> {
            if visited.contains(name) {
                return Ok(());
            }
            if !visiting.insert(name.to_string()) {
                anyhow::bail!("skill dependency cycle detected at '{name}'");
            }
            let manifest = manager
                .find_by_name(name)
                .ok_or_else(|| anyhow::anyhow!("required skill '{name}' was not found"))?;
            for dependency in &manifest.requires {
                visit(manager, dependency, visiting, visited, out)?;
            }
            visiting.remove(name);
            visited.insert(name.to_string());
            out.push(name.to_string());
            Ok(())
        }

        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        let mut out = Vec::new();
        visit(self, name, &mut visiting, &mut visited, &mut out)?;
        Ok(out)
    }

    /// Parse `@skill:` mentions in `text`, activate found skills, queue miss notes.
    /// Returns `(activated_names, missing_names)`.
    pub fn activate_mentions_in(
        &mut self,
        session_id: Option<&str>,
        text: &str,
    ) -> (Vec<String>, Vec<String>) {
        let names = parse_skill_mentions(text);
        let mut activated = Vec::new();
        let mut missing = Vec::new();
        for name in names {
            match self.activation_plan(&name) {
                Ok(plan) => {
                    self.requested_by_scope
                        .entry(Self::scope_key(session_id))
                        .or_default()
                        .insert(name.clone());
                    self.active_set_mut(session_id).extend(plan);
                    activated.push(name);
                }
                Err(error) => {
                    self.push_note(session_id, error.to_string());
                    missing.push(name);
                }
            }
        }
        (activated, missing)
    }

    fn push_note(&mut self, session_id: Option<&str>, note: String) {
        let key = Self::scope_key(session_id);
        self.pending_notes_by_scope
            .entry(key)
            .or_default()
            .push(note);
    }

    /// Drain one-shot notes for injection into Segment 6 (clears the queue).
    pub fn drain_notes(&mut self, session_id: Option<&str>) -> Vec<String> {
        let key = Self::scope_key(session_id);
        self.pending_notes_by_scope.remove(&key).unwrap_or_default()
    }

    /// Deactivate a skill in the anonymous/default scope.
    pub fn deactivate(&mut self, name: &str) -> bool {
        self.deactivate_for(None, name)
    }

    /// Deactivate a skill for a specific session scope.
    pub fn deactivate_for(&mut self, session_id: Option<&str>, name: &str) -> bool {
        let key = Self::scope_key(session_id);
        let removed = self
            .requested_by_scope
            .get_mut(&key)
            .is_some_and(|requested| requested.remove(name));
        if !removed {
            return false;
        }
        self.rebuild_scope(session_id);
        true
    }

    /// Check if a skill is active in the anonymous/default scope.
    pub fn is_active(&self, name: &str) -> bool {
        self.is_active_for(None, name)
    }

    /// Check if a skill is active for a specific session scope.
    pub fn is_active_for(&self, session_id: Option<&str>, name: &str) -> bool {
        self.active_set(session_id)
            .is_some_and(|a| a.contains(name))
    }

    /// Get names of skills active in the anonymous/default scope.
    pub fn active_skill_names(&self) -> Vec<&str> {
        self.active_skill_names_for(None)
    }

    /// Get names of skills active for a specific session scope.
    pub fn active_skill_names_for(&self, session_id: Option<&str>) -> Vec<&str> {
        match self.active_set(session_id) {
            Some(a) => a.iter().map(|s| s.as_str()).collect(),
            None => Vec::new(),
        }
    }

    /// Deactivate all skills in the anonymous/default scope.
    pub fn deactivate_all(&mut self) {
        self.deactivate_all_for(None);
    }

    /// Deactivate all skills for a specific session scope.
    pub fn deactivate_all_for(&mut self, session_id: Option<&str>) {
        self.requested_by_scope.remove(&Self::scope_key(session_id));
        if let Some(set) = self.active_by_scope.get_mut(&Self::scope_key(session_id)) {
            set.clear();
        }
    }

    /// Drop all active-skill state for a session (on session delete).
    pub fn clear_session(&mut self, session_id: &str) {
        self.active_by_scope.remove(session_id);
        self.requested_by_scope.remove(session_id);
        self.pending_notes_by_scope.remove(session_id);
    }

    fn rebuild_scope(&mut self, session_id: Option<&str>) {
        let key = Self::scope_key(session_id);
        let requested = self
            .requested_by_scope
            .get(&key)
            .cloned()
            .unwrap_or_default();
        let mut active = HashSet::new();
        for root in requested {
            if let Ok(plan) = self.activation_plan(&root) {
                active.extend(plan);
            }
        }
        self.active_by_scope.insert(key, active);
    }

    // ── Content loading ──────────────────────────────────────────────

    /// Load the full content of a skill (body after frontmatter).
    pub fn load_content(&self, manifest: &SkillManifest) -> Result<String> {
        // Try reading body from the SKILL.md file
        if manifest.content_path.as_os_str().is_empty() {
            anyhow::bail!("No content path for skill '{}'", manifest.name);
        }
        SkillManifest::read_body(&manifest.content_path)
    }

    /// Build context string for all active skills (anonymous/default scope).
    pub fn build_active_context(&self) -> String {
        self.build_active_context_for(None)
    }

    /// Build context string for skills active in a session scope.
    pub fn build_active_context_for(&self, session_id: Option<&str>) -> String {
        let Some(active) = self.active_set(session_id) else {
            return String::new();
        };
        if active.is_empty() {
            return String::new();
        }

        let mut parts: Vec<String> = Vec::new();
        for skill in &self.manifests {
            if active.contains(&skill.manifest.name) {
                if let Ok(content) = self.load_content(&skill.manifest) {
                    let mut block = format!(
                        "## Skill: {} (v{})\nSkill directory: {}\n{}\n",
                        skill.manifest.name,
                        skill.manifest.version,
                        skill.source_dir.display(),
                        content,
                    );
                    let assets = self.discover_assets(&skill.manifest.name);
                    block.push_str(&Self::assets_context(&assets));
                    parts.push(block);
                }
            }
        }

        if parts.is_empty() {
            return String::new();
        }

        let mut result = format!(
            "The following skills are ACTIVE and loaded into your context. \
             Use their knowledge to guide your responses. For files listed under \
             Skill assets, prefer `skill_read_resource` with the listed relative \
             path; use the absolute path only when a tool explicitly requires it. \
             Do not shell-find or glob the skill tree:\n\n{}",
            parts.join("\n")
        );

        // Append script catalog for all active skills.
        let active_scripts = self.get_active_scripts_for(session_id);
        if !active_scripts.is_empty() {
            result.push_str(&Self::scripts_context(&active_scripts));
        }

        result
    }

    /// Build a concise skill catalog for the system prompt (anonymous scope markers).
    pub fn build_catalog(&self) -> String {
        self.build_catalog_for(None)
    }

    /// Build skill catalog with ACTIVE markers for a session scope.
    pub fn build_catalog_for(&self, session_id: Option<&str>) -> String {
        if self.manifests.is_empty() {
            return String::new();
        }

        let mut lines: Vec<String> = Vec::new();
        lines.push(
            "Available skills (inactive: call `skill_load` with the name, or wait for \
             `@skill:name` / auto-trigger. Do not browse skill directories to discover \
             SKILL.md; use `skill_search` if this compact catalog is truncated. \
             Active skills are already injected below — follow their body \
             and use `skill_read_resource` on listed asset paths; do not call `skill_load` again \
             for [ACTIVE] skills):"
                .to_string(),
        );

        for skill in &self.manifests {
            let active_marker = if self.is_active_for(session_id, &skill.manifest.name) {
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

    /// Scan directories and re-activate skills that were active in each scope.
    /// Returns `(skill_count, reactivated_count)`.
    pub fn reload_preserving_active(&mut self) -> Result<(usize, usize)> {
        let requested_snapshot = self.requested_by_scope.clone();
        let count = self.scan()?;
        self.active_by_scope.clear();
        self.requested_by_scope = requested_snapshot;
        let mut reactivated = 0usize;
        let scopes: Vec<String> = self.requested_by_scope.keys().cloned().collect();
        for scope in scopes {
            self.rebuild_scope(if scope.is_empty() { None } else { Some(&scope) });
            reactivated += self.active_by_scope.get(&scope).map_or(0, HashSet::len);
        }
        Ok((count, reactivated))
    }

    // ── Lookup ──────────────────────────────────────────────────────

    /// Get the directory containing a skill's files (SKILL.md, scripts/, etc.).
    pub fn source_dir_of(&self, name: &str) -> Option<&Path> {
        self.manifests
            .iter()
            .find(|s| s.manifest.name == name)
            .map(|s| s.source_dir.as_path())
    }

    /// Discover non-script skill assets (templates/, references/, assets/, top-level files).
    /// Returns `(relative_path, absolute_path)` pairs, capped for context size.
    pub fn discover_assets(&self, name: &str) -> Vec<(String, PathBuf)> {
        self.discover_resources_with_limits(name, ASSET_MAX_FILES, ASSET_MAX_DEPTH)
    }

    /// Full on-demand resource index. This is intentionally larger than the
    /// prompt-facing asset preview used by `discover_assets`.
    pub fn discover_resources(&self, name: &str) -> Vec<(String, PathBuf)> {
        self.discover_resources_with_limits(name, RESOURCE_MAX_FILES, RESOURCE_MAX_DEPTH)
    }

    fn discover_resources_with_limits(
        &self,
        name: &str,
        max_files: usize,
        max_depth: usize,
    ) -> Vec<(String, PathBuf)> {
        let Some(source_dir) = self.source_dir_of(name) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut seen = HashSet::new();

        // Top-level files except SKILL.md
        if let Ok(entries) = std::fs::read_dir(source_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if file_name.eq_ignore_ascii_case("SKILL.md") || file_name.starts_with('.') {
                    continue;
                }
                let rel = file_name.to_string();
                if seen.insert(rel.clone()) {
                    if let Ok(canonical) =
                        manifest::resolve_existing_path_within(source_dir, &path, "skill asset")
                    {
                        out.push((rel, canonical));
                    }
                    if out.len() >= max_files {
                        out.sort_by(|a, b| a.0.cmp(&b.0));
                        return out;
                    }
                }
            }
        }

        for dir_name in ASSET_DIRS {
            let dir = source_dir.join(dir_name);
            if !dir.is_dir() {
                continue;
            }
            Self::walk_asset_dir(
                &dir, source_dir, 1, max_depth, max_files, &mut out, &mut seen,
            );
            if out.len() >= max_files {
                break;
            }
        }

        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Resolve and read one resource by a skill-relative path. The path must
    /// stay within the canonical skill directory, including through symlinks.
    pub fn read_resource(&self, name: &str, relative_path: &str) -> Result<String> {
        let source_dir = self
            .source_dir_of(name)
            .ok_or_else(|| anyhow::anyhow!("skill '{name}' not found"))?;
        let requested = Path::new(relative_path);
        if requested.is_absolute() {
            anyhow::bail!("resource path must be relative to the skill directory");
        }
        let resolved =
            manifest::resolve_existing_path_within(source_dir, requested, "skill resource")?;
        if resolved.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
            anyhow::bail!("use skill_load to read SKILL.md");
        }
        Ok(std::fs::read_to_string(resolved)?)
    }

    fn walk_asset_dir(
        dir: &Path,
        root: &Path,
        depth: usize,
        max_depth: usize,
        max_files: usize,
        out: &mut Vec<(String, PathBuf)>,
        seen: &mut HashSet<String>,
    ) {
        if depth > max_depth || out.len() >= max_files {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if out.len() >= max_files {
                return;
            }
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if path.is_symlink() {
                    continue;
                }
                Self::walk_asset_dir(&path, root, depth + 1, max_depth, max_files, out, seen);
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .ok()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|| name.to_string());
                if seen.insert(rel.clone()) {
                    if let Ok(canonical) =
                        manifest::resolve_existing_path_within(root, &path, "skill asset")
                    {
                        out.push((rel, canonical));
                    }
                }
            }
        }
    }

    fn assets_context(assets: &[(String, PathBuf)]) -> String {
        if assets.is_empty() {
            return String::new();
        }
        let mut lines = vec!["\n### Skill assets\n".to_string()];
        for (rel, abs) in assets {
            lines.push(format!("- {rel} → {}\n", abs.display()));
        }
        lines.join("")
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
                        if manifest::resolve_existing_path_within(source_dir, &path, "script file")
                            .is_err()
                        {
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
                                    format!(
                                        "scripts/{}",
                                        path.file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or(script_name)
                                    )
                                }),
                            timeout_secs: 60,
                            schema: serde_json::Value::Object(serde_json::Map::new()),
                        });
                    }
                }
            }
        }

        scripts.sort_by(|a, b| a.name.cmp(&b.name));
        scripts
    }

    /// Get script entries for a specific skill (manifest + auto-discovered).
    pub fn get_scripts(&self, name: &str) -> Vec<ScriptEntry> {
        self.discover_scripts(name)
    }

    /// Get all active skill scripts as (skill_name, ScriptEntry) pairs (default scope).
    pub fn get_active_scripts(&self) -> Vec<(String, ScriptEntry)> {
        self.get_active_scripts_for(None)
    }

    /// Get active skill scripts for a session scope.
    pub fn get_active_scripts_for(&self, session_id: Option<&str>) -> Vec<(String, ScriptEntry)> {
        let mut result = Vec::new();
        let Some(active) = self.active_set(session_id) else {
            return result;
        };
        for name in active {
            for script in self.discover_scripts(name) {
                result.push((name.clone(), script));
            }
        }
        result
    }

    /// Format a single script entry as a catalog line for the context prompt.
    pub fn script_line(skill_name: &str, script: &ScriptEntry) -> String {
        let api_name =
            crate::tools::sanitize_tool_name(&format!("skill.{}.{}", skill_name, script.name));
        format!(
            "- **{}**: {} [timeout: {}s]",
            api_name, script.description, script.timeout_secs
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
                "== Skill: {} (v{}) ==\nSkill directory: {}\n{}\n",
                skill.manifest.name,
                skill.manifest.version,
                skill.source_dir.display(),
                content,
            );
            let assets = self.discover_assets(&skill.manifest.name);
            result.push_str(&Self::assets_context(&assets));
            result.push_str(&format!("== End Skill: {} ==\n", skill.manifest.name));
            // Append available scripts.
            let scripts = self.discover_scripts(&skill.manifest.name);
            if !scripts.is_empty() {
                result.push_str("\n### Available Scripts\n");
                for script in &scripts {
                    let api_name = crate::tools::sanitize_tool_name(&format!(
                        "skill.{}.{}",
                        skill.manifest.name, script.name
                    ));
                    result.push_str(&format!(
                        "- {}: {} ({})\n",
                        api_name, script.description, script.file
                    ));
                }
            }
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    /// Union declared skill names with session-scoped actives (deduped, declared order first).
    pub fn resolve_subagent_skills(
        &self,
        declared: &[String],
        session_id: Option<&str>,
    ) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for name in declared {
            let expanded = self
                .activation_plan(name)
                .unwrap_or_else(|_| vec![name.clone()]);
            for expanded_name in expanded {
                if seen.insert(expanded_name.clone()) {
                    out.push(expanded_name);
                }
            }
        }
        for name in self.active_skill_names_for(session_id) {
            let owned = name.to_string();
            if seen.insert(owned.clone()) {
                out.push(owned);
            }
        }
        out
    }

    pub fn search_dirs(&self) -> &[PathBuf] {
        &self.search_dirs
    }

    pub fn count(&self) -> usize {
        self.manifests.len()
    }

    // ── Composition helpers (used by Workflow executor + Standalone paths) ──

    /// Inject skill content for the given skill `names` into `system_prompt`.
    ///
    /// This is the canonical implementation used by every subagent construction
    /// path (workflow `execute_agent_node` and `run_agent_standalone`). It
    /// folds each named skill's `SKILL.md` body + available script catalog
    /// into the subagent's system prompt.
    ///
    /// Calling with `skill_manager = None` returns the prompt unchanged.
    pub fn inject_skill_content_into(
        skill_manager: Option<&Arc<Mutex<SkillManager>>>,
        skills: &[String],
        system_prompt: &str,
    ) -> String {
        let Some(sm) = skill_manager else {
            return system_prompt.to_string();
        };
        let mgr = sm.lock();
        let mut prompt = system_prompt.to_string();
        for name in skills {
            if let Ok(Some(content)) = mgr.load_skill_context(name) {
                if !prompt.is_empty() {
                    prompt.push_str("\n\n");
                }
                prompt.push_str(&content);
            }
        }
        prompt
    }

    /// Register `skill.<name>.<script>` tools for the named skills into
    /// `registry`, removing any previously-registered `skill.*` tools first
    /// (so a Brain-built registry doesn't leak Brain-wide active_skills'
    /// script tools to a subagent that only declares a subset).
    ///
    /// This is the subagent-side mirror of `Run::sync_skill_scripts`, but
    /// restricted to an explicit skill list rather than the live `active_skills`
    /// set, so workflow / standalone agent nodes only get script tools for
    /// their *own* declared skills (P2-16: don't inherit unrelated skills).
    ///
    /// When `supervisor` is set, the script tools will use it for
    /// process-group isolation.
    pub fn sync_skill_scripts_for_skills(
        skill_manager: Option<&Arc<Mutex<SkillManager>>>,
        skills: &[String],
        registry: &mut crate::tools::ToolRegistry,
        supervisor: Option<Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>>,
    ) {
        use crate::tools::script::SkillScriptTool;
        // 1. Remove any existing `skill.*` tools (could be Brain-inherited).
        let existing_skill_tools: Vec<String> = registry
            .list_names()
            .into_iter()
            .filter(|n| n.starts_with("skill."))
            .map(|s| s.to_string())
            .collect();
        if !existing_skill_tools.is_empty() {
            let names: Vec<&str> = existing_skill_tools.iter().map(|s| s.as_str()).collect();
            registry.remove_all(&names);
        }

        let Some(sm) = skill_manager else {
            return;
        };
        crate::tools::skill::register_skill_resource_tools(registry, sm.clone());
        let mgr = sm.lock();

        // 2. For each named skill, register script tools.
        for skill_name in skills {
            let Some(source_dir) = mgr.source_dir_of(skill_name) else {
                tracing::warn!(
                    skill = %skill_name,
                    "sync_skill_scripts_for_skills: skill not found — skipping"
                );
                continue;
            };
            for script in mgr.discover_scripts(skill_name) {
                let tool_name = format!("skill.{}.{}", skill_name, script.name);
                if registry.has(&tool_name) {
                    // Shouldn't happen given clear above + unique script
                    // names within a skill, but defensive.
                    continue;
                }
                let mut tool = SkillScriptTool::new(skill_name, &script, source_dir.to_path_buf());
                if let Some(sv) = &supervisor {
                    tool = tool.with_supervisor(sv.clone());
                }
                registry.register(Box::new(tool));
            }
        }
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
    fn active_skills_are_session_scoped() {
        let dir = PathBuf::from("/tmp/test_skills_session_scope");
        let _ = fs::remove_dir_all(&dir);
        write_skill(&dir, "s1", "skill-one");

        let mut mgr = SkillManager::new(dir.clone());
        mgr.scan().unwrap();

        assert!(mgr.activate_for(Some("sess-a"), "skill-one"));
        assert!(mgr.is_active_for(Some("sess-a"), "skill-one"));
        assert!(!mgr.is_active_for(Some("sess-b"), "skill-one"));
        assert!(mgr.build_active_context_for(Some("sess-b")).is_empty());
        assert!(!mgr.build_active_context_for(Some("sess-a")).is_empty());

        mgr.clear_session("sess-a");
        assert!(!mgr.is_active_for(Some("sess-a"), "skill-one"));

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
        // High priority should come first (match bold catalog names, not substrings like "below")
        let high_pos = catalog.find("**high**").unwrap();
        let low_pos = catalog.find("**low**").unwrap();
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
    fn parse_skill_mentions_mid_sentence_and_punctuation() {
        let names = parse_skill_mentions("please use @skill:dividend-risk, thanks @skill:foo!");
        assert_eq!(names, vec!["dividend-risk".to_string(), "foo".to_string()]);
        // Dedup
        let names2 = parse_skill_mentions("@skill:a @skill:a");
        assert_eq!(names2, vec!["a".to_string()]);
    }

    #[test]
    fn discover_assets_lists_templates() {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("risk");
        fs::create_dir_all(skill.join("templates")).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: risk\ndescription: d\n---\nBody\n",
        )
        .unwrap();
        fs::write(skill.join("templates/output_schema.json"), "{}").unwrap();

        let mut mgr = SkillManager::new(dir.path().to_path_buf());
        mgr.scan().unwrap();
        let assets = mgr.discover_assets("risk");
        assert!(
            assets.iter().any(|(rel, abs)| {
                rel.replace('\\', "/") == "templates/output_schema.json" && abs.exists()
            }),
            "assets={assets:?}"
        );

        mgr.activate("risk");
        let ctx = mgr.build_active_context();
        assert!(ctx.contains("### Skill assets"));
        assert!(ctx.contains("templates/output_schema.json"));
    }

    #[test]
    fn activate_mentions_queues_miss_notes() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "known", "known");
        let mut mgr = SkillManager::new(dir.path().to_path_buf());
        mgr.scan().unwrap();

        let (ok, missing) =
            mgr.activate_mentions_in(Some("s1"), "run @skill:known and @skill:nope");
        assert_eq!(ok, vec!["known".to_string()]);
        assert_eq!(missing, vec!["nope".to_string()]);
        assert!(mgr.is_active_for(Some("s1"), "known"));

        let notes = mgr.drain_notes(Some("s1"));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("nope"));
        assert!(mgr.drain_notes(Some("s1")).is_empty());
    }

    #[test]
    fn resolve_subagent_skills_unions_session_actives() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "a", "a");
        write_skill(dir.path(), "b", "b");
        write_skill(dir.path(), "c", "c");
        let mut mgr = SkillManager::new(dir.path().to_path_buf());
        mgr.scan().unwrap();
        mgr.activate_for(Some("sess"), "b");
        mgr.activate_for(Some("other"), "c");

        let resolved =
            mgr.resolve_subagent_skills(&[String::from("a"), String::from("b")], Some("sess"));
        assert_eq!(resolved, vec!["a".to_string(), "b".to_string()]);

        let resolved2 = mgr.resolve_subagent_skills(&[String::from("a")], Some("sess"));
        assert_eq!(resolved2, vec!["a".to_string(), "b".to_string()]);
        // other session's active not inherited
        assert!(!resolved2.contains(&"c".to_string()));
    }

    #[test]
    fn catalog_active_does_not_only_push_skill_load() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "x", "x");
        let mut mgr = SkillManager::new(dir.path().to_path_buf());
        mgr.scan().unwrap();
        mgr.activate("x");
        let catalog = mgr.build_catalog();
        assert!(catalog.contains("[ACTIVE]"));
        assert!(catalog.contains("do not call `skill_load` again"));
    }

    #[test]
    fn activation_resolves_requires_dependency_first() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "child", "child");
        fs::create_dir_all(dir.path().join("parent")).unwrap();
        fs::write(
            dir.path().join("parent/SKILL.md"),
            "---\nname: parent\ndescription: parent\nrequires: [child]\n---\nParent body\n",
        )
        .unwrap();

        let mut mgr = SkillManager::new(dir.path().to_path_buf());
        mgr.scan().unwrap();
        assert_eq!(
            mgr.activation_plan("parent").unwrap(),
            vec!["child", "parent"]
        );
        assert!(mgr.activate_for(Some("s"), "parent"));
        assert!(mgr.is_active_for(Some("s"), "child"));
        assert!(mgr.is_active_for(Some("s"), "parent"));
        assert!(mgr.deactivate_for(Some("s"), "parent"));
        assert!(!mgr.is_active_for(Some("s"), "child"));
        assert!(!mgr.is_active_for(Some("s"), "parent"));
    }

    #[test]
    fn activation_rejects_dependency_cycles_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        for (name, dependency) in [("a", "b"), ("b", "a")] {
            fs::create_dir_all(dir.path().join(name)).unwrap();
            fs::write(
                dir.path().join(name).join("SKILL.md"),
                format!(
                    "---\nname: {name}\ndescription: {name}\nrequires: [{dependency}]\n---\nBody\n"
                ),
            )
            .unwrap();
        }
        let mut mgr = SkillManager::new(dir.path().to_path_buf());
        mgr.scan().unwrap();
        assert!(
            mgr.activation_plan("a")
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
        assert!(!mgr.activate_for(Some("s"), "a"));
        assert!(mgr.active_skill_names_for(Some("s")).is_empty());
    }

    #[test]
    fn scans_explicit_nested_subskills() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "parent", "parent");
        let nested_root = dir.path().join("parent/subskills");
        write_skill(&nested_root, "child", "child");

        let mut mgr = SkillManager::new(dir.path().to_path_buf());
        assert_eq!(mgr.scan().unwrap(), 2);
        assert!(mgr.find_by_name("child").is_some());
    }

    #[test]
    fn workspace_roots_include_agents_and_codex() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SkillManager::with_dirs(Vec::new());
        assert!(mgr.add_workspace_root(dir.path()));
        assert!(
            mgr.search_dirs()
                .contains(&dir.path().join(".agents/skills"))
        );
        assert!(
            mgr.search_dirs()
                .contains(&dir.path().join(".codex/skills"))
        );
        assert!(!mgr.add_workspace_root(dir.path()));
    }

    #[test]
    fn global_defaults_exclude_process_workspace_roots() {
        let cwd = std::env::current_dir().unwrap();
        let mgr = SkillManager::with_global_defaults();
        for local in [
            cwd.join(".agent/skills"),
            cwd.join(".agents/skills"),
            cwd.join(".claude/skills"),
            cwd.join(".codex/skills"),
            cwd.join("skills"),
        ] {
            assert!(!mgr.search_dirs().contains(&local));
        }
    }

    #[test]
    fn read_resource_rejects_escape() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "safe", "safe");
        fs::create_dir_all(dir.path().join("safe/references")).unwrap();
        fs::write(dir.path().join("safe/references/guide.md"), "guide").unwrap();
        fs::write(dir.path().join("secret.txt"), "secret").unwrap();

        let mut mgr = SkillManager::new(dir.path().to_path_buf());
        mgr.scan().unwrap();
        assert_eq!(
            mgr.read_resource("safe", "references/guide.md").unwrap(),
            "guide"
        );
        assert!(mgr.read_resource("safe", "../secret.txt").is_err());
    }

    #[test]
    fn on_demand_resource_index_goes_deeper_than_prompt_preview() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "deep", "deep");
        let nested = dir.path().join("deep/references/api/v2/examples");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("request.json"), "{}").unwrap();
        let mut mgr = SkillManager::new(dir.path().to_path_buf());
        mgr.scan().unwrap();
        assert!(!mgr
            .discover_assets("deep")
            .iter()
            .any(|(path, _)| path.ends_with("request.json")));
        assert!(mgr
            .discover_resources("deep")
            .iter()
            .any(|(path, _)| path == "references/api/v2/examples/request.json"));
    }

    #[test]
    fn scan_reports_invalid_manifest() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("broken")).unwrap();
        fs::write(
            dir.path().join("broken/SKILL.md"),
            "---\nname: broken\ndescription:\n---\nBody\n",
        )
        .unwrap();
        let mut mgr = SkillManager::new(dir.path().to_path_buf());
        assert_eq!(mgr.scan().unwrap(), 0);
        assert_eq!(mgr.diagnostics().len(), 1);
        assert!(mgr.diagnostics()[0].message.contains("description"));
    }

    #[test]
    fn subagent_skill_sync_registers_resources_and_validated_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("reporting");
        fs::create_dir_all(skill.join("scripts")).unwrap();
        fs::write(skill.join("scripts/export.sh"), "#!/bin/sh\n").unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: reporting\ndescription: reports\nscripts:\n  - name: export\n    description: export\n    file: scripts/export.sh\n---\nBody\n",
        )
        .unwrap();
        let mut manager = SkillManager::new(dir.path().to_path_buf());
        manager.scan().unwrap();
        let manager = Arc::new(Mutex::new(manager));
        let mut registry = crate::tools::ToolRegistry::new();
        SkillManager::sync_skill_scripts_for_skills(
            Some(&manager),
            &["reporting".to_string()],
            &mut registry,
            None,
        );
        assert!(registry.has("skill.reporting.export"));
        assert!(registry.has("skill_list_resources"));
        assert!(registry.has("skill_read_resource"));
    }
}
