//! Brain — the reusable, stateless part of the agent.
//!
//! A Brain is constructed once (from config) and shared across all Runs.
//! It holds everything that does NOT change per-request:
//! - LLM client configuration
//! - Tool factory (each Run builds its own registry)
//! - Memory manager (shared SQLite + embeddings)
//! - Skill manager (shared catalog)
//! - Recovery engine (shared strategy)
//!
//! It does NOT hold: context, cancel tokens, permissions (per-Run copy),
//! hooks (per-Run), or any mutable execution state.

use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};

use crate::client::OpenAIClient;
use crate::config::{Config, ModelConfig};
use crate::error_recovery::RecoveryEngine;
use crate::hooks::HookRegistry;
use crate::memory::MemoryManager;
use crate::permission::{PermissionConfig, PermissionPolicy};
use crate::prompt;
use crate::skills::SkillManager;
use crate::tools::ToolRegistry;

/// The reusable "brain" shared by all Runs.
///
/// Cloneable via `Arc` — each Run holds an `Arc<Brain>`.
/// Contains only immutable/shared state; per-request state lives in [`crate::runtime::Run`].
pub struct Brain {
    /// The full configuration (models, permissions, memory, mcp).
    pub config: Config,
    /// Shared memory manager (if memory is enabled).
    pub memory: Option<Arc<Mutex<MemoryManager>>>,
    /// Shared skill manager (if skills are enabled).
    pub skill_manager: Option<Arc<Mutex<SkillManager>>>,
    /// The currently active model name (e.g. "openai/gpt-4o").
    /// Switching the model updates this; new Runs use the new model.
    current_model_name: String,
}

impl Brain {
    /// Build a Brain from a loaded Config.
    pub fn from_config(config: Config) -> Result<Self> {
        let memory = Self::build_memory(&config)?;
        let skill_manager = Self::build_skill_manager(&config)?;

        let current_model_name = config.default_model.clone();

        Ok(Self {
            config,
            memory,
            skill_manager,
            current_model_name,
        })
    }

    /// Build a Brain by loading config from a file path.
    pub fn load_config(path: &str) -> Result<Self> {
        let config = Config::load(path).with_context(|| format!("failed to load config: {path}"))?;
        Self::from_config(config)
    }

    fn build_memory(config: &Config) -> Result<Option<Arc<Mutex<MemoryManager>>>> {
        if let Some(ref mem_config) = config.memory {
            let m = MemoryManager::new(
                &mem_config.db_path,
                &mem_config.embedding_model,
                mem_config.default_block_max_chars,
            )?;
            Ok(Some(Arc::new(Mutex::new(m))))
        } else {
            Ok(None)
        }
    }

    fn build_skill_manager(_config: &Config) -> Result<Option<Arc<Mutex<SkillManager>>>> {
        // Skill manager is optional and requires a skills directory.
        // For now, return None — can be wired up via builder.
        Ok(None)
    }

    /// Get the currently active model's full config.
    pub fn current_model_config(&self) -> Result<ModelConfig> {
        self.config
            .get_model(&self.current_model_name)
            .cloned()
            .with_context(|| {
                format!(
                    "current model '{}' not found in config",
                    self.current_model_name
                )
            })
    }

    /// The currently active model name.
    pub fn current_model_name(&self) -> &str {
        &self.current_model_name
    }

    /// Switch the active model. New Runs will use this model.
    pub fn switch_model(&mut self, name: &str) -> Result<()> {
        if self.config.get_model(name).is_none() {
            anyhow::bail!("model '{name}' not found in config");
        }
        self.current_model_name = name.to_string();
        Ok(())
    }

