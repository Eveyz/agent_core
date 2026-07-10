//! Harness failure taxonomy — scaffolding tags only (not model quality).

/// Tags that indicate harness/runtime contract failures.
pub const HARNESS_FAIL_TAGS: &[&str] = &[
    "hung_no_terminal",
    "double_terminal",
    "tool_unpaired",
    "orphan_subagent",
    "max_iterations",
    "permission_false_positive",
    "permission_false_negative",
    "approval_deadlock",
    "steer_dropped",
    "steer_after_terminal",
    "pause_resume_corrupt",
    "context_lost_after_compact",
    "recovery_exhausted",
    "process_leak",
    "seq_gap",
    "cache_ledger_missing",
];

/// Non-gate tags (task/model outcomes — not CI harness blockers by themselves).
pub const NON_GATE_TAGS: &[&str] = &["model_fail", "grader_fail"];

pub fn is_harness_fail_tag(tag: &str) -> bool {
    HARNESS_FAIL_TAGS.contains(&tag)
}

pub fn harness_fail_count(tags: &[String]) -> usize {
    tags.iter().filter(|t| is_harness_fail_tag(t)).count()
}
