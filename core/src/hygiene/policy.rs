//! Shared truncation policy — single source of truth for tool results **and**
//! tool-call arguments.
//!
//! Consumed by BOTH truncation layers so the model never sees a request view
//! that diverges from the persisted-history view (PLAN-0008 / PLAN-0016):
//! - `hygiene::truncate_tool_result` / `truncate_tool_args` — L2, every turn,
//!   request-boundary copy
//! - `compressor::snip_compact` — L3, persistent history, on overload
//! - spill-at-ingest — oversized incidental results written to disk before
//!   `append_conversation`, so resume does not re-inflate huge shell logs
//!
//! Tools fall into three semantic kinds for **results**. Only `Incidental`
//! output gets the tail-heavy + signal split; `ActivelyRead` output (the model
//! asked for it, and L1 already bounded the read) skips that split and gets
//! only a higher char cap; `Instruction` output is never touched.
//!
//! **Arguments** use a separate rule: content-bearing tools (`write_file`,
//! `edit`) are never truncated — the args *are* the intent. Everything else
//! gets a structured JSON summary when over budget (not a raw placeholder).

/// Semantic kind of a tool result, deciding how (if at all) it is truncated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationKind {
    /// Instruction content for the model — never truncate (e.g. `skill_load`).
    Instruction,
    /// Content the model explicitly requested — skip head/tail; char-cap only
    /// (e.g. `read_file`). L1 (the tool itself) already bounded the read.
    ActivelyRead,
    /// Incidental / potentially noisy output — tail-heavy + signal lines
    /// (errors usually appear at the end of shell/test logs).
    Incidental,
}

/// Tools whose results are instructions to the model and must never be cut.
const INSTRUCTION_TOOLS: &[&str] = &["skill_load"];

/// Tools whose results the model explicitly requested; bounded by L1 already.
const ACTIVE_READ_TOOLS: &[&str] = &["read_file", "subagent", "subagents"];

/// Tools whose arguments *are* the file content / edit payload. Truncating
/// these args makes the model lose what it just wrote — never touch them.
const CONTENT_BEARING_ARG_TOOLS: &[&str] = &["write_file", "edit"];

// ── Budgets (PLAN-0008 → PLAN-0016, Pi-aligned dual limit) ───────────
//
// Incidental output (shell logs, etc.): truncate when either the line cap or
// the byte cap is hit — whichever comes first (Pi-style). Spill recovers the
// full body. Tail-heavy retention keeps exit codes / stack traces visible.

/// Byte budget for incidental tool results (50KB, aligned with Pi).
pub const INCIDENTAL_MAX_CHARS: usize = 50 * 1024;
/// Line budget for incidental tool results (aligned with Pi).
pub const INCIDENTAL_MAX_LINES: usize = 2000;
/// Thin head kept so the model sees how the command started.
pub const INCIDENTAL_HEAD_LINES: usize = 5;
/// Char cap for actively-read tool results — higher than incidental because the
/// model asked for this content and it must stay contiguous (no head/tail split).
pub const ACTIVE_READ_MAX_CHARS: usize = 128_000;

/// Char cap for subagent/subagents results.  Since subagent messages are now
/// persisted to disk and the parent context only carries a summary + file path,
/// this cap is a safety net — subagent results should normally stay well below
/// it after the 2026-07 refactor.
pub const SUBAGENT_RESULT_MAX_CHARS: usize = 256_000;

/// Whole-arguments budget for non-content-bearing tools. Old value was 200 —
/// far too small (a path + a few lines already exceeded it) and the old
/// strategy replaced the entire JSON with an illegal placeholder.
pub const TOOL_ARG_MAX_CHARS: usize = 4_000;
/// Per-string-field budget when summarizing oversized args JSON.
pub const TOOL_ARG_STRING_MAX_CHARS: usize = 1_000;
/// Preview length kept when args are not valid JSON.
const TOOL_ARG_PREVIEW_CHARS: usize = 500;