    /// Build an OpenAIClient for the current model (with fallback chain).
    ///
    /// Each Run calls this to get its own client instance. The client is
    /// cheap to construct (shares an HTTP connection pool internally).
    pub fn build_client(&self) -> Result<OpenAIClient> {
        let model_config = self.current_model_config()?;

        // Build fallback chain (up to 3 levels deep).
        let mut current = model_config.clone();
        let mut fallbacks = Vec::new();
        for _ in 0..3 {
            if let Some(ref fallback_name) = current.fallback_model {
                if let Some(fallback_cfg) = self.config.get_model(fallback_name) {
                    fallbacks.push(fallback_cfg.clone());
                    current = fallback_cfg.clone();
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        let mut client_opt: Option<OpenAIClient> = None;
        for fallback_cfg in fallbacks.into_iter().rev() {
            if let Some(child) = client_opt {
                client_opt = Some(OpenAIClient::with_fallback(fallback_cfg, Some(child)));
            } else {
                client_opt = Some(OpenAIClient::new(fallback_cfg));
            }
        }

        let client = if let Some(child) = client_opt {
            OpenAIClient::with_fallback(model_config, Some(child))
        } else {
            OpenAIClient::new(model_config)
        };

        Ok(client)
    }

    /// Build a fresh ToolRegistry for a Run.
    ///
    /// Each Run gets its own registry so MCP tools, memory tools, and
    /// subagent tools can be registered per-Run without interference.
    pub fn build_tool_registry(&self) -> ToolRegistry {
        let mut registry = ToolRegistry::with_defaults();

        if let Some(ref mem) = self.memory {
            crate::tools::register_memory_tools(&mut registry, mem.clone());
        }

        if let Ok(model_config) = self.current_model_config() {
            let available_tools: Vec<String> = registry.list_names().into_iter().map(|s| s.to_string()).collect();
            crate::tools::subagent::register_subagent_tools(
                &mut registry,
                model_config,
                available_tools,
                None,
                self.config.permissions.clone(),
            );
        }

        registry
    }

    /// Build a fresh PermissionPolicy for a Run (from config defaults).
    pub fn build_permission_policy(&self) -> PermissionPolicy {
        PermissionPolicy::with_builtin_defaults().with_config(&self.config.permissions)
    }

    /// The recovery engine (stateless strategy, shared).
    pub fn build_recovery(&self) -> RecoveryEngine {
        RecoveryEngine::default()
    }

    /// The identity text for the system prompt.
    pub fn identity_text(&self) -> String {
        if let Ok(model_config) = self.current_model_config() {
            if let Some(ref custom) = model_config.system_prompt {
                return custom.clone();
            }
        }
        prompt::DEFAULT_IDENTITY.to_string()
    }

    /// The principles text for the system prompt.
    pub fn principles_text(&self, permission_mode: &str) -> String {
        format!(
            "{}\n\nPermission Mode: {} — tools may require user approval before execution.",
            prompt::DEFAULT_PRINCIPLES, permission_mode
        )
    }

    /// Build a default HookRegistry (empty — Runs add their own hooks).
    pub fn build_hooks(&self) -> HookRegistry {
        HookRegistry::new()
    }

    /// The permission config (for subagent construction).
    pub fn permission_config(&self) -> &PermissionConfig {
        &self.config.permissions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        let toml = r#"
default_model = "test/default"

[providers.test]
name = "test"
base_url = "http://127.0.0.1:1"
api_key = "sk-test"

[providers.test.models]
default = { model_id = "mock" }
"#;
        let mut config: Config = toml::from_str(toml).unwrap();
        config.rebuild_models();
        config
    }

    #[test]
    fn brain_builds_from_config() {
        let brain = Brain::from_config(test_config()).unwrap();
        assert_eq!(brain.current_model_name(), "test/default");
    }

    #[test]
    fn brain_builds_client() {
        let brain = Brain::from_config(test_config()).unwrap();
        let _client = brain.build_client().unwrap();
    }

    #[test]
    fn brain_builds_tool_registry() {
        let brain = Brain::from_config(test_config()).unwrap();
        let registry = brain.build_tool_registry();
        assert!(registry.has("read_file"));
        assert!(registry.has("bash"));
    }

    #[test]
    fn brain_switch_model() {
        let mut brain = Brain::from_config(test_config()).unwrap();
        // Switching to the same model should work.
        brain.switch_model("test/default").unwrap();
        // Switching to a nonexistent model should fail.
        assert!(brain.switch_model("nonexistent").is_err());
    }
}
