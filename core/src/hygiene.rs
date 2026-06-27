//! History Hygiene Layer — cleans messages before they're sent to the model API.
//!
//! Operations performed on the request-boundary copy only (persistent history untouched):
//! 1. Truncate oversized tool results (keep head + tail + signal lines)
//! 2. Replace long tool arguments with placeholders
//!
//! This prevents context pollution from bloated tool outputs and keeps the
//! message prefix stable for prompt KV cache hits.

use crate::types::{Message, Role};

const TOOL_RESULT_MAX_CHARS: usize = 4000;
const TOOL_RESULT_HEAD_LINES: usize = 15;
const TOOL_RESULT_TAIL_LINES: usize = 8;
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

/// Truncate an oversized tool result message to head + tail + signal lines.
fn truncate_tool_result(msg: &mut Message) -> bool {
    if msg.role != Role::Tool {
        return false;
    }
    let content = match &msg.content {
        Some(c) if c.len() > TOOL_RESULT_MAX_CHARS => c,
        _ => return false,
    };

    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= TOOL_RESULT_HEAD_LINES + TOOL_RESULT_TAIL_LINES {
        return false;
    }

    let head: Vec<&str> = lines.iter().take(TOOL_RESULT_HEAD_LINES).copied().collect();
    let tail: Vec<&str> = lines
        .iter()
        .rev()
        .take(TOOL_RESULT_TAIL_LINES)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    let signals: Vec<&str> = lines
        .iter()
        .filter(|l| {
            let lower = l.to_lowercase();
            lower.contains("error")
                || lower.contains("exit code")
                || lower.contains("warning")
                || lower.contains("failed")
                || lower.contains("denied")
        })
        .take(5)
        .copied()
        .collect();

    let truncated = format!(
        "[truncated: {} lines / {} chars → {} char budget]\n{}\n...\n{}\n--- signals ---\n{}",
        lines.len(),
        content.len(),
        TOOL_RESULT_MAX_CHARS,
        head.join("\n"),
        tail.join("\n"),
        signals.join("\n")
    );

    msg.content = Some(truncated);
    true
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
        }
    }

    #[test]
    fn truncate_large_tool_result() {
        let big = "line ".repeat(1000);
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
        let lines: Vec<String> = (0..200)
            .map(|i| format!("line {}", i))
            .chain(vec!["Error: something went wrong".to_string(), "exit code: 1".to_string()])
            .collect();
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
        let big = "long ".repeat(2000);
        let mut msgs = vec![make_tool_msg(&big), make_tool_msg("ok")];
        let n = sanitize(&mut msgs);
        assert_eq!(n, 1);
    }
}
