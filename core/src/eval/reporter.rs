//! Suite aggregation + JSON/Markdown report writers.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;

use super::ledger::{
    CostRollup, EvalMode, HarnessConfig, HarnessHealth, MatrixReport, MatrixRow, ModelInfo,
    NorthStar, RunLedger, ScorecardRow, SuiteSummary,
};
use super::taxonomy;

/// Aggregate run ledgers into a suite summary.
pub fn summarize_suite(
    suite: &str,
    mode: EvalMode,
    model: ModelInfo,
    harness: HarnessConfig,
    runs: Vec<RunLedger>,
) -> SuiteSummary {
    let n_tasks = runs.len();
    let n_pass = runs.iter().filter(|r| r.result.pass).count();
    let pass_at_1 = rate(n_pass, n_tasks);

    let harness_health = compute_harness_health(&runs);
    let scorecard = compute_scorecard(&runs);
    let cost = compute_cost(&runs, n_pass);

    let all_walls: Vec<u64> = runs.iter().map(|r| r.metrics.wall_ms).collect();
    let all_tools: Vec<f64> = runs.iter().map(|r| r.metrics.tool_calls as f64).collect();

    let north_star = NorthStar {
        pass_at_1,
        usd_per_successful_task: cost.usd_per_pass,
        p90_wall_ms: percentile_u64(&all_walls, 0.90),
        median_tool_calls: median_f64(&all_tools),
        harness_fail_rate: harness_health.harness_fail_rate,
    };

    SuiteSummary {
        suite: suite.to_string(),
        mode,
        model,
        harness,
        generated_at: Utc::now().to_rfc3339(),
        n_tasks,
        n_pass,
        pass_at_1,
        harness_health,
        scorecard,
        cost,
        north_star,
        runs,
    }
}

fn compute_harness_health(runs: &[RunLedger]) -> HarnessHealth {
    let n = runs.len().max(1) as f64;
    let tag_rate = |tag: &str| {
        runs.iter()
            .filter(|r| r.result.fail_tags.iter().any(|t| t == tag))
            .count() as f64
            / n
    };
    let harness_fail_rate = runs
        .iter()
        .filter(|r| taxonomy::harness_fail_count(&r.result.fail_tags) > 0)
        .count() as f64
        / n;

    HarnessHealth {
        hung_no_terminal: tag_rate("hung_no_terminal"),
        tool_unpaired: tag_rate("tool_unpaired"),
        approval_deadlock: tag_rate("approval_deadlock"),
        orphan_subagent: tag_rate("orphan_subagent"),
        max_iterations: tag_rate("max_iterations"),
        seq_gap: tag_rate("seq_gap"),
        process_leak: tag_rate("process_leak"),
        steer_dropped: tag_rate("steer_dropped"),
        harness_fail_rate,
    }
}

