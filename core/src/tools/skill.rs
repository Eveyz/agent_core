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
    registry.register(Box::new(SkillLoadTool::new(manager.clone())));
    registry.register(Box::new(SkillDeactivateTool::new(manager.clone())));
    registry.register(Box::new(SkillReloadTool::new(manager)));
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
Use this to discover what skills are available before loading one."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        let mgr = self.manager.lock();
        let skills = mgr.list();
        if skills.is_empty() {
            return Ok(
                "No skills available. Create SKILL.md files in .agent/skills/ to add skills."
                    .to_string(),
            );
        }

        let mut out = String::from("Available skills:\n");
        for skill in skills {
            let active = if mgr.is_active(&skill.name) {
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
                "- {}{}{}: {}\n",
                skill.name, active, triggers, skill.description
            ));
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
        "Load a skill by name to inject its content into your context. \
Args: name (string). The skill's knowledge will guide your responses."
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
        let name = args["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'name'"))?;
        let mut mgr = self.manager.lock();

        let description = mgr.find_by_name(name).map(|m| m.description.clone()).unwrap_or_default();

        let content = match mgr.load_skill_context(name)? {
            Some(c) => c,
            None => {
                return Ok(format!(
                    "Skill '{}' not found. Use skill_list to see available skills.",
                    name
                ));
            }
        };

        mgr.activate(name);
        Ok(format!(
            "Skill '{}' loaded and activated.\nDescription: {}\n\n{}",
            name, description, content
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
        let name = args["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'name'"))?;
        let mut mgr = self.manager.lock();

        if name == "all" {
            let count = mgr.active_skill_names().len();
            mgr.deactivate_all();
            return Ok(format!("Deactivated all {} active skills.", count));
        }

        if mgr.deactivate(name) {
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
        let old_active: Vec<String> = mgr
            .active_skill_names()
            .iter()
            .map(|s| s.to_string())
            .collect();

        let count = mgr.scan()?;

        // Re-activate skills that still exist
        let mut reactivated = 0usize;
        for name in &old_active {
            if mgr.activate(name) {
                reactivated += 1;
            }
        }

        Ok(format!(
            "Reloaded {} skills from disk. {} previously active skills restored.",
            count, reactivated
        ))
    }
}
