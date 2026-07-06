use agent_core::{Brain, Config, PermissionPolicy, ToolExecutionMode};

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

#[test]
fn test_brain_builds_with_defaults() {
    let config = build_test_config();
    let brain = Brain::from_config(config).unwrap();

    assert_eq!(brain.current_model_name(), "test/default");
}

#[test]
fn test_brain_sets_all_options() {
    let config = build_test_config();
    let mut brain = Brain::from_config(config).unwrap();
    brain.set_tool_execution_mode(ToolExecutionMode::Sequential);
    assert_eq!(brain.tool_execution_mode, ToolExecutionMode::Sequential);
}

#[test]
fn test_brain_tool_registry() {
    let config = build_test_config();
    let brain = Brain::from_config(config).unwrap();

    let registry = brain.build_tool_registry(agent_core::AgentMode::Build);
    let tools = registry.list_names();
    assert!(
        tools.contains(&"read_file"),
        "Tool registry missing read_file: {:?}",
        tools
    );
}

#[test]
fn test_permission_policy_modes() {
    let yolo =
        PermissionPolicy::with_builtin_defaults().with_mode(agent_core::PermissionMode::Yolo);
    let paranoid =
        PermissionPolicy::with_builtin_defaults().with_mode(agent_core::PermissionMode::Paranoid);
    let standard =
        PermissionPolicy::with_builtin_defaults().with_mode(agent_core::PermissionMode::Standard);
    let permissive =
        PermissionPolicy::with_builtin_defaults().with_mode(agent_core::PermissionMode::Permissive);

    // Just verify they build
    drop(yolo);
    drop(paranoid);
    drop(standard);
    drop(permissive);
}