fn compute_scorecard(runs: &[RunLedger]) -> Vec<ScorecardRow> {
    let mut by_bucket: HashMap<String, Vec<&RunLedger>> = HashMap::new();
    for r in runs {
        let b = r
            .bucket
            .clone()
            .unwrap_or_else(|| "ALL".to_string());
        by_bucket.entry(b).or_default().push(r);
    }
    // Always include ALL
    by_bucket
        .entry("ALL".into())
        .or_insert_with(|| runs.iter().collect());

    let mut rows: Vec<_> = by_bucket
        .into_iter()
        .map(|(bucket, rs)| {
            let n = rs.len();
            let n_pass = rs.iter().filter(|r| r.result.pass).count();
            let walls: Vec<u64> = rs.iter().map(|r| r.metrics.wall_ms).collect();
            let turns: Vec<f64> = rs.iter().map(|r| r.metrics.turns as f64).collect();
            let tools: Vec<f64> = rs.iter().map(|r| r.metrics.tool_calls as f64).collect();
            let cache_rates: Vec<f64> = rs.iter().map(|r| r.metrics.cache_hit_rate).collect();
            let pass_cost: f64 = rs
                .iter()
                .filter(|r| r.result.pass)
                .map(|r| r.metrics.cost_usd)
                .sum();
            ScorecardRow {
                bucket,
                n,
                pass_at_1: rate(n_pass, n),
                p50_wall_ms: percentile_u64(&walls, 0.50),
                p90_wall_ms: percentile_u64(&walls, 0.90),
                median_turns: median_f64(&turns),
                median_tool_calls: median_f64(&tools),
                usd_per_pass: if n_pass > 0 {
                    pass_cost / n_pass as f64
                } else {
                    0.0
                },
                cache_hit_rate: mean_f64(&cache_rates),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.bucket.cmp(&b.bucket));
    rows
}

fn compute_cost(runs: &[RunLedger], n_pass: usize) -> CostRollup {
    let total_usd: f64 = runs.iter().map(|r| r.metrics.cost_usd).sum();
    let usd_on_failures: f64 = runs
        .iter()
        .filter(|r| !r.result.pass)
        .map(|r| r.metrics.cost_usd)
        .sum();
    let pass_cost: f64 = runs
        .iter()
        .filter(|r| r.result.pass)
        .map(|r| r.metrics.cost_usd)
        .sum();
    CostRollup {
        total_usd,
        usd_on_failures,
        usd_per_pass: if n_pass > 0 {
            pass_cost / n_pass as f64
        } else {
            0.0
        },
    }
}

/// Write `summary.json`, `report.md`, and per-run JSON files under `out_dir`.
pub fn write_report(summary: &SuiteSummary, out_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let runs_dir = out_dir.join("runs");
    std::fs::create_dir_all(&runs_dir)?;

    for run in &summary.runs {
        let path = runs_dir.join(format!("{}.json", sanitize(&run.task_id)));
        let json = serde_json::to_string_pretty(run)?;
        std::fs::write(&path, json)
            .with_context(|| format!("write {}", path.display()))?;
    }

    let summary_path = out_dir.join("summary.json");
    std::fs::write(&summary_path, serde_json::to_string_pretty(summary)?)?;

    let md = render_report_md(summary);
    std::fs::write(out_dir.join("report.md"), md)?;

    Ok(())
}

/// Write a multi-model / ablation matrix report.
pub fn write_matrix(matrix: &MatrixReport, out_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;
    std::fs::write(
        out_dir.join("matrix.json"),
        serde_json::to_string_pretty(matrix)?,
    )?;
    std::fs::write(out_dir.join("matrix.md"), render_matrix_md(matrix))?;
    Ok(())
}

pub fn matrix_from_summaries(suite: &str, kind: &str, summaries: &[SuiteSummary]) -> MatrixReport {
    let rows = summaries
        .iter()
        .map(|s| MatrixRow {
            label: s
                .harness
                .variant
                .clone()
                .unwrap_or_else(|| format!("{}/{}", s.model.provider, s.model.model_id)),
            model_id: s.model.model_id.clone(),
            variant: s.harness.variant.clone(),
            pass_at_1: s.pass_at_1,
            p50_wall_ms: s
                .scorecard
                .iter()
                .find(|r| r.bucket == "ALL")
                .map(|r| r.p50_wall_ms)
                .unwrap_or(0),
            median_turns: s
                .scorecard
                .iter()
                .find(|r| r.bucket == "ALL")
                .map(|r| r.median_turns)
                .unwrap_or(0.0),
            usd_per_pass: s.cost.usd_per_pass,
            harness_fail_rate: s.harness_health.harness_fail_rate,
            n_tasks: s.n_tasks,
            n_pass: s.n_pass,
        })
        .collect();
    MatrixReport {
        suite: suite.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        kind: kind.to_string(),
        rows,
    }
}

pub fn render_report_md(summary: &SuiteSummary) -> String {
    let mut out = String::new();
    out.push_str("# Harness Eval Report\n\n");
    out.push_str(&format!("suite: {} ({} tasks)\n", summary.suite, summary.n_tasks));
    out.push_str(&format!("mode: {}\n", summary.mode.as_str()));
    out.push_str(&format!(
        "model: {}/{}\n",
        summary.model.provider, summary.model.model_id
    ));
    out.push_str(&format!(
        "harness: {} / compress={} / max_iter={}{}\n",
        summary.harness.permission_mode,
        summary.harness.compression,
        summary.harness.max_iterations,
        summary
            .harness
            .git_sha
            .as_ref()
            .map(|s| format!(" / sha={s}"))
            .unwrap_or_default()
    ));
    if let Some(v) = &summary.harness.variant {
        out.push_str(&format!("variant: {v}\n"));
    }
    out.push_str(&format!("generated: {}\n\n", summary.generated_at));

    out.push_str("## North Star\n\n");
    out.push_str("| pass@1 | $/pass | p90 wall | med tools | harness_fail |\n");
    out.push_str("|--------|--------|----------|-----------|--------------|\n");
    out.push_str(&format!(
        "| {:.0}% | ${:.4} | {}ms | {:.1} | {:.0}% |\n\n",
        summary.north_star.pass_at_1 * 100.0,
        summary.north_star.usd_per_successful_task,
        summary.north_star.p90_wall_ms,
        summary.north_star.median_tool_calls,
        summary.north_star.harness_fail_rate * 100.0,
    ));

    out.push_str("## Scorecard\n\n");
    out.push_str(
        "| bucket | n | pass@1 | p50 wall | p90 wall | med turns | med tools | $/pass | cache hit |\n",
    );
    out.push_str(
        "|--------|---|--------|----------|----------|-----------|-----------|--------|-----------|\n",
    );
    for row in &summary.scorecard {
        out.push_str(&format!(
            "| {} | {} | {:.0}% | {}ms | {}ms | {:.1} | {:.1} | ${:.4} | {:.0}% |\n",
            row.bucket,
            row.n,
            row.pass_at_1 * 100.0,
            row.p50_wall_ms,
            row.p90_wall_ms,
            row.median_turns,
            row.median_tool_calls,
            row.usd_per_pass,
            row.cache_hit_rate * 100.0,
        ));
    }
    out.push('\n');

    out.push_str("## Harness health\n\n");
    out.push_str("| check | rate |\n|-------|------|\n");
    let h = &summary.harness_health;
    for (name, val) in [
        ("hung_no_terminal", h.hung_no_terminal),
        ("tool_unpaired", h.tool_unpaired),
        ("approval_deadlock", h.approval_deadlock),
        ("orphan_subagent", h.orphan_subagent),
        ("max_iterations", h.max_iterations),
        ("seq_gap", h.seq_gap),
        ("process_leak", h.process_leak),
        ("steer_dropped", h.steer_dropped),
        ("harness_fail_rate", h.harness_fail_rate),
    ] {
        out.push_str(&format!("| {name} | {:.0}% |\n", val * 100.0));
    }
    out.push('\n');

    out.push_str("## Failures\n\n");
    let failures: Vec<_> = summary.runs.iter().filter(|r| !r.result.pass).collect();
    if failures.is_empty() {
        out.push_str("_None_\n\n");
    } else {
        out.push_str("| task | tags | turns | $ | terminal | note |\n");
        out.push_str("|------|------|-------|---|----------|------|\n");
        for r in failures {
            out.push_str(&format!(
                "| {} | {} | {} | {:.4} | {} | {} |\n",
                r.task_id,
                r.result.fail_tags.join(","),
                r.metrics.turns,
                r.metrics.cost_usd,
                r.result.terminal,
                r.result.note.as_deref().unwrap_or(""),
            ));
        }
        out.push('\n');
    }

    out.push_str("## Cost rollup\n\n");
    out.push_str(&format!(
        "total_usd: {:.4} | usd_on_failures: {:.4} | usd_per_pass: {:.4}\n",
        summary.cost.total_usd, summary.cost.usd_on_failures, summary.cost.usd_per_pass
    ));

    out
}

pub fn render_matrix_md(matrix: &MatrixReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Eval Matrix ({})\n\nsuite: {}\ngenerated: {}\n\n",
        matrix.kind, matrix.suite, matrix.generated_at
    ));
    out.push_str(
        "| label | model | pass@1 | p50 wall | med turns | $/pass | harness_fail% | n |\n",
    );
    out.push_str(
        "|-------|-------|--------|----------|-----------|--------|---------------|---|\n",
    );
    for r in &matrix.rows {
        out.push_str(&format!(
            "| {} | {} | {:.0}% | {}ms | {:.1} | ${:.4} | {:.0}% | {}/{} |\n",
            r.label,
            r.model_id,
            r.pass_at_1 * 100.0,
            r.p50_wall_ms,
            r.median_turns,
            r.usd_per_pass,
            r.harness_fail_rate * 100.0,
            r.n_pass,
            r.n_tasks,
        ));
    }
    out.push_str(
        "\n_harness_fail% = share of runs with scaffolding tags (hung/unpaired/deadlock/…)._\n",
    );
    out
}

