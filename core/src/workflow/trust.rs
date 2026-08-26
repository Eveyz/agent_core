//! Workflow-level trust mode — overrides per-agent permission posture.
//!
//! `Inherit` leaves each agent's own `permission_mode` in place. `Trusted`
//! forces Yolo (all tools auto-allowed) so an automation workflow isn't
//! blocked by approval prompts. `Readonly` restricts to ReadOnly-level tools.

use crate::agent_registry::AgentDef;
use crate::permission::{DangerLevel, PermissionConfig, PermissionMode};
use crate::workflow::definition::TrustMode;

impl TrustMode {
    /// Build a [`PermissionConfig`] for an agent, applying the workflow's
    /// trust posture as an override on top of the agent's own config.
    pub fn build_permission_config(
        &self,
        base: &PermissionConfig,
        agent_def: &AgentDef,
    ) -> PermissionConfig {
        let mut config = crate::agent_registry::build_permission_config(agent_def, base);
        match self {
            TrustMode::Trusted => {
                config.mode = PermissionMode::Yolo;
                config.auto_allow_up_to = Some(DangerLevel::Destructive);
            }
            TrustMode::Readonly => {
                config.mode = PermissionMode::Paranoid;
                config.auto_allow_up_to = Some(DangerLevel::ReadOnly);
            }
            TrustMode::Inherit => {
                // Keep the agent's own permission_mode (already applied above).
            }
        }
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_registry::AgentDef;

    fn make_agent(permission_mode: &str) -> AgentDef {
        AgentDef {
            id: "test-agent".into(),
            name: "Test".into(),
            description: String::new(),
            system_prompt: String::new(),
            model: String::new(),
            skills: vec![],
            tools: vec![],
            permission_mode: permission_mode.into(),
            permission_rules: serde_json::Value::Array(vec![]),
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

    #[test]
    fn trusted_mode_forces_yolo() {
        let agent = make_agent("paranoid");
        let base = PermissionConfig::default();
        let config = TrustMode::Trusted.build_permission_config(&base, &agent);
        assert_eq!(config.mode, PermissionMode::Yolo);
        assert_eq!(config.auto_allow_up_to, Some(DangerLevel::Destructive));
    }

    #[test]
    fn readonly_mode_forces_paranoid() {
        let agent = make_agent("yolo");
        let base = PermissionConfig::default();
        let config = TrustMode::Readonly.build_permission_config(&base, &agent);
        assert_eq!(config.mode, PermissionMode::Paranoid);
        assert_eq!(config.auto_allow_up_to, Some(DangerLevel::ReadOnly));
    }

    #[test]
    fn inherit_mode_keeps_agent_permission() {
        let agent = make_agent("developer");
        let base = PermissionConfig::default();
        let config = TrustMode::Inherit.build_permission_config(&base, &agent);
        assert_eq!(config.mode, PermissionMode::Developer);
    }

    #[test]
    fn trusted_overrides_even_yolo_agent() {
        // Even if the agent is already yolo, Trusted should still set
        // auto_allow_up_to to Destructive explicitly.
        let agent = make_agent("yolo");
        let base = PermissionConfig::default();
        let config = TrustMode::Trusted.build_permission_config(&base, &agent);
        assert_eq!(config.mode, PermissionMode::Yolo);
        assert_eq!(config.auto_allow_up_to, Some(DangerLevel::Destructive));
    }

    #[test]
    fn readonly_overrides_permissive_agent() {
        let agent = make_agent("permissive");
        let base = PermissionConfig::default();
        let config = TrustMode::Readonly.build_permission_config(&base, &agent);
        assert_eq!(config.mode, PermissionMode::Paranoid);
        assert_eq!(config.auto_allow_up_to, Some(DangerLevel::ReadOnly));
    }
}
