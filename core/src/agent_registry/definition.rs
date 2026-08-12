//! Agent definitions and database CRUD.
//!
//! An [`AgentDef`] is a persisted, user-defined agent configuration. It is
//! the blueprint that [`super::AgentRegistry`] turns into runtime components
//! (`SubagentConfig`, `PermissionConfig`, `ModelConfig`) when a workflow or
//! standalone run needs to execute the agent.

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::config::{Config, ModelConfig};
use crate::memory::storage::Storage;
use crate::permission::{ConfigRule, PermissionConfig, PermissionMode};
use crate::subagent::{AgentMemoryIdentity, SubagentConfig};

/// A user-defined agent definition persisted in the `agents` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    /// "provider/model" or "" to use the global default model.
    pub model: String,
    pub skills: Vec<String>,
    /// Tool names; empty = inherit all available tools.
    pub tools: Vec<String>,
    pub permission_mode: String,
    /// Custom permission rules (JSON), merged onto the base `PermissionConfig`.
    #[serde(default = "default_json_array")]
    pub permission_rules: serde_json::Value,
    pub max_iterations: usize,
    pub max_context_tokens: usize,
    /// 0 = stateless, 1 = standard, 2 = deep.
    pub memory_enabled: u8,
    /// Empty = per-agent isolated memory; non-empty = shared group memory index.
    #[serde(default)]
    pub memory_group: String,
    pub icon: String,
    pub color: String,
    pub created_at: String,
    pub updated_at: String,
}

fn default_json_array() -> serde_json::Value {
    serde_json::Value::Array(Vec::new())
}

impl Default for AgentDef {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            system_prompt: String::new(),
            model: String::new(),
            skills: Vec::new(),
            tools: Vec::new(),
            permission_mode: "standard".to_string(),
            permission_rules: serde_json::Value::Array(Vec::new()),
            max_iterations: 50,
            max_context_tokens: 32000,
            memory_enabled: 1,
            memory_group: String::new(),
            icon: String::new(),
            color: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }
}

impl AgentDef {
    /// Resolve the memory isolation key.
    ///
    /// If `memory_group` is empty, memory is isolated per-agent (key = agent id).
    /// If `memory_group` is set, all agents in the group share one memory index.
    pub fn memory_key(&self) -> &str {
        if self.memory_group.is_empty() {
            &self.id
        } else {
            &self.memory_group
        }
    }

    pub fn memory_identity(&self) -> AgentMemoryIdentity {
        AgentMemoryIdentity::new(self.id.clone(), self.memory_key().to_string())
    }

