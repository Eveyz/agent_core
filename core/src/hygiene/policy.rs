//! Shared tool-result truncation policy — single source of truth.
//!
//! Consumed by BOTH truncation layers so the model never sees a request view
//! that diverges from the persisted-history view (PLAN-0008):
//! - `hygiene::truncate_tool_result`  — L2, every turn, request-boundary copy
//! - `compressor::snip_compact`       — L3, persistent history, on overload
//!
//! Tools fall into three semantic kinds. Only `Incidental` output gets the
//! head/tail/signal split; `ActivelyRead` output (the model asked for it, and
//! L1 already bounded the read) skips that split and gets only a higher char
//! cap; `Instruction` output is never touched.

/// Semantic kind of a tool result, deciding how (if at all) it is truncated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationKind {
    /// Instruction content for the model — never truncate (e.g. `skill_load`).
    Instruction,
    /// Content the model explicitly requested — skip head/tail; char-cap only
    /// (e.g. `read_file`). L1 (the tool itself) already bounded the read.
    ActivelyRead,
    /// Incidental / potentially noisy output — head + tail + signal lines.
    Incidental,
}

/// Tools whose results are instructions to the model and must never be cut.
const INSTRUCTION_TOOLS: &[&str] = &["skill_load"];

/// Tools whose results the model explicitly requested; bounded by L1 already.
const ACTIVE_READ_TOOLS: &[&str] = &["read_file", "subagent", "subagents"];

// ── Budgets (PLAN-0008, updated 2026-07) ────────────────────────────
//
// With subagent messages now persisted to disk files (~/.agverse/subagents/)
// and only a summary going into the parent context, the caps below serve as
// a safety net rather than a primary constraint.  Still kept conservative
// enough that a single tool result never dominates the model's context window.

/// Char budget for incidental tool results before head/tail truncation kicks in
/// (≈4K tokens, ~3% of a 128K context). Old value was 4000 — too small for any
/// real source file, which is what produced the "content was truncated" reports.
pub const INCIDENTAL_MAX_CHARS: usize = 16_000;
pub const INCIDENTAL_HEAD_LINES: usize = 40;
pub const INCIDENTAL_TAIL_LINES: usize = 20;
/// Char cap for actively-read tool results — higher than incidental because the
/// model asked for this content and it must stay contiguous (no head/tail split).
pub const ACTIVE_READ_MAX_CHARS: usize = 128_000;

/// Char cap for subagent/subagents results.  Since subagent messages are now
/// persisted to disk and the parent context only carries a summary + file path,
/// this cap is a safety net — subagent results should normally stay well below
/// it after the 2026-07 refactor.
pub const SUBAGENT_RESULT_MAX_CHARS: usize = 256_000;

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

/// Truncate a tool result's content per its semantic kind.
///
/// Returns `Some(new_content)` if the content was truncated, or `None` if it is
/// left untouched (small enough, or instruction-class). This is the single
/// function both L2 and L3 call, guaranteeing identical behaviour.
pub fn truncate_content(name: Option<&str>, content: &str) -> Option<String> {
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
            if content.len() <= INCIDENTAL_MAX_CHARS {
                return None;
            }
            Some(truncate_head_tail(content))
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

/// Head + tail + signal-line truncation for incidental output.
fn truncate_head_tail(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= INCIDENTAL_HEAD_LINES + INCIDENTAL_TAIL_LINES {
        // Few lines but over the char budget: just char-cap so the marker shows.
        return truncate_char_cap(content, INCIDENTAL_MAX_CHARS);
    }

    let head: Vec<&str> = lines.iter().take(INCIDENTAL_HEAD_LINES).copied().collect();
    let tail: Vec<&str> = lines
        .iter()
        .rev()
        .take(INCIDENTAL_TAIL_LINES)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    let signals: Vec<&str> = lines
        .iter()
        .filter(|l| {
            let lower = l.to_lowercase();
            SIGNAL_KEYWORDS.iter().any(|kw| lower.contains(kw))
        })
        .take(MAX_SIGNAL_LINES)
        .copied()
        .collect();

    format!(
        "[truncated: {} lines / {} chars → {} char budget]\n{}\n...\n{}\n--- signals ---\n{}",
        lines.len(),
        content.len(),
        INCIDENTAL_MAX_CHARS,
        head.join("\n"),
        tail.join("\n"),
        signals.join("\n")
    )
}

use crate::util::floor_char_boundary;

#[cfg(test)]
mod tests {
    use super::*;

    fn big_incidental() -> String {
        // > INCIDENTAL_MAX_CHARS and many lines so head/tail path is exercised.
        (0..2000).map(|i| format!("line number {i}")).collect::<Vec<_>>().join("\n")
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
    fn incidental_head_tail_when_large() {
        let big = big_incidental();
        assert!(big.len() > INCIDENTAL_MAX_CHARS);
        let out = truncate_content(Some("exec"), &big).unwrap();
        assert!(out.contains("truncated"));
        assert!(out.contains("..."));
        assert!(out.contains("--- signals ---"));
        assert!(out.len() < big.len());
    }

    #[test]
    fn incidental_preserves_signal_lines() {
        let mut lines: Vec<String> = (0..2000).map(|i| format!("line {}", i)).collect();
        lines.push("Error: boom".to_string());
        lines.push("exit code: 1".to_string());
        let big = lines.join("\n");
        let out = truncate_content(Some("exec"), &big).unwrap();
        assert!(out.contains("Error: boom"));
        assert!(out.contains("exit code: 1"));
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
}
