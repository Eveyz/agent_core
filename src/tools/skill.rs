use crate::skills::SkillLoader;
use crate::tools::Tool;
use anyhow::Result;
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub fn register_skill_tools(
    registry: &mut crate::tools::ToolRegistry,
    loader: Arc<Mutex<SkillLoader>>,
) {
    registry.register(Box::new(SkillListTool::new(loader.clone())));
    registry.register(Box::new(SkillLoadTool::new(loader)));
}

struct SkillListTool {
    loader: Arc<Mutex<SkillLoader>>,
}

impl SkillListTool {
    fn new(loader: Arc<Mutex<SkillLoader>>) -> Self {
        Self { loader }
    }
}

#[async_trait::async_trait]
impl Tool for SkillListTool {
    fn name(&self) -> &str {
        "skill_list"
    }

    fn description(&self) -> &str {
        "List all available skills with their names and descriptions"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        let loader = self.loader.lock().unwrap();
        let skills = loader.list();
        if skills.is_empty() {
            return Ok("No skills available.".to_string());
        }

        let mut out = String::from("Available skills:\n");
        for skill in skills {
            out.push_str(&format!(
                "- {}: {} (triggers: {})\n",
                skill.name,
                skill.description,
                skill.triggers.join(", ")
            ));
        }
        Ok(out)
    }
}

struct SkillLoadTool {
    loader: Arc<Mutex<SkillLoader>>,
}

impl SkillLoadTool {
    fn new(loader: Arc<Mutex<SkillLoader>>) -> Self {
        Self { loader }
    }
}

#[async_trait::async_trait]
impl Tool for SkillLoadTool {
    fn name(&self) -> &str {
        "skill_load"
    }

    fn description(&self) -> &str {
        "Load a skill by name to inject its content into context"
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
        let name = args["name"].as_str().ok_or_else(|| anyhow::anyhow!("missing 'name'"))?;
        let loader = self.loader.lock().unwrap();
        match loader.load_skill_context(name)? {
            Some(content) => Ok(content),
            None => Ok(format!("Skill '{}' not found. Use skill_list to see available skills.", name)),
        }
    }
}
