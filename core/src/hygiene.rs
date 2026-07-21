//! History Hygiene Layer — cleans messages before they're sent to the model API.
//!
//! Operations performed on the request-boundary copy only (persistent history untouched):
//! 1. Truncate oversized tool results (keep head + tail + signal lines)
//! 2. Summarize long tool arguments (content-bearing tools like write_file/edit exempt)
//! 3. Strip `<think>` blocks from assistant turns before the last user message
//! 4. Truncate remaining `<think>` blocks in the active tool loop (4KB)
//! 5. Inject stream-retry hints with partial thinking (request copy only)
//!
//! This prevents context pollution from bloated tool outputs and keeps the
//! message prefix stable for prompt KV cache hits.
//!
//! Tool-result and tool-arg truncation delegate to [`policy`] (shared with
//! `compressor::snip_compact`) so the request view and the persisted-history
//! view never diverge. See PLAN-0008.

pub mod policy;

use crate::types::{Message, Role};

const THINKING_OPEN: &str = "<think>";
const THINKING_CLOSE: &str = "</think>";

/// Max chars kept from a `<think>` body on outbound API requests.
pub const THINKING_BODY_MAX_CHARS: usize = 4_096;
/// Max chars of partial assistant text injected on stream retry.
pub const PARTIAL_TEXT_MAX_CHARS: usize = 2_048;
pub const NO_STATUS_ACKNOWLEDGEMENT: &str =
    "Output no preamble or status acknowledgement.";

/// Embed thinking into assistant content for live context / Chat Completions compat.
/// Empty thinking returns `text` unchanged. Format matches frontend `entriesToMessages`.
pub fn wrap_thinking(thinking: &str, text: &str) -> String {
    let thinking = thinking.trim();
    if thinking.is_empty() {
        return text.to_string();
    }
    if text.is_empty() {
        format!("{THINKING_OPEN}{thinking}{THINKING_CLOSE}")
    } else {
        format!("{THINKING_OPEN}{thinking}{THINKING_CLOSE}\n{text}")
    }
}

/// Remove every `<think>…</think>` block from content (including unclosed open tags).
pub fn strip_thinking_in_content(content: &str) -> String {
    if !content.contains(THINKING_OPEN) {
        return content.to_string();
    }
    let mut result = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find(THINKING_OPEN) {
        result.push_str(&rest[..start]);
        rest = &rest[start + THINKING_OPEN.len()..];
        if let Some(end) = rest.find(THINKING_CLOSE) {
            rest = &rest[end + THINKING_CLOSE.len()..];
            // Drop a single leading newline left by wrap_thinking.
            if rest.starts_with('\n') {
                rest = &rest[1..];
            }
        } else {
            // Unclosed tag: drop the remainder of the thinking body.
            break;
        }
    }
    result.push_str(rest);
    result
}

/// Strip plaintext `<think>` from assistant messages before the last *real* user turn.
/// Keeps thinking in the active tool loop (messages after the last user).
///
/// Runtime-owned System messages are naturally ignored when locating the last
/// user, so the active tool loop remains active across per-turn injection.
///
/// Also clears structured reasoning (text + opaque blobs) on those historical
/// assistants. Active-loop blobs are left intact for provider round-trip.
pub fn strip_historical_thinking(messages: &mut [Message]) -> usize {
    let last_user = messages.iter().enumerate().rev().find_map(|(i, m)| {
        if m.role == Role::User {
            Some(i)
        } else {
            None
        }
    });
    let Some(last_user) = last_user else {
        return 0;
    };
    let mut modified = 0;
    for msg in messages.iter_mut().take(last_user) {
        if msg.role != Role::Assistant {
            continue;
        }
        let mut changed = false;
        if let Some(content) = msg.content.as_ref() {
            if content.contains(THINKING_OPEN) {
                let stripped = strip_thinking_in_content(content);
                if stripped != *content {
                    msg.content = if stripped.is_empty() {
                        None
                    } else {
                        Some(stripped)
                    };
                    changed = true;
                }
            }
        }
        if let Some(ref mut reasoning) = msg.reasoning {
            if reasoning.text.take().is_some() {
                changed = true;
            }
            if reasoning.encrypted_content.take().is_some() {
                changed = true;
            }
            if reasoning.signature.take().is_some() {
                changed = true;
            }
            if reasoning.summary.take().is_some() {
                changed = true;
            }
            if reasoning.is_empty() {
                msg.reasoning = None;
            }
        }
        if changed {
            modified += 1;
        }
    }
    modified
}

