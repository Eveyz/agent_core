//! Integration tests for the full agent pipeline.
//!
//! These tests build real Agent instances and verify properties of the
//! builder, config, context, and pipeline without needing a real LLM.

use agent_core::{AgentBuilder, Config, PermissionPolicy, ToolExecutionMode};

// ── Helpers ──────────────────────────────────────────────────────────

fn build_test_config() -> Config {
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

// ── Tests ────────────────────────────────────────────────────────────

#[test]
fn test_agent_builds_with_defaults() {
    let config = build_test_config();
    let agent = AgentBuilder::with_config(config)
        .with_memory(false)
        .build()
        .unwrap();

    assert_eq!(agent.current_model(), "test");
    let state_str = format!("{:?}", agent.state());
    assert!(state_str.contains("Idle"), "Expected Idle, got: {}", state_str);
}

#[test]
fn test_agent_builder_sets_all_options() {
    let config = build_test_config();
    let agent = AgentBuilder::with_config(config)
        .with_system_prompt("You are a test agent.")
        .with_permission_policy(
            PermissionPolicy::with_builtin_defaults()
                .with_mode(agent_core::PermissionMode::Paranoid),
        )
        .with_memory(false)
        .with_tool_execution_mode(ToolExecutionMode::Sequential)
        .build()
        .unwrap();

    assert_eq!(agent.tool_execution_mode(), ToolExecutionMode::Sequential);
}

#[test]
fn test_agent_builder_defaults() {
    let config = build_test_config();
    let agent = AgentBuilder::with_config(config)
        .with_memory(false)
        .build()
        .unwrap();

    assert_eq!(agent.tool_execution_mode(), ToolExecutionMode::Parallel);
}

#[test]
fn test_agent_with_permission_policy() {
    let config = build_test_config();
    let agent = AgentBuilder::with_config(config)
        .with_permission_policy(
            PermissionPolicy::with_builtin_defaults()
                .with_mode(agent_core::PermissionMode::Yolo),
        )
        .with_memory(false)
        .build()
        .unwrap();

    // Verify the permission policy is Yolo
    // (we can't easily inspect the policy, but build succeeds)
    assert!(format!("{:?}", agent.state()).contains("Idle"));
}

#[test]
fn test_agent_with_skill_manager() {
    let config = build_test_config();
    let skill_manager = agent_core::SkillManager::with_defaults();

    let agent = AgentBuilder::with_config(config)
        .with_skill_manager(std::sync::Arc::new(std::sync::Mutex::new(skill_manager)))
        .with_memory(false)
        .build()
        .unwrap();

    assert_eq!(agent.current_model(), "test");
}

#[test]
fn test_agent_abort_flag() {
    let config = build_test_config();
    let agent = AgentBuilder::with_config(config)
        .with_memory(false)
        .build()
        .unwrap();

    assert!(!agent.abort_flag.load(std::sync::atomic::Ordering::Relaxed));
    agent.abort_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(agent.abort_flag.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn test_agent_context_initialized() {
    let config = build_test_config();
    let agent = AgentBuilder::with_config(config)
        .with_memory(false)
        .build()
        .unwrap();

    // Context should have messages after build (system + environment segments)
    let messages = agent.context_messages();
    assert!(!messages.is_empty(), "Context should have messages after build");
}

#[test]
fn test_agent_from_config_file() {
    // Test loading from the actual config.toml
    let result = AgentBuilder::from_config("config.toml");
    assert!(result.is_ok(), "Failed to load config.toml: {:?}", result.err());
}

#[test]
fn test_agent_tool_registry() {
    let config = build_test_config();
    let agent = AgentBuilder::with_config(config)
        .with_tool(StubTool)
        .with_memory(false)
        .build()
        .unwrap();

    let tools = agent.tool_registry().list_names();
    assert!(tools.contains(&"stub_test"), "Tool registry missing stub: {:?}", tools);
    assert!(agent.tool_registry().has("stub_test"));
}

#[test]
fn test_permission_policy_modes() {
    let yolo = PermissionPolicy::with_builtin_defaults()
        .with_mode(agent_core::PermissionMode::Yolo);
    let paranoid = PermissionPolicy::with_builtin_defaults()
        .with_mode(agent_core::PermissionMode::Paranoid);
    let standard = PermissionPolicy::with_builtin_defaults()
        .with_mode(agent_core::PermissionMode::Standard);
    let permissive = PermissionPolicy::with_builtin_defaults()
        .with_mode(agent_core::PermissionMode::Permissive);

    // Just verify they build
    drop(yolo);
    drop(paranoid);
    drop(standard);
    drop(permissive);
}

#[test]
fn test_clears_context() {
    let config = build_test_config();
    let mut agent = AgentBuilder::with_config(config)
        .with_memory(false)
        .build()
        .unwrap();

    let before = agent.context_messages().len();
    agent.clear_context();
    // After clear, only system-generated segments remain
    let after = agent.context_messages().len();
    assert!(after <= before, "Context should not grow after clear");
}

// ── Stub tool for testing ────────────────────────────────────────────

struct StubTool;

#[async_trait::async_trait]
impl agent_core::Tool for StubTool {
    fn name(&self) -> &str { "stub_test" }
    fn description(&self) -> &str { "A stub tool for testing" }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        Ok("stub result".to_string())
    }
}
