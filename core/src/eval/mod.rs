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

pub use collector::{CollectOpts, collect_ledger, load_trace_jsonl};
pub use grader::{GradeOutcome, GraderKind, GraderSpec, grade};
pub use ledger::{
    CostRollup, EvalMode, HarnessConfig, HarnessHealth, MatrixReport, MatrixRow, ModelInfo,
    NorthStar, RunLedger, RunMetrics, RunResult, ScorecardRow, SuiteSummary,
};
pub use mock_llm::{MockScript, MockServer, MockStep, start_mock_server};
pub use prices::{PriceTable, estimate_cost_usd, load_price_table};
pub use reporter::{
    matrix_from_summaries, render_matrix_md, render_report_md, summarize_suite, write_matrix,
    write_report,
};
pub use runner::{EvalRunOptions, EvalRunResult, resolve_suite_dir, run_suite};
pub use task::{EvalSuite, EvalTask, SuiteManifest, TaskManifest, load_suite, load_task};
pub use taxonomy::{HARNESS_FAIL_TAGS, NON_GATE_TAGS, harness_fail_count, is_harness_fail_tag};