/// Strip plaintext thinking from every assistant message (content tags + reasoning.text).
/// Opaque blobs/signatures are also cleared — after compaction the model starts fresh.
/// Returns the number of messages modified.
pub fn strip_all_thinking(messages: &mut [Message]) -> usize {
    let mut modified = 0;
    for msg in messages.iter_mut() {
        if msg.role != Role::Assistant {
            continue;
        }
        let mut changed = false;
        if let Some(content) = msg.content.as_ref() {
            if content.contains(THINKING_OPEN) {
                let stripped = strip_thinking_in_content(content);
                if stripped != *content {
                    msg.content = if stripped.is_empty() {
                        None
                    } else {
                        Some(stripped)
                    };
                    changed = true;
                }
            }
        }
        if msg.reasoning.take().is_some() {
            changed = true;
        }
        if changed {
            modified += 1;
        }
    }
    modified
}

/// Truncate a raw thinking string to the outbound budget.
pub fn truncate_thinking_body(thinking: &str) -> String {
    if thinking.len() <= THINKING_BODY_MAX_CHARS {
        return thinking.to_string();
    }
    let marker = format!(
        "\n[thinking truncated in middle from {} chars]\n",
        thinking.len()
    );
    let content_budget = THINKING_BODY_MAX_CHARS.saturating_sub(marker.len());
    let head_budget = content_budget * 2 / 3;
    let tail_budget = content_budget.saturating_sub(head_budget);
    let head_end = crate::util::floor_char_boundary(thinking, head_budget);
    let tail_start_target = thinking.len().saturating_sub(tail_budget);
    let tail_start = thinking
        .char_indices()
        .map(|(idx, _)| idx)
        .find(|idx| *idx >= tail_start_target)
        .unwrap_or(thinking.len());
    format!(
        "{}{}{}",
        &thinking[..head_end],
        marker,
        &thinking[tail_start..]
    )
}

/// Truncate every `<think>` block inside assistant content.
pub fn truncate_thinking_in_content(content: &str) -> Option<String> {
    if !content.contains(THINKING_OPEN) {
        return None;
    }
    let mut result = String::new();
    let mut rest = content;
    let mut modified = false;
    while let Some(start) = rest.find(THINKING_OPEN) {
        result.push_str(&rest[..start]);
        rest = &rest[start + THINKING_OPEN.len()..];
        if let Some(end) = rest.find(THINKING_CLOSE) {
            let body = &rest[..end];
            let truncated = truncate_thinking_body(body);
            if truncated != body {
                modified = true;
            }
            result.push_str(THINKING_OPEN);
            result.push_str(&truncated);
            result.push_str(THINKING_CLOSE);
            rest = &rest[end + THINKING_CLOSE.len()..];
        } else {
            result.push_str(THINKING_OPEN);
            result.push_str(&truncate_thinking_body(rest));
            modified = true;
            return Some(result);
        }
    }
    result.push_str(rest);
    if modified { Some(result) } else { None }
}

/// Append a trailing runtime hint so a stream retry can continue without redoing work.
/// Only mutates the request-boundary message list — never persisted history.
pub fn inject_stream_retry_hint(messages: &mut Vec<Message>, thinking: &str, text: &str) {
    if thinking.is_empty() && text.is_empty() {
        return;
    }
    let think = truncate_thinking_body(thinking);
    let partial_text = if text.len() > PARTIAL_TEXT_MAX_CHARS {
        truncate_thinking_body(text)
    } else {
        text.to_string()
    };
    let mut partial = Message::assistant(&wrap_thinking(&think, &partial_text));
    if !think.is_empty() {
        partial.reasoning = Some(crate::types::ReasoningState::from_text(think));
    }
    messages.push(partial);
    messages.push(Message::system(&format!(
        "Continue directly from the assistant output above; do not repeat completed work. \
         {NO_STATUS_ACKNOWLEDGEMENT}"
    )));
}

/// Run the full hygiene pass on a message list (mutates in place).
/// Returns the count of messages that were modified.
///
/// Order matters: strip historical thinking first, then truncate what remains
/// in the active tool loop. Opaque `encrypted_content` / `signature` blobs are
/// never truncated.
pub fn sanitize(messages: &mut Vec<Message>) -> usize {
    let mut modified = strip_historical_thinking(messages);
    for msg in messages.iter_mut() {
        if truncate_tool_result(msg) {
            modified += 1;
        }
        if truncate_tool_args(msg) {
            modified += 1;
        }
        if truncate_assistant_thinking(msg) {
            modified += 1;
        }
    }
    modified
}

