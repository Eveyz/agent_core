use serde_json::Value;

#[derive(Debug, Clone)]
pub enum HookEvent {
    PreToolUse {
        tool_name: String,
        input: Value,
    },
    PostToolUse {
        tool_name: String,
        input: Value,
        output: String,
    },
    SessionStart {
        session_id: String,
    },
    SessionEnd {
        session_id: String,
    },
}

#[derive(Debug, Clone)]
pub enum HookAction {
    Continue,
    Veto(String),
    ModifyInput(Value),
    ModifyOutput(String),
}

pub trait Hook: Send + Sync {
    fn name(&self) -> &str;
    fn handle(&self, event: &HookEvent) -> Option<HookAction>;
}

pub struct HookRegistry {
    hooks: Vec<Box<dyn Hook>>,
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn register(&mut self, hook: Box<dyn Hook>) {
        self.hooks.push(hook);
    }

    pub fn fire_pre_tool_use(&self, tool_name: &str, input: &Value) -> PreToolResult {
        let event = HookEvent::PreToolUse {
            tool_name: tool_name.to_string(),
            input: input.clone(),
        };

        let mut current_input = input.clone();

        for hook in &self.hooks {
            if let Some(action) = hook.handle(&event) {
                match action {
                    HookAction::Veto(reason) => return PreToolResult::Veto(reason),
                    HookAction::ModifyInput(new_input) => {
                        current_input = new_input;
                    }
                    HookAction::Continue => {}
                    HookAction::ModifyOutput(_) => {}
                }
            }
        }

        PreToolResult::Proceed(current_input)
    }

    pub fn fire_post_tool_use(
        &self,
        tool_name: &str,
        input: &Value,
        output: &str,
    ) -> String {
        let event = HookEvent::PostToolUse {
            tool_name: tool_name.to_string(),
            input: input.clone(),
            output: output.to_string(),
        };

        let mut current_output = output.to_string();

        for hook in &self.hooks {
            if let Some(action) = hook.handle(&event) {
                match action {
                    HookAction::ModifyOutput(new_output) => {
                        current_output = new_output;
                    }
                    HookAction::Continue => {}
                    _ => {}
                }
            }
        }

        current_output
    }

    pub fn fire_session_start(&self, session_id: &str) {
        let event = HookEvent::SessionStart {
            session_id: session_id.to_string(),
        };
        for hook in &self.hooks {
            hook.handle(&event);
        }
    }

    pub fn fire_session_end(&self, session_id: &str) {
        let event = HookEvent::SessionEnd {
            session_id: session_id.to_string(),
        };
        for hook in &self.hooks {
            hook.handle(&event);
        }
    }

    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }
}

#[derive(Debug)]
pub enum PreToolResult {
    Proceed(Value),
    Veto(String),
}

pub struct LoggingHook;

impl Hook for LoggingHook {
    fn name(&self) -> &str {
        "logging"
    }

    fn handle(&self, event: &HookEvent) -> Option<HookAction> {
        match event {
            HookEvent::PreToolUse { tool_name, input } => {
                eprintln!("[hook] pre_tool_use: {} input={}", tool_name, input);
            }
            HookEvent::PostToolUse {
                tool_name,
                output,
                ..
            } => {
                let preview = if output.len() > 200 {
                    let safe_end = output.floor_char_boundary(200);
                    format!("{}...", &output[..safe_end])
                } else {
                    output.clone()
                };
                eprintln!("[hook] post_tool_use: {} output={}", tool_name, preview);
            }
            HookEvent::SessionStart { session_id } => {
                eprintln!("[hook] session_start: {}", session_id);
            }
            HookEvent::SessionEnd { session_id } => {
                eprintln!("[hook] session_end: {}", session_id);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct VetoHook;

    impl Hook for VetoHook {
        fn name(&self) -> &str {
            "veto"
        }
        fn handle(&self, event: &HookEvent) -> Option<HookAction> {
            match event {
                HookEvent::PreToolUse { tool_name, .. } if tool_name == "dangerous" => {
                    Some(HookAction::Veto("blocked".to_string()))
                }
                _ => None,
            }
        }
    }

    struct ModifyInputHook;

    impl Hook for ModifyInputHook {
        fn name(&self) -> &str {
            "modify_input"
        }
        fn handle(&self, event: &HookEvent) -> Option<HookAction> {
            match event {
                HookEvent::PreToolUse { tool_name, .. } if tool_name == "add_flag" => {
                    Some(HookAction::ModifyInput(json!({"flag": true})))
                }
                _ => None,
            }
        }
    }

    #[test]
    fn test_veto_hook() {
        let mut registry = HookRegistry::new();
        registry.register(Box::new(VetoHook));

        let result = registry.fire_pre_tool_use("dangerous", &json!({}));
        assert!(matches!(result, PreToolResult::Veto(_)));
    }

    #[test]
    fn test_modify_input_hook() {
        let mut registry = HookRegistry::new();
        registry.register(Box::new(ModifyInputHook));

        let result = registry.fire_pre_tool_use("add_flag", &json!({}));
        match result {
            PreToolResult::Proceed(val) => assert_eq!(val, json!({"flag": true})),
            _ => panic!("expected Proceed"),
        }
    }

    #[test]
    fn test_no_hooks_passes_through() {
        let registry = HookRegistry::new();
        let input = json!({"key": "value"});
        let result = registry.fire_pre_tool_use("any_tool", &input);
        match result {
            PreToolResult::Proceed(val) => assert_eq!(val, input),
            _ => panic!("expected Proceed"),
        }
    }
}