/// Keywords marking "signal" lines worth preserving from the middle.
const SIGNAL_KEYWORDS: &[&str] = &["error", "exit code", "warning", "failed", "denied"];
const MAX_SIGNAL_LINES: usize = 5;

/// Classify a tool result by its semantic kind, based on the tool name.
pub fn classify(name: Option<&str>) -> TruncationKind {
    match name {
        Some(n) if INSTRUCTION_TOOLS.contains(&n) => TruncationKind::Instruction,
        Some(n) if ACTIVE_READ_TOOLS.contains(&n) => TruncationKind::ActivelyRead,
        _ => TruncationKind::Incidental,
    }
}

/// Whether incidental content exceeds the Pi-style dual budget (lines OR bytes).
pub fn incidental_over_budget(content: &str) -> bool {
    content.len() > INCIDENTAL_MAX_CHARS || content.lines().count() > INCIDENTAL_MAX_LINES
}

/// Whether this tool result should be spilled + truncated at conversation ingest.
pub fn should_spill_at_ingest(name: Option<&str>, content: &str) -> bool {
    classify(name) == TruncationKind::Incidental && incidental_over_budget(content)
}

/// Truncate a tool result's content per its semantic kind.
///
/// Returns `Some(new_content)` if the content was truncated, or `None` if it is
/// left untouched (small enough, or instruction-class). This is the single
/// function both L2 and L3 call, guaranteeing identical behaviour.
pub fn truncate_content(name: Option<&str>, content: &str) -> Option<String> {
    truncate_content_with_spill(name, content, None)
}

/// Like [`truncate_content`], but when `spill_path` is set the truncation
/// marker tells the model to `read_file` that path for the full output.
pub fn truncate_content_with_spill(
    name: Option<&str>,
    content: &str,
    spill_path: Option<&str>,
) -> Option<String> {
    match classify(name) {
        TruncationKind::Instruction => None,
        TruncationKind::ActivelyRead => {
            // Subagent results can be substantially larger than a single read_file
            // call — they aggregate findings from multiple tool invocations.
            let cap = match name {
                Some("subagent") | Some("subagents") => SUBAGENT_RESULT_MAX_CHARS,
                _ => ACTIVE_READ_MAX_CHARS,
            };
            if content.len() <= cap {
                return None;
            }
            Some(truncate_char_cap(content, cap))
        }
        TruncationKind::Incidental => {
            if !incidental_over_budget(content) {
                return None;
            }
            Some(truncate_tail_heavy(content, spill_path))
        }
    }
}

/// Char-cap truncation for actively-read content: keep a contiguous prefix up
/// to the cap (on a UTF-8 boundary) and append a marker. No head/tail split —
/// the model requested this contiguously and can re-read with offset/limit.
fn truncate_char_cap(content: &str, cap: usize) -> String {
    let end = floor_char_boundary(content, cap);
    format!(
        "{}\n[... truncated from {} chars; use a smaller range and re-read]",
        &content[..end],
        content.len()
    )
}

