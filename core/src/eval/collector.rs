//! Collect metrics + harness fail tags from a RunEvent envelope stream.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::runtime::event::{Envelope, RunEvent};

use super::ledger::{
    EvalMode, HarnessConfig, ModelInfo, RunLedger, RunMetrics, RunResult,
};
use super::taxonomy;

/// Options for building a ledger from events.
#[derive(Debug, Clone)]
pub struct CollectOpts {
    pub task_id: String,
    pub suite: String,
    pub mode: EvalMode,
    pub harness: HarnessConfig,
    pub model: ModelInfo,
    pub grader: String,
    pub grader_pass: Option<bool>,
    pub extra_fail_tags: Vec<String>,
    pub trace_path: Option<String>,
    pub bucket: Option<String>,
    pub note: Option<String>,
    /// Optional wall clock override (ms). If None, derived from event timestamps.
    pub wall_ms_override: Option<u64>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub tokens_estimated: bool,
}

impl Default for CollectOpts {
    fn default() -> Self {
        Self {
            task_id: "unknown".into(),
            suite: "unknown".into(),
            mode: EvalMode::Mock,
            harness: HarnessConfig::default(),
            model: ModelInfo::default(),
            grader: "expect_events".into(),
            grader_pass: None,
            extra_fail_tags: vec![],
            trace_path: None,
            bucket: None,
            note: None,
            wall_ms_override: None,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
            tokens_estimated: false,
        }
    }
}

