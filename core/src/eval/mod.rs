//! Harness evaluation: ledger, event collector, reporter, and (later) runner.
//!
//! See `docs/active/PLAN-0010_harness_evaluation.md`.

pub mod collector;
pub mod ledger;
pub mod reporter;
pub mod taxonomy;

pub use collector::{collect_ledger, load_trace_jsonl, CollectOpts};
pub use ledger::{
    CostRollup, EvalMode, HarnessConfig, HarnessHealth, MatrixReport, MatrixRow, ModelInfo,
    NorthStar, RunLedger, RunMetrics, RunResult, ScorecardRow, SuiteSummary,
};
pub use reporter::{
    matrix_from_summaries, render_matrix_md, render_report_md, summarize_suite, write_matrix,
    write_report,
};
pub use taxonomy::{harness_fail_count, is_harness_fail_tag, HARNESS_FAIL_TAGS, NON_GATE_TAGS};
