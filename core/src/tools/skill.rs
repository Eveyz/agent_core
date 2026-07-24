use crate::skills::SkillManager;
use crate::tools::Tool;
use anyhow::Result;
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;

pub fn register_skill_tools(
    registry: &mut crate::tools::ToolRegistry,
    manager: Arc<Mutex<SkillManager>>,
) {
    registry.register(Box::new(SkillListTool::new(manager.clone())));
    registry.register(Box::new(SkillSearchTool::new(manager.clone())));
    registry.register(Box::new(SkillLoadTool::new(manager.clone())));
    register_skill_resource_tools(registry, manager.clone());
    registry.register(Box::new(SkillDeactivateTool::new(manager.clone())));
    registry.register(Box::new(SkillReloadTool::new(manager)));
}

struct SkillSearchTool {
    manager: Arc<Mutex<SkillManager>>,
}

impl SkillSearchTool {
    fn new(manager: Arc<Mutex<SkillManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl Tool for SkillSearchTool {
    fn name(&self) -> &str {
        "skill_search"
    }

    fn description(&self) -> &str {
        "Search skill names, descriptions, tags, and triggers without loading every skill body."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'query'"))?
            .trim()
            .to_lowercase();
        if query.is_empty() {
            anyhow::bail!("query must not be empty");
        }
        let mgr = self.manager.lock();
        let mut matches = Vec::new();
        for (skill, source) in mgr.list_with_sources() {
            let matched = skill.name.to_lowercase().contains(&query)
                || skill.description.to_lowercase().contains(&query)
                || skill.tags.iter().any(|tag| tag.to_lowercase().contains(&query))
                || skill
                    .triggers
                    .iter()
                    .any(|trigger| trigger.to_lowercase().contains(&query));
            if matched {
                matches.push(format!(
                    "- {}: {} [source: {}]",
                    skill.name,
                    skill.description,
                    source.display()
                ));
            }
            if matches.len() == 20 {
                break;
            }
        }
        if matches.is_empty() {
            Ok(format!("No skills matched '{query}'."))
        } else {
            Ok(format!("Matching skills:\n{}", matches.join("\n")))
        }
    }
}

pub fn register_skill_resource_tools(
    registry: &mut crate::tools::ToolRegistry,
    manager: Arc<Mutex<SkillManager>>,
) {
    registry.register(Box::new(SkillListResourcesTool::new(manager.clone())));
    registry.register(Box::new(SkillReadResourceTool::new(manager)));
}

// ── Skill resources ────────────────────────────────────────────────

struct SkillListResourcesTool {
    manager: Arc<Mutex<SkillManager>>,
}

impl SkillListResourcesTool {
    fn new(manager: Arc<Mutex<SkillManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl Tool for SkillListResourcesTool {
    fn name(&self) -> &str {
        "skill_list_resources"
    }

    fn description(&self) -> &str {
        "List indexed templates, references, assets, and subskill files for a skill."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let name = args["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'name'"))?;
        let mgr = self.manager.lock();
        if mgr.find_by_name(name).is_none() {
            return Ok(format!("Skill '{name}' not found."));
        }
        let resources = mgr.discover_resources(name);
        if resources.is_empty() {
            return Ok(format!("Skill '{name}' has no indexed resources."));
        }
        let mut out = format!("Resources for skill '{name}':\n");
        for (relative, _) in resources {
            out.push_str(&format!("- {relative}\n"));
        }
        Ok(out)
    }
}

struct SkillReadResourceTool {
    manager: Arc<Mutex<SkillManager>>,
}

impl SkillReadResourceTool {
    fn new(manager: Arc<Mutex<SkillManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl Tool for SkillReadResourceTool {
    fn name(&self) -> &str {
        "skill_read_resource"
    }

    fn description(&self) -> &str {
        "Read one UTF-8 resource using a skill name and a skill-relative path."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "path": {"type": "string", "description": "Relative path such as references/api.md"}
            },
            "required": ["name", "path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let name = args["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'name'"))?;
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        self.manager.lock().read_resource(name, path)
    }
}

/// Session scope injected by the tool orchestrator (`_session_id`).
fn session_scope(args: &Value) -> Option<&str> {
    args.get("_session_id").and_then(|v| v.as_str())
}

// ── SkillListTool ────────────────────────────────────────────────────

struct SkillListTool {
    manager: Arc<Mutex<SkillManager>>,
}

impl SkillListTool {
    fn new(manager: Arc<Mutex<SkillManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl Tool for SkillListTool {
    fn name(&self) -> &str {
        "skill_list"
    }

    fn description(&self) -> &str {
        "List all available skills with names, descriptions, triggers, and active status. \
Use when the compact Skill Catalog is truncated or you need full discovery before skill_load."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let sid = session_scope(&args);
        let mgr = self.manager.lock();
        let skills = mgr.list_with_sources();
        if skills.is_empty() {
            return Ok(
                "No skills available. Create SKILL.md files in .agent/skills/ to add skills."
                    .to_string(),
            );
        }

        let mut out = String::from("Available skills:\n");
        for (skill, source) in skills {
            let active = if mgr.is_active_for(sid, &skill.name) {
                " [ACTIVE]"
            } else {
                ""
            };
            let triggers = if skill.triggers.is_empty() {
                String::new()
            } else {
                format!(" (triggers: {})", skill.triggers.join(", "))
            };
            out.push_str(&format!(
                "- {}{}{}: {} [source: {}]\n",
                skill.name,
                active,
                triggers,
                skill.description,
                source.display()
            ));
        }
        if !mgr.diagnostics().is_empty() {
            out.push_str(&format!(
                "\n{} skill entries were skipped or shadowed:\n",
                mgr.diagnostics().len()
            ));
            for diagnostic in mgr.diagnostics().iter().take(10) {
                out.push_str(&format!(
                    "- {}: {}\n",
                    diagnostic.path.display(),
                    diagnostic.message
                ));
            }
        }
        out.push_str("\nUse skill_load <name> to activate a skill.");
        Ok(out)
    }
}

// ── SkillLoadTool ────────────────────────────────────────────────────

struct SkillLoadTool {
    manager: Arc<Mutex<SkillManager>>,
}

impl SkillLoadTool {
    fn new(manager: Arc<Mutex<SkillManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl Tool for SkillLoadTool {
    fn name(&self) -> &str {
        "skill_load"
    }

    fn description(&self) -> &str {
        "Load a skill by name when its catalog description/triggers clearly match the \
current task. Prefer one skill per task; do not load speculative stacks. \
Args: name (string). Injects the skill body into context for subsequent turns."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Skill name to load"}
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let sid = session_scope(&args);
        let name = args["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'name'"))?;
        let mut mgr = self.manager.lock();

        let description = mgr
            .find_by_name(name)
            .map(|m| m.description.clone())
            .unwrap_or_default();

        let plan = match mgr.activation_plan(name) {
            Ok(plan) => plan,
            Err(error) => return Ok(format!("Skill '{name}' could not be activated: {error}")),
        };
        let mut loaded = Vec::new();
        for skill_name in &plan {
            if let Some(content) = mgr.load_skill_context(skill_name)? {
                loaded.push(content);
            }
        }
        if !mgr.activate_for(sid, name) {
            return Ok(format!("Skill '{name}' could not be activated."));
        }
        Ok(format!(
            "Skill '{}' loaded and activated with {} dependency package(s).\nDescription: {}\n\n{}",
            name,
            plan.len().saturating_sub(1),
            description,
            loaded.join("\n")
        ))
    }
}

// ── SkillDeactivateTool ──────────────────────────────────────────────

struct SkillDeactivateTool {
    manager: Arc<Mutex<SkillManager>>,
}

impl SkillDeactivateTool {
    fn new(manager: Arc<Mutex<SkillManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl Tool for SkillDeactivateTool {
    fn name(&self) -> &str {
        "skill_deactivate"
    }

    fn description(&self) -> &str {
        "Deactivate a previously loaded skill to remove it from context. \
Args: name (string). Use 'all' to deactivate all active skills."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Skill name or 'all'"}
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let sid = session_scope(&args);
        let name = args["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'name'"))?;
        let mut mgr = self.manager.lock();

        if name == "all" {
            let count = mgr.active_skill_names_for(sid).len();
            mgr.deactivate_all_for(sid);
            return Ok(format!("Deactivated all {} active skills.", count));
        }

        if mgr.deactivate_for(sid, name) {
            Ok(format!("Skill '{}' deactivated.", name))
        } else {
            Ok(format!("Skill '{}' was not active.", name))
        }
    }
}

// ── SkillReloadTool ──────────────────────────────────────────────────

struct SkillReloadTool {
    manager: Arc<Mutex<SkillManager>>,
}

impl SkillReloadTool {
    fn new(manager: Arc<Mutex<SkillManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl Tool for SkillReloadTool {
    fn name(&self) -> &str {
        "skill_reload"
    }

    fn description(&self) -> &str {
        "Rescan all skill directories and reload manifests. \
Use this after adding or modifying SKILL.md files. Active skills are preserved."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        let mut mgr = self.manager.lock();
        let (count, reactivated) = mgr.reload_preserving_active()?;
        Ok(format!(
            "Reloaded {} skills from disk. {} previously active skills restored.",
            count, reactivated
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn resource_manager() -> (tempfile::TempDir, Arc<Mutex<SkillManager>>) {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("docs");
        fs::create_dir_all(skill.join("references")).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: docs\ndescription: docs\n---\nBody\n",
        )
        .unwrap();
        fs::write(skill.join("references/guide.md"), "Guide").unwrap();
        let mut manager = SkillManager::new(dir.path().to_path_buf());
        manager.scan().unwrap();
        (dir, Arc::new(Mutex::new(manager)))
    }

    #[tokio::test]
    async fn resource_read_is_contained_but_does_not_require_global_activation_state() {
        let (_dir, manager) = resource_manager();
        let tool = SkillReadResourceTool::new(manager.clone());
        let args = serde_json::json!({
            "name": "docs",
            "path": "references/guide.md",
            "_session_id": "s1"
        });
        assert_eq!(tool.execute(args).await.unwrap(), "Guide");
    }

    #[tokio::test]
    async fn resource_list_is_stable_and_relative() {
        let (_dir, manager) = resource_manager();
        let tool = SkillListResourcesTool::new(manager);
        let output = tool
            .execute(serde_json::json!({"name": "docs"}))
            .await
            .unwrap();
        assert!(output.contains("references/guide.md"));
        assert!(!output.contains("/tmp/"));
    }

    #[tokio::test]
    async fn search_finds_skill_without_loading_its_body() {
        let (_dir, manager) = resource_manager();
        let tool = SkillSearchTool::new(manager.clone());
        let output = tool
            .execute(serde_json::json!({"query": "docs"}))
            .await
            .unwrap();
        assert!(output.contains("docs"));
        assert!(!manager.lock().is_active("docs"));
    }
}