/// Aggregate envelopes into a [`RunLedger`].
pub fn collect_ledger(envelopes: &[Envelope], opts: CollectOpts) -> RunLedger {
    let mut metrics = RunMetrics::default();
    let mut fail_tags: Vec<String> = opts.extra_fail_tags.clone();

    let mut terminals: Vec<String> = Vec::new();
    let mut started_at: Option<DateTime<Utc>> = None;
    let mut terminal_at: Option<DateTime<Utc>> = None;

    let mut open_tools: HashSet<String> = HashSet::new();
    let mut tool_names: HashSet<String> = HashSet::new();
    let mut tool_start_ts: HashMap<String, DateTime<Utc>> = HashMap::new();

    let mut open_subagents: HashSet<String> = HashSet::new();
    let mut open_processes: HashSet<String> = HashSet::new();

    let mut pending_approvals: HashSet<String> = HashSet::new();
    let mut queued_steers: HashSet<String> = HashSet::new();
    let mut injected_or_cancelled_steers: HashSet<String> = HashSet::new();

    let mut last_seq: Option<u64> = None;
    let mut saw_run_started = false;
    let mut saw_cache_info = false;
    let mut cache_summary_seen = false;
    let mut turn_started = 0u64;
    let mut turn_ended = 0u64;

    let mut model_call_open: Option<DateTime<Utc>> = None;
    let mut model_ms_acc = 0u64;
    let mut tool_ms_acc = 0u64;

    for env in envelopes {
        if let Some(prev) = last_seq {
            if env.seq != prev + 1 && env.seq != prev {
                // allow equal only if duplicate delivery; otherwise gap
                if env.seq > prev + 1 {
                    push_tag(&mut fail_tags, "seq_gap");
                }
            }
        }
        last_seq = Some(env.seq);

        match &env.event {
            RunEvent::RunStarted => {
                saw_run_started = true;
                started_at = Some(env.ts);
            }
            RunEvent::RunCompleted { .. } => {
                terminals.push("RunCompleted".into());
                terminal_at = Some(env.ts);
                // steers after terminal
                // (checked at end for queued leftover; also tag if Injected after)
            }
            RunEvent::RunFailed { error } => {
                terminals.push("RunFailed".into());
                terminal_at = Some(env.ts);
                if error.to_ascii_lowercase().contains("max")
                    && error.to_ascii_lowercase().contains("iter")
                {
                    push_tag(&mut fail_tags, "max_iterations");
                }
                if error.to_ascii_lowercase().contains("max retries")
                    || error.to_ascii_lowercase().contains("recovery")
                {
                    push_tag(&mut fail_tags, "recovery_exhausted");
                }
            }
            RunEvent::RunCancelled { .. } => {
                terminals.push("RunCancelled".into());
                terminal_at = Some(env.ts);
            }
            RunEvent::TurnStarted { .. } => {
                turn_started += 1;
            }
            RunEvent::TurnEnded { .. } => {
                turn_ended += 1;
            }
            RunEvent::ModelCallStarted => {
                model_call_open = Some(env.ts);
            }
            RunEvent::ModelCallEnded { .. } => {
                if let Some(t0) = model_call_open.take() {
                    model_ms_acc += ms_between(t0, env.ts);
                }
            }
            RunEvent::ToolStarted { call_id, name, .. } => {
                open_tools.insert(call_id.clone());
                tool_names.insert(name.clone());
                tool_start_ts.insert(call_id.clone(), env.ts);
                metrics.tool_calls += 1;
            }
            RunEvent::ToolEnded {
                call_id,
                is_error,
                ..
            } => {
                open_tools.remove(call_id);
                if let Some(t0) = tool_start_ts.remove(call_id) {
                    tool_ms_acc += ms_between(t0, env.ts);
                }
                if *is_error {
                    metrics.tool_errors += 1;
                }
            }
            RunEvent::ApprovalRequired { prompt_id, .. } => {
                pending_approvals.insert(prompt_id.clone());
                metrics.approvals += 1;
            }
            RunEvent::ApprovalResolved { prompt_id, .. } => {
                pending_approvals.remove(prompt_id);
            }
            RunEvent::SteerQueued { steer_id, .. } => {
                queued_steers.insert(steer_id.clone());
                metrics.steers += 1;
            }
            RunEvent::SteerInjected { steer_id, .. }
            | RunEvent::SteerCancelled { steer_id, .. }
            | RunEvent::SteerFailed { steer_id, .. } => {
                injected_or_cancelled_steers.insert(steer_id.clone());
                queued_steers.remove(steer_id);
                if terminals.len() > 0 {
                    push_tag(&mut fail_tags, "steer_after_terminal");
                }
            }
            RunEvent::ContextCompacted { .. } => {
                metrics.compactions += 1;
            }
            RunEvent::Error { message } => {
                let lower = message.to_ascii_lowercase();
                if lower.contains("retry") {
                    metrics.retries += 1;
                }
            }
            RunEvent::SubagentStarted { subagent_id, .. } => {
                open_subagents.insert(subagent_id.clone());
            }
            RunEvent::SubagentEnded { subagent_id, .. } => {
                open_subagents.remove(subagent_id);
            }
            RunEvent::ProcessSpawned { child_id, .. } => {
                open_processes.insert(child_id.clone());
            }
            RunEvent::ProcessKilled { child_id, .. } => {
                open_processes.remove(child_id);
            }
            RunEvent::CacheInfo {
                hit_tokens,
                miss_tokens,
                ..
            } => {
                saw_cache_info = true;
                metrics.cache_hit += hit_tokens;
                metrics.cache_miss += miss_tokens;
            }
            RunEvent::CacheSummary {
                total_hit_tokens,
                total_miss_tokens,
                cumulative_hit_rate,
                ..
            } => {
                cache_summary_seen = true;
                // Prefer summary totals if present
                if *total_hit_tokens > 0 || *total_miss_tokens > 0 {
                    metrics.cache_hit = *total_hit_tokens;
                    metrics.cache_miss = *total_miss_tokens;
                    metrics.cache_hit_rate = *cumulative_hit_rate;
                }
            }
            _ => {}
        }
    }

    // Derive wall time
    metrics.wall_ms = opts.wall_ms_override.unwrap_or_else(|| {
        match (started_at, terminal_at) {
            (Some(a), Some(b)) => ms_between(a, b),
            _ => 0,
        }
    });
    metrics.model_ms = model_ms_acc;
    metrics.tool_ms = tool_ms_acc;
    metrics.turns = turn_started.max(turn_ended);
    metrics.unique_tools = {
        let mut v: Vec<_> = tool_names.into_iter().collect();
        v.sort();
        v
    };

    if metrics.cache_hit + metrics.cache_miss > 0 && metrics.cache_hit_rate == 0.0 {
        let total = metrics.cache_hit + metrics.cache_miss;
        metrics.cache_hit_rate = metrics.cache_hit as f64 / total as f64;
    }

    metrics.tokens_in = opts.tokens_in;
    metrics.tokens_out = opts.tokens_out;
    metrics.cost_usd = opts.cost_usd;
    metrics.tokens_estimated = opts.tokens_estimated;

    // ── Harness health tags ────────────────────────────────────────
    if saw_run_started && terminals.is_empty() {
        push_tag(&mut fail_tags, "hung_no_terminal");
    }
    if terminals.len() > 1 {
        push_tag(&mut fail_tags, "double_terminal");
    }
    if !open_tools.is_empty() {
        push_tag(&mut fail_tags, "tool_unpaired");
    }
    if !open_subagents.is_empty() {
        push_tag(&mut fail_tags, "orphan_subagent");
    }
    if !open_processes.is_empty() {
        push_tag(&mut fail_tags, "process_leak");
    }
    if !pending_approvals.is_empty() {
        push_tag(&mut fail_tags, "approval_deadlock");
    }
    if !queued_steers.is_empty() {
        // queued but never injected/cancelled
        let leftover: Vec<_> = queued_steers
            .iter()
            .filter(|id| !injected_or_cancelled_steers.contains(*id))
            .collect();
        if !leftover.is_empty() {
            push_tag(&mut fail_tags, "steer_dropped");
        }
    }

    // max_iterations heuristic: many turns equal to configured max
    if opts.harness.max_iterations > 0 && metrics.turns >= opts.harness.max_iterations as u64 {
        // Only tag if not already completed cleanly with fewer issues —
        // still useful signal when terminal is Failed or we hit the cap.
        if terminals.first().map(|t| t.as_str()) != Some("RunCompleted")
            || metrics.turns >= opts.harness.max_iterations as u64
        {
            // Tag when we hit the configured ceiling (even on Completed-with-summary).
            if metrics.turns >= opts.harness.max_iterations as u64
                && terminals.first().map(|t| t.as_str()) != Some("RunCompleted")
            {
                push_tag(&mut fail_tags, "max_iterations");
            }
        }
    }

    if saw_cache_info && !cache_summary_seen && !terminals.is_empty() {
        // soft: summary missing at end
        push_tag(&mut fail_tags, "cache_ledger_missing");
    }

    fail_tags.sort();
    fail_tags.dedup();

    let terminal = terminals.first().cloned().unwrap_or_else(|| "None".into());

    let harness_ok = taxonomy::harness_fail_count(&fail_tags) == 0;
    let grader_ok = opts.grader_pass.unwrap_or(true);
    let pass = harness_ok && grader_ok;

    if opts.grader_pass == Some(false) {
        push_tag(&mut fail_tags, "grader_fail");
        fail_tags.sort();
        fail_tags.dedup();
    }

    RunLedger {
        task_id: opts.task_id,
        suite: opts.suite,
        mode: opts.mode,
        harness: opts.harness,
        model: opts.model,
        result: RunResult {
            pass,
            grader: opts.grader,
            fail_tags,
            terminal,
            note: opts.note,
        },
        metrics,
        trace_path: opts.trace_path,
        bucket: opts.bucket,
        expect_harness_fail: false,
    }
}