/// Tail-heavy truncation under the dual line/byte budget (PLAN-0016 / Pi).
///
/// Keeps a thin head, then as much of the **end** as fits in the remaining
/// line and byte budget. Signal lines from the dropped middle are preserved.
fn truncate_tail_heavy(content: &str, spill_path: Option<&str>) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let line_count = lines.len();
    let char_count = content.len();

    // Few lines but over the byte budget: keep a contiguous tail of bytes.
    if line_count <= INCIDENTAL_HEAD_LINES + 1 {
        let start = {
            let approx = content.len().saturating_sub(INCIDENTAL_MAX_CHARS);
            floor_char_boundary(content, approx)
        };
        let body = if start == 0 {
            let end = floor_char_boundary(content, INCIDENTAL_MAX_CHARS);
            &content[..end]
        } else {
            &content[start..]
        };
        return format_incidental_truncated(line_count, char_count, "", body, &[], spill_path);
    }

    let head: Vec<&str> = lines.iter().take(INCIDENTAL_HEAD_LINES).copied().collect();
    let head_text = head.join("\n");
    // Reserve room for head + separators inside the char budget.
    let overhead = head_text.len() + 64; // "\n...\n" + marker slack
    let char_budget_for_tail = INCIDENTAL_MAX_CHARS.saturating_sub(overhead);
    let line_budget_for_tail = INCIDENTAL_MAX_LINES.saturating_sub(INCIDENTAL_HEAD_LINES);

    // Grow the tail from the end until we hit line or byte budget.
    let mut tail: Vec<&str> = Vec::new();
    let mut tail_chars = 0usize;
    for line in lines.iter().rev() {
        if tail.len() >= line_budget_for_tail {
            break;
        }
        let add = line.len() + if tail.is_empty() { 0 } else { 1 }; // newline
        if tail_chars + add > char_budget_for_tail && !tail.is_empty() {
            break;
        }
        if tail_chars + add > char_budget_for_tail && tail.is_empty() {
            // Single enormous last line: keep a UTF-8-safe suffix.
            let start = {
                let approx = line.len().saturating_sub(char_budget_for_tail);
                floor_char_boundary(line, approx)
            };
            tail.push(&line[start..]);
            break;
        }
        tail.push(line);
        tail_chars += add;
    }
    tail.reverse();

    let tail_start_idx = line_count.saturating_sub(tail.len());
    let middle = if tail_start_idx > INCIDENTAL_HEAD_LINES {
        &lines[INCIDENTAL_HEAD_LINES..tail_start_idx]
    } else {
        &[][..]
    };
    let signals: Vec<&str> = middle
        .iter()
        .filter(|l| {
            let lower = l.to_lowercase();
            SIGNAL_KEYWORDS.iter().any(|kw| lower.contains(kw))
        })
        .take(MAX_SIGNAL_LINES)
        .copied()
        .collect();

    // If the "tail" already includes the head region, skip duplicating head.
    let head_for_fmt = if tail_start_idx <= INCIDENTAL_HEAD_LINES {
        ""
    } else {
        &head_text
    };

    format_incidental_truncated(
        line_count,
        char_count,
        head_for_fmt,
        &tail.join("\n"),
        &signals,
        spill_path,
    )
}

fn format_incidental_truncated(
    line_count: usize,
    char_count: usize,
    head: &str,
    tail: &str,
    signals: &[&str],
    spill_path: Option<&str>,
) -> String {
    let mut out = format!(
        "[truncated: {line_count} lines / {char_count} chars → ≤{INCIDENTAL_MAX_LINES} lines or ≤{INCIDENTAL_MAX_CHARS} bytes; tail-heavy]\n"
    );
    if !head.is_empty() {
        out.push_str(head);
        out.push_str("\n...\n");
    } else if incidental_over_budget_counts(line_count, char_count) {
        out.push_str("...\n");
    }
    out.push_str(tail);
    if !signals.is_empty() {
        out.push_str("\n--- signals ---\n");
        out.push_str(&signals.join("\n"));
    }
    if let Some(path) = spill_path {
        out.push_str(&format!(
            "\n[Full output spilled to '{path}'. Use read_file on that path if you need earlier lines.]"
        ));
    }
    out
}

fn incidental_over_budget_counts(line_count: usize, char_count: usize) -> bool {
    char_count > INCIDENTAL_MAX_CHARS || line_count > INCIDENTAL_MAX_LINES
}

// ── Tool-call argument truncation ───────────────────────────────────

/// Whether this tool's arguments must be preserved in full (file content /
/// edit payload). Used by hygiene so the model can still see what it wrote.
pub fn is_content_bearing_args(tool_name: &str) -> bool {
    CONTENT_BEARING_ARG_TOOLS.contains(&tool_name)
}

