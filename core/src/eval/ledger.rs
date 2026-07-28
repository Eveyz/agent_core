//! Run ledger + suite summary types for harness evaluation.

use serde::{Deserialize, Serialize};

/// Eval execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EvalMode {
    #[default]
    Mock,
    Live,
}

impl EvalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::Live => "live",
        }
    }
}

impl std::str::FromStr for EvalMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "mock" => Ok(Self::Mock),
            "live" => Ok(Self::Live),
            other => Err(format!("unknown eval mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HarnessConfig {
    pub permission_mode: String,
    pub max_iterations: u32,
    pub compression: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
    /// Ablation / variant label (e.g. "compress=off").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelInfo {
    pub provider: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunResult {
    pub pass: bool,
    #[serde(default)]
    pub grader: String,
    #[serde(default)]
    pub fail_tags: Vec<String>,
    #[serde(default)]
    pub terminal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunMetrics {
    pub wall_ms: u64,
    pub model_ms: u64,
    pub tool_ms: u64,
    pub turns: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    #[serde(default)]
    pub unique_tools: Vec<String>,
    pub approvals: u64,
    pub steers: u64,
    /// Longest observed delay from steer acceptance to context injection.
    #[serde(default)]
    pub steer_interrupt_latency_ms: u64,
    pub compactions: u64,
    pub retries: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_hit: u64,
    pub cache_miss: u64,
    pub cache_hit_rate: f64,
    pub cost_usd: f64,
    /// True when tokens/cost were estimated rather than from provider usage.
    #[serde(default)]
    pub tokens_estimated: bool,
}

/// Per-task run ledger — the atomic eval artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLedger {
    pub task_id: String,
    pub suite: String,
    pub mode: EvalMode,
    pub harness: HarnessConfig,
    pub model: ModelInfo,
    pub result: RunResult,
    pub metrics: RunMetrics,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket: Option<String>,
    /// When true, this run is a taxonomy self-check and excluded from harness_fail_rate.
    #[serde(default)]
    pub expect_harness_fail: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HarnessHealth {
    pub hung_no_terminal: f64,
    pub tool_unpaired: f64,
    pub approval_deadlock: f64,
    pub orphan_subagent: f64,
    pub max_iterations: f64,
    pub seq_gap: f64,
    pub process_leak: f64,
    pub steer_dropped: f64,
    /// Fraction of runs with any harness fail tag.
    pub harness_fail_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScorecardRow {
    pub bucket: String,
    pub n: usize,
    pub pass_at_1: f64,
    pub p50_wall_ms: u64,
    pub p90_wall_ms: u64,
    pub median_turns: f64,
    pub median_tool_calls: f64,
    pub usd_per_pass: f64,
    pub cache_hit_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostRollup {
    pub total_usd: f64,
    pub usd_on_failures: f64,
    pub usd_per_pass: f64,
}

/// Aggregated suite report (machine-readable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteSummary {
    pub suite: String,
    pub mode: EvalMode,
    pub model: ModelInfo,
    pub harness: HarnessConfig,
    pub generated_at: String,
    pub n_tasks: usize,
    pub n_pass: usize,
    pub pass_at_1: f64,
    pub harness_health: HarnessHealth,
    pub scorecard: Vec<ScorecardRow>,
    pub cost: CostRollup,
    pub north_star: NorthStar,
    pub runs: Vec<RunLedger>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NorthStar {
    pub pass_at_1: f64,
    pub usd_per_successful_task: f64,
    pub p90_wall_ms: u64,
    pub median_tool_calls: f64,
    pub harness_fail_rate: f64,
}

/// One cell in a multi-model / ablation matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixRow {
    pub label: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub pass_at_1: f64,
    pub p50_wall_ms: u64,
    pub median_turns: f64,
    pub usd_per_pass: f64,
    pub harness_fail_rate: f64,
    pub n_tasks: usize,
    pub n_pass: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixReport {
    pub suite: String,
    pub generated_at: String,
    pub kind: String, // "model_compare" | "harness_ablation"
    pub rows: Vec<MatrixRow>,
}
