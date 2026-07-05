use parking_lot::Mutex;
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
    /// Fired at the start of each turn (before context refresh).
    TurnStart {
        turn_index: usize,
    },
    /// Fired at the end of each turn.
    TurnEnd {
        turn_index: usize,
    },
    /// Fired just before the LLM is invoked; carries a serialized snapshot
    /// of the outgoing messages so hooks can inspect/modify without borrowing
    /// the agent.
    BeforeModel {
        messages: Vec<Value>,
    },
    /// Fired just after the LLM stream completes successfully.
    AfterModel {
        text: String,
        tool_call_count: usize,
    },
}

#[derive(Debug, Clone)]
pub enum HookAction {
    Continue,
    Veto(String),
    ModifyInput(Value),
    ModifyOutput(String),
    /// Only valid in response to `BeforeModel`: skip the actual LLM call and
    /// use `preset_text` as the assistant response (no tool calls). Useful for
    /// testing, caching, or short-circuiting.
    SkipModel {
        preset_text: String,
    },
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
                    HookAction::SkipModel { .. } => {}
                }
            }
        }

        PreToolResult::Proceed(current_input)
    }

    pub fn fire_post_tool_use(&self, tool_name: &str, input: &Value, output: &str) -> String {
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

    /// Fire all hooks for `TurnStart`.
    pub fn fire_turn_start(&self, turn_index: usize) {
        let event = HookEvent::TurnStart { turn_index };
        for hook in &self.hooks {
            hook.handle(&event);
        }
    }

    /// Fire all hooks for `TurnEnd`.
    pub fn fire_turn_end(&self, turn_index: usize) {
        let event = HookEvent::TurnEnd { turn_index };
        for hook in &self.hooks {
            hook.handle(&event);
        }
    }

    /// Fire all `BeforeModel` hooks. Returns `Some(preset_text)` if any hook
    /// requested the LLM call be skipped with a preset response.
    pub fn fire_before_model(&self, messages: &[Value]) -> Option<String> {
        let event = HookEvent::BeforeModel {
            messages: messages.to_vec(),
        };
        for hook in &self.hooks {
            if let Some(action) = hook.handle(&event) {
                if let HookAction::SkipModel { preset_text } = action {
                    return Some(preset_text);
                }
            }
        }
        None
    }

    /// Fire all `AfterModel` hooks.
    pub fn fire_after_model(&self, text: &str, tool_call_count: usize) {
        let event = HookEvent::AfterModel {
            text: text.to_string(),
            tool_call_count,
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
                tracing::debug!(%tool_name, %input, "pre_tool_use");
            }
            HookEvent::PostToolUse {
                tool_name, output, ..
            } => {
                let preview = if output.len() > 200 {
                    let safe_end = output.floor_char_boundary(200);
                    format!("{}...", &output[..safe_end])
                } else {
                    output.clone()
                };
                tracing::debug!(tool_name, %preview, "post_tool_use");
            }
            HookEvent::SessionStart { session_id } => {
                tracing::info!(session_id, "session_start");
            }
            HookEvent::SessionEnd { session_id } => {
                tracing::info!(session_id, "session_end");
            }
            HookEvent::TurnStart { turn_index } => {
                tracing::debug!(turn_index, "turn_start");
            }
            HookEvent::TurnEnd { turn_index } => {
                tracing::debug!(turn_index, "turn_end");
            }
            HookEvent::BeforeModel { messages } => {
                tracing::debug!(message_count = messages.len(), "before_model");
            }
            HookEvent::AfterModel {
                text,
                tool_call_count,
            } => {
                tracing::debug!(tool_call_count, chars = text.len(), "after_model");
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

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

    /// A hook that short-circuits the LLM with a preset response.
    struct SkipModelHook;

    impl Hook for SkipModelHook {
        fn name(&self) -> &str {
            "skip_model"
        }
        fn handle(&self, event: &HookEvent) -> Option<HookAction> {
            match event {
                HookEvent::BeforeModel { .. } => Some(HookAction::SkipModel {
                    preset_text: "preset answer".to_string(),
                }),
                _ => None,
            }
        }
    }

    #[test]
    fn test_before_model_skip() {
        let mut registry = HookRegistry::new();
        registry.register(Box::new(SkipModelHook));

        let result = registry.fire_before_model(&[json!({"role": "user"})]);
        assert_eq!(result.as_deref(), Some("preset answer"));
    }

    #[test]
    fn test_before_model_no_hooks_returns_none() {
        let registry = HookRegistry::new();
        let result = registry.fire_before_model(&[json!({"role": "user"})]);
        assert!(result.is_none());
    }

    /// Records which lifecycle hooks fired, in order. Uses a shared
    /// `Arc<Mutex<Vec>>` so the recorded events remain readable after the hook
    /// is moved into the registry.
    #[derive(Default, Clone)]
    struct RecordingHook {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl Hook for RecordingHook {
        fn name(&self) -> &str {
            "recording"
        }
        fn handle(&self, event: &HookEvent) -> Option<HookAction> {
            let label = match event {
                HookEvent::SessionStart { .. } => "session_start",
                HookEvent::SessionEnd { .. } => "session_end",
                HookEvent::TurnStart { .. } => "turn_start",
                HookEvent::TurnEnd { .. } => "turn_end",
                HookEvent::BeforeModel { .. } => "before_model",
                HookEvent::AfterModel { .. } => "after_model",
                HookEvent::PreToolUse { .. } => "pre_tool_use",
                HookEvent::PostToolUse { .. } => "post_tool_use",
            };
            self.events.lock().push(label.to_string());
            None
        }
    }

    #[test]
    fn test_lifecycle_hooks_fire() {
        let hook = RecordingHook::default();
        let events = hook.events.clone();
        let mut registry = HookRegistry::new();
        registry.register(Box::new(hook));

        registry.fire_session_start("s1");
        registry.fire_turn_start(0);
        registry.fire_before_model(&[json!({})]);
        registry.fire_after_model("hi", 0);
        registry.fire_turn_end(0);
        registry.fire_session_end("s1");

        assert_eq!(
            events.lock().as_slice(),
            [
                "session_start",
                "turn_start",
                "before_model",
                "after_model",
                "turn_end",
                "session_end",
            ]
        );
    }
}