    /// Parse a row from the `agents` table into an [`AgentDef`].
    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<AgentDef> {
        let skills_json: String = row.get("skills")?;
        let tools_json: String = row.get("tools")?;
        let rules_json: String = row.get("permission_rules")?;

        let skills: Vec<String> = serde_json::from_str(&skills_json).unwrap_or_default();
        let tools: Vec<String> = serde_json::from_str(&tools_json).unwrap_or_default();
        let permission_rules: serde_json::Value =
            serde_json::from_str(&rules_json).unwrap_or_default();

        Ok(AgentDef {
            id: row.get("id")?,
            name: row.get("name")?,
            description: row.get("description")?,
            system_prompt: row.get("system_prompt")?,
            model: row.get("model")?,
            skills,
            tools,
            permission_mode: row.get("permission_mode")?,
            permission_rules,
            max_iterations: row.get::<_, i64>("max_iterations")? as usize,
            max_context_tokens: row.get::<_, i64>("max_context_tokens")? as usize,
            memory_enabled: row.get::<_, i64>("memory_enabled")? as u8,
            memory_group: row.get("memory_group")?,
            icon: row.get("icon")?,
            color: row.get("color")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

/// Fields that may be updated on an existing agent.
///
/// All fields are optional; `None` means "leave unchanged".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentDefUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub system_prompt: Option<String>,
    pub model: Option<String>,
    pub skills: Option<Vec<String>>,
    pub tools: Option<Vec<String>>,
    pub permission_mode: Option<String>,
    pub permission_rules: Option<serde_json::Value>,
    pub max_iterations: Option<usize>,
    pub max_context_tokens: Option<usize>,
    pub memory_enabled: Option<u8>,
    pub memory_group: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
}

// ── Database CRUD ───────────────────────────────────────────────────

/// Create a new agent definition. A fresh UUID + timestamps are assigned.
pub fn create(storage: &Storage, def: &AgentDef) -> Result<AgentDef> {
    let id = if def.id.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        def.id.clone()
    };
    let now = Utc::now().to_rfc3339();

    let skills_json = serde_json::to_string(&def.skills)?;
    let tools_json = serde_json::to_string(&def.tools)?;
    let rules_json = serde_json::to_string(&def.permission_rules)?;

    let db = storage.conn();
    db.execute(
        "INSERT INTO agents (id, name, description, system_prompt, model, skills, tools, \
         permission_mode, permission_rules, max_iterations, max_context_tokens, \
         memory_enabled, memory_group, icon, color, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            id,
            def.name,
            def.description,
            def.system_prompt,
            def.model,
            skills_json,
            tools_json,
            def.permission_mode,
            rules_json,
            def.max_iterations as i64,
            def.max_context_tokens as i64,
            def.memory_enabled as i64,
            def.memory_group,
            def.icon,
            def.color,
            now,
            now,
        ],
    )
    .context("failed to insert agent")?;

    get_with_conn(&db, &id)
}

/// Fetch a single agent by id.
pub fn get(storage: &Storage, id: &str) -> Result<AgentDef> {
    let db = storage.conn();
    get_with_conn(&db, id)
}

pub(crate) fn get_with_conn(db: &rusqlite::Connection, id: &str) -> Result<AgentDef> {
    let mut stmt = db.prepare(
        "SELECT id, name, description, system_prompt, model, skills, tools, \
         permission_mode, permission_rules, max_iterations, max_context_tokens, \
         memory_enabled, memory_group, icon, color, created_at, updated_at \
         FROM agents WHERE id = ?1",
    )?;
    let agent = stmt
        .query_row(params![id], AgentDef::from_row)
        .context("agent not found")?;
    Ok(agent)
}

/// List all agent definitions, newest first.
pub fn list(storage: &Storage) -> Result<Vec<AgentDef>> {
    let db = storage.conn();
    let mut stmt = db.prepare(
        "SELECT id, name, description, system_prompt, model, skills, tools, \
         permission_mode, permission_rules, max_iterations, max_context_tokens, \
         memory_enabled, memory_group, icon, color, created_at, updated_at \
         FROM agents ORDER BY updated_at DESC",
    )?;
    let agents = stmt
        .query_map([], AgentDef::from_row)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(agents)
}

/// Update an existing agent with the non-`None` fields of `updates`.
pub fn update(storage: &Storage, id: &str, updates: &AgentDefUpdate) -> Result<AgentDef> {
    let now = Utc::now().to_rfc3339();
    let skills_json = updates
        .skills
        .as_ref()
        .map(|s| serde_json::to_string(s))
        .transpose()?;
    let tools_json = updates
        .tools
        .as_ref()
        .map(|t| serde_json::to_string(t))
        .transpose()?;
    let rules_json = updates
        .permission_rules
        .as_ref()
        .map(|r| serde_json::to_string(r))
        .transpose()?;

    let db = storage.conn();
    db.execute(
        "UPDATE agents SET \
         name = COALESCE(?2, name), \
         description = COALESCE(?3, description), \
         system_prompt = COALESCE(?4, system_prompt), \
         model = COALESCE(?5, model), \
         skills = COALESCE(?6, skills), \
         tools = COALESCE(?7, tools), \
         permission_mode = COALESCE(?8, permission_mode), \
         permission_rules = COALESCE(?9, permission_rules), \
         max_iterations = COALESCE(?10, max_iterations), \
         max_context_tokens = COALESCE(?11, max_context_tokens), \
         memory_enabled = COALESCE(?12, memory_enabled), \
         memory_group = COALESCE(?13, memory_group), \
         icon = COALESCE(?14, icon), \
         color = COALESCE(?15, color), \
         updated_at = ?16 \
         WHERE id = ?1",
        params![
            id,
            updates.name,
            updates.description,
            updates.system_prompt,
            updates.model,
            skills_json,
            tools_json,
            updates.permission_mode,
            rules_json,
            updates.max_iterations.map(|v| v as i64),
            updates.max_context_tokens.map(|v| v as i64),
            updates.memory_enabled.map(|v| v as i64),
            updates.memory_group,
            updates.icon,
            updates.color,
            now,
        ],
    )
    .context("failed to update agent")?;

    get_with_conn(&db, id)
}

/// Delete an agent by id. Cascades to `agent_history`.
pub fn delete(storage: &Storage, id: &str) -> Result<()> {
    let db = storage.conn();
    db.execute("DELETE FROM agents WHERE id = ?1", params![id])
        .context("failed to delete agent")?;
    Ok(())
}

// ── Runtime component builders ──────────────────────────────────────

/// Build a [`SubagentConfig`] from an [`AgentDef`].
///
/// `tools` empty means "inherit all available tools" — the executor decides
/// what "all" means at run time.
pub fn build_subagent_config(def: &AgentDef) -> SubagentConfig {
    SubagentConfig {
        system_prompt: def.system_prompt.clone(),
        tools: def.tools.clone(),
        max_iterations: def.max_iterations,
        max_context_tokens: def.max_context_tokens,
        model: if def.model.is_empty() {
            None
        } else {
            Some(def.model.clone())
        },
        skills: def.skills.clone(),
        permission_mode: if def.permission_mode.is_empty() {
            None
        } else {
            Some(def.permission_mode.clone())
        },
        memory_enabled: if def.memory_enabled == 0 {
            None
        } else {
            Some(def.memory_enabled)
        },
        temperature: None,
        result_strategy: crate::subagent::ResultStrategy::Auto,
        working_dir: None,
        recursion_depth: 0,
    }
}

/// Build a [`PermissionConfig`] from an [`AgentDef`] overlaid on a base config.
pub fn build_permission_config(def: &AgentDef, base: &PermissionConfig) -> PermissionConfig {
    let mut config = base.clone();
    config.mode = parse_permission_mode(&def.permission_mode).unwrap_or(config.mode);

    // Merge any custom rules declared on the agent definition.
    if let Some(rules) = parse_permission_rules(&def.permission_rules) {
        if !rules.is_empty() {
            config.rules.extend(rules);
        }
    }
    config
}

/// Build a [`ModelConfig`] from an [`AgentDef`], falling back to the config
/// default model when `def.model` is empty or unknown.
pub fn build_model_config(def: &AgentDef, config: &Config) -> ModelConfig {
    if !def.model.is_empty() {
        if let Some(mc) = config.models.get(&def.model) {
            return mc.clone();
        }
        tracing::warn!(
            "agent '{}' references unknown model '{}'; falling back to default",
            def.name,
            def.model
        );
    }
    config.default_model().cloned().unwrap_or_default()
}

/// Parse a permission-mode string (case-insensitive) into a [`PermissionMode`].
pub fn parse_permission_mode(s: &str) -> Option<PermissionMode> {
    match s.to_ascii_lowercase().as_str() {
        "paranoid" => Some(PermissionMode::Paranoid),
        "standard" => Some(PermissionMode::Standard),
        "developer" => Some(PermissionMode::Developer),
        "permissive" => Some(PermissionMode::Permissive),
        "yolo" => Some(PermissionMode::Yolo),
        _ => None,
    }
}

/// Parse a JSON value (array of rule objects) into [`ConfigRule`]s.
fn parse_permission_rules(value: &serde_json::Value) -> Option<Vec<ConfigRule>> {
    if value.is_array() {
        serde_json::from_value(value.clone()).ok()
    } else if value.is_null() {
        Some(Vec::new())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_identity_uses_group_for_isolation_and_agent_for_provenance() {
        let def = AgentDef {
            id: "agent-a".into(),
            memory_group: "shared-reviewers".into(),
            ..AgentDef::default()
        };
        let identity = def.memory_identity();
        assert_eq!(identity.agent_id(), "agent-a");
        assert_eq!(identity.memory_key(), "shared-reviewers");
    }
}