fn truncate_assistant_thinking(msg: &mut Message) -> bool {
    if msg.role != Role::Assistant {
        return false;
    }
    let mut changed = false;
    let content = match &msg.content {
        Some(c) if c.contains(THINKING_OPEN) => c.clone(),
        _ => String::new(),
    };
    if !content.is_empty() {
        if let Some(truncated) = truncate_thinking_in_content(&content) {
            msg.content = Some(truncated);
            changed = true;
        }
    }
    // Truncate plaintext reasoning.text only; never touch opaque blobs.
    if let Some(ref mut reasoning) = msg.reasoning {
        if let Some(ref text) = reasoning.text {
            if text.len() > THINKING_BODY_MAX_CHARS {
                reasoning.text = Some(truncate_thinking_body(text));
                changed = true;
            }
        }
    }
    changed
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

/// Truncate long tool call arguments via the shared policy.
///
/// Content-bearing tools (`write_file`, `edit`) are never touched; other tools
/// get a structured JSON summary when over budget. See [`policy::truncate_args`].
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
        if let Some(summarized) = policy::truncate_args(&tc.function.name, &tc.function.arguments) {
            tc.function.arguments = summarized;
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
        reasoning: None,
        images: None,
        }
    }

    fn make_assistant_with_named_args(name: &str, args: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: Some("ok".into()),
            tool_calls: Some(vec![crate::types::ToolCall {
                id: "c1".into(),
                call_type: "function".into(),
                function: crate::types::FunctionCall {
                    name: name.into(),
                    arguments: args.to_string(),
                },
            }]),
            tool_call_id: None,
            name: None,
            model: None,
            metadata: None,
            reasoning: None,
            images: None,
        }
    }

    fn make_assistant_with_args(args: &str) -> Message {
        make_assistant_with_named_args("shell", args)
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
        let long = "x".repeat(5_000);
        let args = serde_json::json!({
            "command": "run",
            "stdin": long,
        })
        .to_string();
        let mut msg = make_assistant_with_args(&args);
        assert!(truncate_tool_args(&mut msg));
        let out = &msg.tool_calls.as_ref().unwrap()[0].function.arguments;
        let v: serde_json::Value = serde_json::from_str(out).unwrap();
        assert_eq!(v["command"], "run");
        assert!(v["stdin"].as_str().unwrap().contains("truncated"));
    }

    #[test]
    fn skip_short_tool_args() {
        let short = r#"{"command": "ls"}"#;
        let mut msg = make_assistant_with_args(short);
        assert!(!truncate_tool_args(&mut msg));
    }

    #[test]
    fn skip_write_file_args_even_when_huge() {
        let content = "fn main() {}\n".repeat(2_000);
        let args = serde_json::json!({
            "path": "src/main.rs",
            "content": content,
        })
        .to_string();
        assert!(args.len() > policy::TOOL_ARG_MAX_CHARS);
        let mut msg = make_assistant_with_named_args("write_file", &args);
        assert!(!truncate_tool_args(&mut msg));
        assert_eq!(
            msg.tool_calls.as_ref().unwrap()[0].function.arguments,
            args
        );
    }

    #[test]
    fn skip_edit_args_even_when_huge() {
        let args = serde_json::json!({
            "path": "src/lib.rs",
            "old_string": "a".repeat(3_000),
            "new_string": "b".repeat(3_000),
        })
        .to_string();
        let mut msg = make_assistant_with_named_args("edit", &args);
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
        reasoning: None,
        images: None,
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
        reasoning: None,
        images: None,
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
        reasoning: None,
        images: None,
        };
        assert!(truncate_tool_result(&mut msg));
        let c = msg.content.unwrap();
        // Char-capped (not head/tail-split): no signal section.
        assert!(c.contains("truncated"));
        assert!(!c.contains("--- signals ---"));
    }

    #[test]
    fn truncate_assistant_thinking_tags() {
        let long = "x".repeat(10_000);
        let content = format!("<think>{long}</think>\nanswer");
        let out = truncate_thinking_in_content(&content).unwrap();
        assert!(out.contains("thinking truncated"));
        assert!(out.contains("answer"));
        assert!(out.len() < content.len());
    }

    #[test]
    fn inject_stream_retry_hint_adds_runtime_message() {
        let mut msgs = vec![Message::user("hi")];
        inject_stream_retry_hint(&mut msgs, "partial reasoning", "partial text");
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1].role, Role::Assistant);
        assert_eq!(
            msgs[1].content.as_deref(),
            Some("<think>partial reasoning</think>\npartial text")
        );
        assert_eq!(
            msgs[1].reasoning.as_ref().and_then(|r| r.text.as_deref()),
            Some("partial reasoning")
        );
        assert_eq!(msgs[2].role, Role::System);
        let hint = msgs[2].content.as_ref().unwrap();
        assert!(hint.contains(NO_STATUS_ACKNOWLEDGEMENT));
        assert!(!hint.contains("recovery"));
    }

    #[test]
    fn long_thinking_preserves_both_head_and_latest_tail() {
        let thinking = format!(
            "HEAD_SENTINEL{}TAIL_SENTINEL",
            "x".repeat(THINKING_BODY_MAX_CHARS)
        );
        let truncated = truncate_thinking_body(&thinking);
        assert!(truncated.contains("HEAD_SENTINEL"));
        assert!(truncated.contains("TAIL_SENTINEL"));
        assert!(truncated.contains("thinking truncated in middle"));
    }

    #[test]
    fn sanitize_truncates_assistant_thinking() {
        let long = "y".repeat(10_000);
        let msg = Message {
            role: Role::Assistant,
            content: Some(format!("<think>{long}</think>")),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            model: None,
            metadata: None,
            reasoning: None,
            images: None,
        };
        let n = sanitize(&mut vec![msg]);
        assert_eq!(n, 1);
    }

    #[test]
    fn wrap_thinking_embeds_tags() {
        assert_eq!(wrap_thinking("", "hi"), "hi");
        assert_eq!(
            wrap_thinking("reason", "hi"),
            "<think>reason</think>\nhi"
        );
        assert_eq!(wrap_thinking("reason", ""), "<think>reason</think>");
    }

    #[test]
    fn strip_thinking_removes_tags() {
        let content = "<think>secret</think>\nvisible";
        assert_eq!(strip_thinking_in_content(content), "visible");
        assert_eq!(strip_thinking_in_content("no tags"), "no tags");
    }

    #[test]
    fn strip_historical_keeps_active_tool_loop() {
        let mut msgs = vec![
            Message::user("first"),
            Message::assistant(&wrap_thinking("old reason", "old answer")),
            Message::user("second"),
            Message::assistant_with_tools(
                &wrap_thinking("active reason", "calling tool"),
                vec![crate::types::ToolCall {
                    id: "c1".into(),
                    call_type: "function".into(),
                    function: crate::types::FunctionCall {
                        name: "shell".into(),
                        arguments: "{}".into(),
                    },
                }],
            ),
            Message::tool("c1".into(), "ok".into(), Some("shell".into())),
            // Trailing context injection must not count as the "last user".
            Message::system("<context_injection>\ncwd=/tmp\n</context_injection>"),
        ];
        let n = strip_historical_thinking(&mut msgs);
        assert_eq!(n, 1);
        assert!(
            !msgs[1].content.as_ref().unwrap().contains("<think>"),
            "historical thinking stripped"
        );
        assert!(
            msgs[3].content.as_ref().unwrap().contains("<think>active reason</think>"),
            "active loop thinking kept"
        );
    }

    #[test]
    fn sanitize_strips_then_truncates_active_loop() {
        let long = "z".repeat(10_000);
        let mut msgs = vec![
            Message::user("q1"),
            Message::assistant(&wrap_thinking("old", "a1")),
            Message::user("q2"),
            Message::assistant(&wrap_thinking(&long, "a2")),
            Message::system("<context_injection>\nx\n</context_injection>"),
        ];
        let n = sanitize(&mut msgs);
        assert!(n >= 2);
        assert!(!msgs[1].content.as_ref().unwrap().contains("<think>"));
        let active = msgs[3].content.as_ref().unwrap();
        assert!(active.contains("thinking truncated"));
        assert!(active.contains("a2"));
    }

    #[test]
    fn opaque_blob_never_truncated_in_active_loop() {
        use crate::types::ReasoningState;
        let blob = "ENCRYPTED_BLOB_".to_string() + &"B".repeat(20_000);
        let msg = Message::assistant("ok").with_reasoning(ReasoningState {
            text: Some("x".repeat(10_000)),
            encrypted_content: Some(blob.clone()),
            signature: Some("sig-unchanged".into()),
            summary: None,
        });
        let mut msgs = vec![
            Message::user("q"),
            msg,
            Message::system("<context_injection>\nx\n</context_injection>"),
        ];
        sanitize(&mut msgs);
        let r = msgs[1].reasoning.as_ref().unwrap();
        assert_eq!(r.encrypted_content.as_deref(), Some(blob.as_str()));
        assert_eq!(r.signature.as_deref(), Some("sig-unchanged"));
        assert!(r.text.as_ref().unwrap().contains("thinking truncated"));
    }

    #[test]
    fn historical_blobs_cleared_across_user_turn() {
        use crate::types::ReasoningState;
        let mut msgs = vec![
            Message::user("q1"),
            Message::assistant("a1").with_reasoning(ReasoningState {
                text: Some("t".into()),
                encrypted_content: Some("blob1".into()),
                signature: Some("sig1".into()),
                summary: None,
            }),
            Message::user("q2"),
            Message::assistant("a2").with_reasoning(ReasoningState {
                encrypted_content: Some("blob2".into()),
                ..Default::default()
            }),
        ];
        strip_historical_thinking(&mut msgs);
        assert!(msgs[1].reasoning.is_none());
        assert_eq!(
            msgs[3]
                .reasoning
                .as_ref()
                .unwrap()
                .encrypted_content
                .as_deref(),
            Some("blob2")
        );
    }
}