/// Load envelopes from a JSONL file (one Envelope JSON per line).
pub fn load_trace_jsonl(path: &std::path::Path) -> anyhow::Result<Vec<Envelope>> {
    let text = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let env: Envelope = serde_json::from_str(line)
            .map_err(|e| anyhow::anyhow!("trace line {}: {e}", i + 1))?;
        out.push(env);
    }
    Ok(out)
}

fn push_tag(tags: &mut Vec<String>, tag: &str) {
    if !tags.iter().any(|t| t == tag) {
        tags.push(tag.to_string());
    }
}

fn ms_between(a: DateTime<Utc>, b: DateTime<Utc>) -> u64 {
    let d = if b >= a { b - a } else { a - b };
    d.num_milliseconds().max(0) as u64
}

#[allow(dead_code)]
fn _duration_ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::event::RunEvent;
    use chrono::TimeZone;

    fn env(seq: u64, ts_secs: i64, event: RunEvent) -> Envelope {
        Envelope {
            seq,
            event_id: format!("e{seq}"),
            run_id: "run-1".into(),
            session_id: None,
            turn_id: None,
            parent_call_id: None,
            ts: Utc.timestamp_opt(ts_secs, 0).unwrap(),
            event,
        }
    }

    #[test]
    fn happy_path_collects_metrics() {
        let events = vec![
            env(0, 1000, RunEvent::RunCreated {
                id: "run-1".into(),
                session_id: None,
            }),
            env(1, 1000, RunEvent::RunStarted),
            env(2, 1001, RunEvent::TurnStarted { index: 0 }),
            env(3, 1001, RunEvent::ModelCallStarted),
            env(
                4,
                1002,
                RunEvent::ModelCallEnded {
                    text: "hi".into(),
                    tool_count: 1,
                },
            ),
            env(
                5,
                1002,
                RunEvent::ToolStarted {
                    subagent_id: None,
                    call_id: "c1".into(),
                    name: "read_file".into(),
                    args: serde_json::json!({}),
                },
            ),
            env(
                6,
                1003,
                RunEvent::ToolEnded {
                    subagent_id: None,
                    call_id: "c1".into(),
                    name: "read_file".into(),
                    result: "ok".into(),
                    is_error: false,
                },
            ),
            env(7, 1003, RunEvent::TurnEnded { index: 0 }),
            env(
                8,
                1004,
                RunEvent::CacheSummary {
                    total_turns: 1,
                    total_hit_tokens: 10,
                    total_miss_tokens: 5,
                    turns_with_hits: 1,
                    cumulative_hit_rate: 10.0 / 15.0,
                },
            ),
            env(
                9,
                1005,
                RunEvent::RunCompleted {
                    final_text: "done".into(),
                },
            ),
        ];

        let ledger = collect_ledger(
            &events,
            CollectOpts {
                task_id: "L1".into(),
                suite: "contract_v1".into(),
                ..Default::default()
            },
        );

        assert!(ledger.result.pass, "tags={:?}", ledger.result.fail_tags);
        assert_eq!(ledger.result.terminal, "RunCompleted");
        assert_eq!(ledger.metrics.tool_calls, 1);
        assert_eq!(ledger.metrics.turns, 1);
        assert_eq!(ledger.metrics.cache_hit, 10);
        assert_eq!(ledger.metrics.wall_ms, 5000); // 1000→1005 secs = 5s
    }

    #[test]
    fn detects_tool_unpaired_and_hung() {
        let events = vec![
            env(0, 1, RunEvent::RunStarted),
            env(
                1,
                2,
                RunEvent::ToolStarted {
                    subagent_id: None,
                    call_id: "c1".into(),
                    name: "shell".into(),
                    args: serde_json::json!({}),
                },
            ),
        ];
        let ledger = collect_ledger(&events, CollectOpts::default());
        assert!(!ledger.result.pass);
        assert!(ledger.result.fail_tags.iter().any(|t| t == "tool_unpaired"));
        assert!(ledger
            .result
            .fail_tags
            .iter()
            .any(|t| t == "hung_no_terminal"));
    }

    #[test]
    fn detects_seq_gap() {
        let events = vec![
            env(0, 1, RunEvent::RunStarted),
            env(5, 2, RunEvent::RunCompleted {
                final_text: "x".into(),
            }),
        ];
        let ledger = collect_ledger(&events, CollectOpts::default());
        assert!(ledger.result.fail_tags.iter().any(|t| t == "seq_gap"));
    }

    #[test]
    fn loads_sample_fixture_and_passes() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../evals/fixtures/sample_trace.jsonl");
        let events = load_trace_jsonl(&path).expect("load fixture");
        let ledger = collect_ledger(
            &events,
            CollectOpts {
                task_id: "fixture".into(),
                suite: "fixtures".into(),
                ..Default::default()
            },
        );
        assert!(ledger.result.pass, "tags={:?}", ledger.result.fail_tags);
        assert_eq!(ledger.metrics.tool_calls, 1);
        assert_eq!(ledger.metrics.cache_hit, 100);
    }
}
