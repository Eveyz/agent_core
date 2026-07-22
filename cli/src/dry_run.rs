//! Hook that vetoes all tool execution for `--dry-run`.

use agent_core::{Hook, HookAction, HookEvent};

/// Veto every `PreToolUse` so the LLM still runs but tools have no side effects.
/// The veto reason is returned to the model as the tool result (`Hook vetoed: …`).
pub struct DryRunHook;

impl Hook for DryRunHook {
    fn name(&self) -> &str {
        "dry_run"
    }

    fn handle(&self, event: &HookEvent) -> Option<HookAction> {
        match event {
            HookEvent::PreToolUse { tool_name, input } => {
                let preview = serde_json::to_string(input).unwrap_or_else(|_| "{}".into());
                let truncated = if preview.len() > 200 {
                    format!("{}…", &preview[..preview.floor_char_boundary(200)])
                } else {
                    preview
                };
                Some(HookAction::Veto(format!(
                    "[dry-run] skipped `{tool_name}` args={truncated}"
                )))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn vetoes_pre_tool_use() {
        let hook = DryRunHook;
        let action = hook
            .handle(&HookEvent::PreToolUse {
                tool_name: "bash".into(),
                input: json!({"cmd": "rm -rf /"}),
            })
            .expect("action");
        match action {
            HookAction::Veto(reason) => {
                assert!(reason.contains("[dry-run]"));
                assert!(reason.contains("bash"));
            }
            other => panic!("expected Veto, got {other:?}"),
        }
    }
}
