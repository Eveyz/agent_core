//! History Hygiene Layer — cleans messages before they're sent to the model API.
//!
//! Operations performed on the request-boundary copy only (persistent history untouched):
//! 1. Truncate oversized tool results (keep head + tail + signal lines)
//! 2. Replace long tool arguments with placeholders
//!
//! This prevents context pollution from bloated tool outputs and keeps the
//! message prefix stable for prompt KV cache hits.
//!
//! Tool-result truncation delegates to [`policy`] (shared with
//! `compressor::snip_compact`) so the request view and the persisted-history
//! view never diverge. See PLAN-0008.

pub mod policy;

use crate::types::{Message, Role};

const TOOL_ARG_MAX_CHARS: usize = 200;

/// Run the full hygiene pass on a message list (mutates in place).
/// Returns the count of messages that were modified.
pub fn sanitize(messages: &mut Vec<Message>) -> usize {
    let mut modified = 0;
    for msg in messages.iter_mut() {
        if truncate_tool_result(msg) {
            modified += 1;
        }
        if truncate_tool_args(msg) {
            modified += 1;
        }
    }
    modified
}

/// Truncate an oversized tool result message.
///
/// Delegates to the shared `hygiene::policy` so L2 (here) and L3
/// (`compressor::snip_compact`) behave identically. See PLAN-0008.
fn truncate_tool_result(msg: &mut Message) -> bool {
    if msg.role != Role::Tool {
        return false;
    }
    let content = match &msg.content {
        Some(c) => c.clone(),
        None => return false,
    };
    match policy::truncate_content(msg.name.as_deref(), &content) {
        Some(truncated) => {
            msg.content = Some(truncated);
            true
        }
        None => false,
    }
}

/// Truncate long tool call arguments to a placeholder.
fn truncate_tool_args(msg: &mut Message) -> bool {
    if msg.role != Role::Assistant {
        return false;
    }
    let calls = match &mut msg.tool_calls {
        Some(c) => c,
        None => return false,
    };

    let mut modified = false;
    for tc in calls.iter_mut() {
        if tc.function.arguments.len() > TOOL_ARG_MAX_CHARS {
            tc.function.arguments = format!(
                "[args truncated: {} bytes]",
                tc.function.arguments.len()
            );
            modified = true;
        }
    }
    modified
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool_msg(content: &str) -> Message {
        Message {
            role: Role::Tool,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: Some("t1".into()),
            name: Some("test_tool".into()),
            model: None,
            metadata: None,
        }
    }

    fn make_assistant_with_args(args: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: Some("ok".into()),
            tool_calls: Some(vec![crate::types::ToolCall {
                id: "c1".into(),
                call_type: "function".into(),
                function: crate::types::FunctionCall {
                    name: "test".into(),
                    arguments: args.to_string(),
                },
            }]),
            tool_call_id: None,
            name: None,
            model: None,
            metadata: None,
        }
    }

    // Incidental output large enough to exceed the 16K char budget.
    fn big_incidental() -> String {
        (0..2000)
            .map(|i| format!("line number {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn truncate_large_tool_result() {
        let big = big_incidental();
        let mut msg = make_tool_msg(&big);
        assert!(truncate_tool_result(&mut msg));
        let c = msg.content.unwrap();
        assert!(c.contains("truncated"));
        assert!(c.len() < big.len());
    }

    #[test]
    fn skip_small_tool_result() {
        let small = "hello world";
        let mut msg = make_tool_msg(small);
        assert!(!truncate_tool_result(&mut msg));
        assert_eq!(msg.content.unwrap(), small);
    }

    #[test]
    fn preserves_error_signals() {
        let mut lines: Vec<String> = (0..2000).map(|i| format!("line {}", i)).collect();
        lines.push("Error: something went wrong".to_string());
        lines.push("exit code: 1".to_string());
        let big = lines.join("\n");
        let mut msg = make_tool_msg(&big);
        truncate_tool_result(&mut msg);
        let c = msg.content.unwrap();
        assert!(c.contains("Error: something went wrong"));
        assert!(c.contains("exit code: 1"));
    }

    #[test]
    fn truncate_long_tool_args() {
        let long_args = "x".repeat(500);
        let mut msg = make_assistant_with_args(&long_args);
        assert!(truncate_tool_args(&mut msg));
        let args = &msg.tool_calls.unwrap()[0].function.arguments;
        assert!(args.contains("truncated"));
        assert!(args.len() < long_args.len());
    }

    #[test]
    fn skip_short_tool_args() {
        let short = r#"{"cmd": "ls"}"#;
        let mut msg = make_assistant_with_args(short);
        assert!(!truncate_tool_args(&mut msg));
    }

    #[test]
    fn sanitize_returns_modified_count() {
        let big = big_incidental();
        let mut msgs = vec![make_tool_msg(&big), make_tool_msg("ok")];
        let n = sanitize(&mut msgs);
        assert_eq!(n, 1);
    }

    #[test]
    fn skip_truncation_for_skill_load() {
        // Instruction-class tools are never truncated regardless of size.
        let big: String = (0..2000)
            .map(|i| format!("instruction line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut msg = Message {
            role: Role::Tool,
            content: Some(big.clone()),
            tool_calls: None,
            tool_call_id: Some("t1".into()),
            name: Some("skill_load".into()),
            model: None,
            metadata: None,
        };
        assert!(!truncate_tool_result(&mut msg));
        assert_eq!(msg.content.unwrap(), big);
    }

    #[test]
    fn non_truncatable_tools_not_counted() {
        let big = big_incidental();
        let skill_msg = Message {
            role: Role::Tool,
            content: Some(big.clone()),
            tool_calls: None,
            tool_call_id: Some("t1".into()),
            name: Some("skill_load".into()),
            model: None,
            metadata: None,
        };
        let normal_msg = make_tool_msg(&big); // name: "test_tool" → Incidental → truncated
        let mut msgs = vec![skill_msg, normal_msg];
        let n = sanitize(&mut msgs);
        // Only normal_msg should be truncated (skill_load skipped)
        assert_eq!(n, 1);
    }

    #[test]
    fn actively_read_tool_exempts_head_tail() {
        // read_file is ActivelyRead: under the 24K cap it is left untouched even
        // though it would be head/tail-split as Incidental.
        let content = "line\n".repeat(30000); // ~150K chars, 30000 lines
        let mut msg = Message {
            role: Role::Tool,
            content: Some(content.clone()),
            tool_calls: None,
            tool_call_id: Some("t1".into()),
            name: Some("read_file".into()),
            model: None,
            metadata: None,
        };
        assert!(truncate_tool_result(&mut msg));
        let c = msg.content.unwrap();
        // Char-capped (not head/tail-split): no signal section.
        assert!(c.contains("truncated"));
        assert!(!c.contains("--- signals ---"));
    }
}
