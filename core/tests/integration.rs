use agent_core::{
    Brain, Config, ContextEngine, FunctionCall, Message, PermissionPolicy, ReasoningState, Role,
    ToolCall, ToolExecutionMode,
};

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
    assert_eq!(brain.tool_execution_mode(), ToolExecutionMode::Sequential);
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

#[test]
fn active_react_context_survives_compaction_and_assembly() {
    let mut context = ContextEngine::new("identity", 8_000);
    context.add(Message::user("old completed task"));
    context.add(Message::assistant_with_tools(
        "",
        vec![ToolCall {
            id: "old_call".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "exec".into(),
                arguments: "{}".into(),
            },
        }],
    ));
    context.add(Message::tool(
        "old_call".into(),
        "old-result".repeat(6_000),
        Some("exec".into()),
    ));
    context.add(Message::user("inspect every result and finish the task"));
    for i in 0..6 {
        let id = format!("call_{i}");
        context.add(
            Message::assistant_with_tools(
                "",
                vec![ToolCall {
                    id: id.clone(),
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: "exec".into(),
                        arguments: "{}".into(),
                    },
                }],
            )
            .with_reasoning(ReasoningState::from_text(format!("reasoning-{i}"))),
        );
        context.add(Message::tool(
            id,
            format!("tool-result-{i}"),
            Some("exec".into()),
        ));
    }
    let compacted = context.trim_to_fit();

    assert!(!compacted.stages_ran.is_empty());
    let outbound = context.messages();
    let active_start = outbound
        .iter()
        .position(|message| {
            message.role == Role::User
                && message.content.as_deref() == Some("inspect every result and finish the task")
        })
        .expect("active user task must survive compaction");
    for i in 0..6 {
        let assistant = &outbound[active_start + 1 + i * 2];
        let tool = &outbound[active_start + 2 + i * 2];
        let expected_reasoning = format!("reasoning-{i}");
        let expected_result = format!("tool-result-{i}");
        assert_eq!(
            assistant
                .reasoning
                .as_ref()
                .and_then(|reasoning| reasoning.text.as_deref()),
            Some(expected_reasoning.as_str())
        );
        assert_eq!(tool.content.as_deref(), Some(expected_result.as_str()));
    }
}