/// Truncate oversized tool-call arguments for the request-boundary copy.
///
/// Returns `Some(new_arguments)` when truncated, or `None` when left untouched
/// (content-bearing tools, or under budget).
///
/// Strategy:
/// - `write_file` / `edit` → never truncate
/// - valid JSON object → keep short fields; replace oversized string values
///   with `"[truncated N chars]"` so the skeleton stays valid JSON
/// - otherwise → UTF-8-safe prefix + size marker (still a JSON string value
///   wrapped as an object so providers don't choke on bare placeholders)
pub fn truncate_args(tool_name: &str, arguments: &str) -> Option<String> {
    if is_content_bearing_args(tool_name) {
        return None;
    }
    if arguments.len() <= TOOL_ARG_MAX_CHARS {
        return None;
    }
    Some(summarize_args(arguments))
}

fn summarize_args(arguments: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(arguments) {
        Ok(serde_json::Value::Object(mut map)) => {
            for (_key, value) in map.iter_mut() {
                if let serde_json::Value::String(s) = value {
                    if s.len() > TOOL_ARG_STRING_MAX_CHARS {
                        *value =
                            serde_json::Value::String(format!("[truncated {} chars]", s.len()));
                    }
                }
            }
            serde_json::to_string(&map).unwrap_or_else(|_| fallback_args_stub(arguments))
        }
        Ok(other) => {
            // Non-object JSON (array / string / number): keep a short preview.
            let rendered = other.to_string();
            if rendered.len() <= TOOL_ARG_MAX_CHARS {
                rendered
            } else {
                fallback_args_stub(arguments)
            }
        }
        Err(_) => fallback_args_stub(arguments),
    }
}

fn fallback_args_stub(arguments: &str) -> String {
    let end = floor_char_boundary(arguments, TOOL_ARG_PREVIEW_CHARS);
    let preview = &arguments[..end];
    serde_json::json!({
        "_truncated": true,
        "_original_bytes": arguments.len(),
        "_preview": preview,
    })
    .to_string()
}

use crate::util::floor_char_boundary;

#[cfg(test)]
mod tests {
    use super::*;

