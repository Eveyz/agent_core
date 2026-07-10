//! Harness evaluation: ledger, event collector, reporter, mock LLM, runner.
//!
//! See `docs/active/PLAN-0010_harness_evaluation.md`.

pub mod collector;
pub mod grader;
pub mod ledger;
pub mod mock_llm;
pub mod prices;
pub mod reporter;
pub mod runner;
pub mod task;
pub mod taxonomy;

pub use collector::{collect_ledger, load_trace_jsonl, CollectOpts};
pub use grader::{grade, GradeOutcome, GraderKind, GraderSpec};
pub use ledger::{
    CostRollup, EvalMode, HarnessConfig, HarnessHealth, MatrixReport, MatrixRow, ModelInfo,
    NorthStar, RunLedger, RunMetrics, RunResult, ScorecardRow, SuiteSummary,
};
pub use mock_llm::{start_mock_server, MockScript, MockServer, MockStep};
pub use prices::{estimate_cost_usd, load_price_table, PriceTable};
pub use reporter::{
    matrix_from_summaries, render_matrix_md, render_report_md, summarize_suite, write_matrix,
    write_report,
};
pub use runner::{resolve_suite_dir, run_suite, EvalRunOptions, EvalRunResult};
pub use task::{load_suite, load_task, EvalSuite, EvalTask, SuiteManifest, TaskManifest};
pub use taxonomy::{harness_fail_count, is_harness_fail_tag, HARNESS_FAIL_TAGS, NON_GATE_TAGS};