fn rate(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64
    }
}

fn percentile_u64(values: &[u64], p: f64) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let idx = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[idx.min(v.len() - 1)]
}

fn median_f64(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    if v.len() % 2 == 0 {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    }
}

fn mean_f64(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::ledger::{RunMetrics, RunResult};

    fn sample_run(id: &str, pass: bool, wall: u64, cost: f64) -> RunLedger {
        RunLedger {
            task_id: id.into(),
            suite: "t".into(),
            mode: EvalMode::Mock,
            harness: HarnessConfig::default(),
            model: ModelInfo {
                provider: "test".into(),
                model_id: "mock".into(),
                price_profile: None,
            },
            result: RunResult {
                pass,
                grader: "x".into(),
                fail_tags: if pass {
                    vec![]
                } else {
                    vec!["tool_unpaired".into()]
                },
                terminal: "RunCompleted".into(),
                note: None,
            },
            metrics: RunMetrics {
                wall_ms: wall,
                tool_calls: 2,
                turns: 3,
                cost_usd: cost,
                ..Default::default()
            },
            trace_path: None,
            bucket: Some("edit".into()),
        }
    }

    #[test]
    fn summarize_and_render() {
        let runs = vec![
            sample_run("a", true, 1000, 0.01),
            sample_run("b", false, 5000, 0.05),
        ];
        let summary = summarize_suite(
            "contract_v1",
            EvalMode::Mock,
            ModelInfo {
                provider: "test".into(),
                model_id: "mock".into(),
                price_profile: None,
            },
            HarnessConfig {
                permission_mode: "yolo".into(),
                max_iterations: 10,
                compression: true,
                git_sha: None,
                variant: None,
            },
            runs,
        );
        assert_eq!(summary.n_pass, 1);
        assert!((summary.pass_at_1 - 0.5).abs() < 1e-9);
        assert!(summary.harness_health.harness_fail_rate > 0.0);
        let md = render_report_md(&summary);
        assert!(md.contains("North Star"));
        assert!(md.contains("Harness health"));
    }
}