    fn big_incidental() -> String {
        // Over the line budget (and typically under 50KB for short lines).
        (0..INCIDENTAL_MAX_LINES + 500)
            .map(|i| format!("line number {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn huge_incidental_bytes() -> String {
        // Over the byte budget with few lines.
        format!("start\n{}\nend", "x".repeat(INCIDENTAL_MAX_CHARS + 1024))
    }

    #[test]
    fn classify_known_tools() {
        assert_eq!(classify(Some("skill_load")), TruncationKind::Instruction);
        assert_eq!(classify(Some("read_file")), TruncationKind::ActivelyRead);
        assert_eq!(classify(Some("exec")), TruncationKind::Incidental);
        assert_eq!(classify(None), TruncationKind::Incidental);
    }

    #[test]
    fn instruction_never_truncated() {
        let big = "x".repeat(100_000);
        assert_eq!(truncate_content(Some("skill_load"), &big), None);
    }

    #[test]
    fn actively_read_skipped_when_small() {
        let small = "x".repeat(1000);
        assert_eq!(truncate_content(Some("read_file"), &small), None);
    }

    #[test]
    fn actively_read_char_capped_when_large() {
        let big = "x".repeat(150_000);
        let out = truncate_content(Some("read_file"), &big).unwrap();
        assert!(out.contains("truncated"));
        assert!(out.len() < big.len());
        // No head/tail split marker for actively-read content.
        assert!(!out.contains("--- signals ---"));
    }

    #[test]
    fn incidental_skipped_when_small() {
        let small = "x".repeat(1000);
        assert_eq!(truncate_content(Some("exec"), &small), None);
    }

    #[test]
    fn incidental_tail_heavy_when_over_line_budget() {
        let big = big_incidental();
        assert!(big.lines().count() > INCIDENTAL_MAX_LINES);
        let out = truncate_content(Some("shell"), &big).unwrap();
        assert!(out.contains("truncated"));
        assert!(out.contains("tail-heavy"));
        assert!(out.contains("..."));
        // Last lines preserved.
        assert!(out.contains(&format!("line number {}", INCIDENTAL_MAX_LINES + 499)));
        // Early body dropped (beyond thin head) — use a non-prefix-ambiguous probe.
        assert!(!out.contains("\nline number 50\n"));
        // Thin head kept.
        assert!(out.contains("line number 0"));
        assert!(out.len() < big.len());
    }

    #[test]
    fn incidental_tail_heavy_when_over_byte_budget() {
        let big = huge_incidental_bytes();
        assert!(big.len() > INCIDENTAL_MAX_CHARS);
        let out = truncate_content(Some("shell"), &big).unwrap();
        assert!(out.contains("truncated"));
        assert!(out.contains("end"));
        assert!(out.len() < big.len());
    }

    #[test]
    fn incidental_preserves_signal_lines_from_middle() {
        let mut lines: Vec<String> = (0..INCIDENTAL_MAX_LINES + 500)
            .map(|i| format!("line {i}"))
            .collect();
        // Place signals in the dropped middle (after head, before tail window).
        lines[100] = "Error: boom".to_string();
        lines[101] = "exit code: 1".to_string();
        let big = lines.join("\n");
        let out = truncate_content(Some("shell"), &big).unwrap();
        assert!(out.contains("Error: boom"));
        assert!(out.contains("exit code: 1"));
        assert!(out.contains("--- signals ---"));
    }

    #[test]
    fn incidental_spill_path_in_marker() {
        let big = big_incidental();
        let out = truncate_content_with_spill(Some("shell"), &big, Some("/tmp/spill.txt")).unwrap();
        assert!(out.contains("/tmp/spill.txt"));
        assert!(out.contains("read_file"));
    }

    #[test]
    fn should_spill_only_for_oversized_incidental() {
        assert!(!should_spill_at_ingest(Some("shell"), "short"));
        assert!(!should_spill_at_ingest(
            Some("read_file"),
            &"x".repeat(20_000)
        ));
        assert!(should_spill_at_ingest(
            Some("shell"),
            &"x".repeat(INCIDENTAL_MAX_CHARS + 1)
        ));
        assert!(should_spill_at_ingest(Some("shell"), &big_incidental()));
    }

    #[test]
    fn char_cap_is_utf8_safe() {
        // Multibyte content right at the cap boundary must not panic / split a char.
        let big = "é".repeat(100_000); // each é is 2 bytes
        let out = truncate_content(Some("read_file"), &big).unwrap();
        assert!(out.contains("truncated"));
        // The kept prefix is valid UTF-8 (String guarantees this, but assert no panic).
        assert!(out.starts_with('é'));
    }

    #[test]
    fn write_file_args_never_truncated() {
        let content = "x".repeat(20_000);
        let args = serde_json::json!({
            "path": "tests/big.py",
            "content": content,
        })
        .to_string();
        assert!(args.len() > TOOL_ARG_MAX_CHARS);
        assert_eq!(truncate_args("write_file", &args), None);
        assert_eq!(truncate_args("edit", &args), None);
    }

    #[test]
    fn incidental_args_summarized_when_large() {
        let long = "y".repeat(5_000);
        let args = serde_json::json!({
            "command": "echo hello",
            "stdin": long,
        })
        .to_string();
        assert!(args.len() > TOOL_ARG_MAX_CHARS);
        let out = truncate_args("shell", &args).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["command"], "echo hello");
        let stdin = v["stdin"].as_str().unwrap();
        assert!(stdin.starts_with("[truncated"));
        assert!(stdin.contains("5000"));
    }

    #[test]
    fn short_args_untouched() {
        let args = r#"{"command":"ls"}"#;
        assert_eq!(truncate_args("shell", args), None);
    }

    #[test]
    fn invalid_json_args_get_preview_stub() {
        let junk = format!("not-json {}", "z".repeat(5_000));
        let out = truncate_args("shell", &junk).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["_truncated"], true);
        assert!(v["_original_bytes"].as_u64().unwrap() > 5000);
        assert!(v["_preview"].as_str().unwrap().starts_with("not-json"));
    }
}
