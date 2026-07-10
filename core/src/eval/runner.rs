//! Eval runner: mock/live suite execution → ledgers + reports.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::broadcast;

use crate::config::Config;
use crate::permission::{ApprovalChoice, PermissionMode};
use crate::runtime::command::RunCommand;
use crate::runtime::event::{Envelope, RunEvent};
use crate::runtime::{Brain, RunManager};
use crate::types::Message;

use super::collector::{collect_ledger, load_trace_jsonl, CollectOpts};
use super::grader::{grade, GradeOutcome};
use super::ledger::{
    EvalMode, HarnessConfig, ModelInfo, RunLedger, SuiteSummary,
};
use super::mock_llm::{start_mock_server, MockScript, MockServer};
use super::prices::{estimate_cost_usd, load_price_table, PriceTable};
use super::reporter::{summarize_suite, write_report};
use super::task::{
    materialize_workspace, ApprovalPolicy, EvalSuite, EvalTask, TaskDriver,
};
use super::taxonomy;

#[derive(Debug, Clone)]
pub struct EvalRunOptions {
    pub suite_dir: PathBuf,
    pub out_dir: PathBuf,
    pub mode: EvalMode,
    /// Model key like `openai/gpt-4.1` or `eval/mock` (mock mode).
    pub model: String,
    pub price_profile: Option<PathBuf>,
    pub git_sha: Option<String>,
    pub variant: Option<String>,
    pub permission_mode: Option<String>,
    pub max_iterations: Option<u32>,
    pub compression: bool,
    /// Fail the process if harness_fail_rate > 0 (CI gate).
    pub gate_harness: bool,
}

pub struct EvalRunResult {
    pub summary: SuiteSummary,
    pub out_dir: PathBuf,
}

/// Run an eval suite and write reports under `opts.out_dir`.
pub async fn run_suite(opts: EvalRunOptions) -> Result<EvalRunResult> {
    let suite = super::task::load_suite(&opts.suite_dir)?;
    let price_table = match &opts.price_profile {
        Some(p) => load_price_table(p).ok(),
        None => {
            // Try default next to suite: ../../prices/*.toml — optional
            None
        }
    };

    let model_info = parse_model_info(&opts.model, opts.price_profile.as_ref());
    let harness = HarnessConfig {
        permission_mode: opts
            .permission_mode
            .clone()
            .unwrap_or_else(|| "yolo".into()),
        max_iterations: opts.max_iterations.unwrap_or(20),
        compression: opts.compression,
        git_sha: opts.git_sha.clone(),
        variant: opts.variant.clone(),
    };

    let mut ledgers = Vec::new();
    for task in &suite.tasks {
        let ledger = run_one_task(
            &suite,
            task,
            &opts,
            &harness,
            &model_info,
            price_table.as_ref(),
        )
        .await
        .with_context(|| format!("task {}", task.manifest.id))?;
        ledgers.push(ledger);
    }

    let summary = summarize_suite(
        &suite.manifest.name,
        opts.mode,
        model_info,
        harness,
        ledgers,
    );
    write_report(&summary, &opts.out_dir)?;

    if opts.gate_harness && summary.harness_health.harness_fail_rate > 0.0 {
        anyhow::bail!(
            "harness_fail_rate={:.0}% — see {}",
            summary.harness_health.harness_fail_rate * 100.0,
            opts.out_dir.join("report.md").display()
        );
    }

    Ok(EvalRunResult {
        summary,
        out_dir: opts.out_dir,
    })
}

async fn run_one_task(
    _suite: &EvalSuite,
    task: &EvalTask,
    opts: &EvalRunOptions,
    harness: &HarnessConfig,
    model_info: &ModelInfo,
    prices: Option<&PriceTable>,
) -> Result<RunLedger> {
    match task.manifest.driver {
        TaskDriver::Trace => run_trace_task(task, opts, harness, model_info).await,
        TaskDriver::Run => run_agent_task(task, opts, harness, model_info, prices).await,
    }
}

async fn run_trace_task(
    task: &EvalTask,
    opts: &EvalRunOptions,
    harness: &HarnessConfig,
    model_info: &ModelInfo,
) -> Result<RunLedger> {
    let rel = task
        .manifest
        .trace
        .as_deref()
        .unwrap_or("trace.jsonl");
    let path = task.dir.join(rel);
    let events = load_trace_jsonl(&path)?;
    let traces_dir = opts.out_dir.join("traces");
    std::fs::create_dir_all(&traces_dir)?;
    let trace_out = traces_dir.join(format!("{}.jsonl", task.manifest.id));
    std::fs::copy(&path, &trace_out)?;

    let mut ledger = collect_ledger(
        &events,
        CollectOpts {
            task_id: task.manifest.id.clone(),
            suite: opts
                .suite_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("suite")
                .to_string(),
            mode: opts.mode,
            harness: harness.clone(),
            model: model_info.clone(),
            grader: format!("{:?}", task.manifest.grader.kind).to_ascii_lowercase(),
            grader_pass: None,
            extra_fail_tags: vec![],
            trace_path: Some(trace_out.display().to_string()),
            bucket: task.manifest.bucket.clone(),
            note: None,
            wall_ms_override: None,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
            tokens_estimated: false,
        },
    );
    ledger.expect_harness_fail = task.manifest.expect_harness_fail;

    let workspace = materialize_workspace(task)?;
    let outcome = grade(&task.manifest.grader, &events, &ledger, &workspace)?;
    apply_grade(&mut ledger, outcome);
    Ok(ledger)
}

async fn run_agent_task(
    task: &EvalTask,
    opts: &EvalRunOptions,
    harness: &HarnessConfig,
    model_info: &ModelInfo,
    prices: Option<&PriceTable>,
) -> Result<RunLedger> {
    let workspace = materialize_workspace(task)?;
    let timeout = Duration::from_secs(task.manifest.timeout_secs.unwrap_or(120));

    let mut mock_server: Option<MockServer> = None;
    let base_url;
    let api_key;
    let model_id;

    match opts.mode {
        EvalMode::Mock => {
            let script = task.script.clone().unwrap_or(MockScript {
                steps: vec![super::mock_llm::MockStep::Text {
                    text: "done".into(),
                    cache_hit: 0,
                    cache_miss: 0,
                }],
            });
            let server = start_mock_server(script).await?;
            base_url = server.base_url.clone();
            api_key = "sk-eval-mock".to_string();
            model_id = "mock".to_string();
            mock_server = Some(server);
        }
        EvalMode::Live => {
            // Resolve from environment / existing config.toml if present.
            let (url, key, mid) = resolve_live_endpoint(&opts.model)?;
            base_url = url;
            api_key = key;
            model_id = mid;
        }
    }

    let perm_mode = task
        .manifest
        .permission_mode
        .as_deref()
        .or(opts.permission_mode.as_deref())
        .unwrap_or(match task.manifest.approval {
            ApprovalPolicy::Yolo => "yolo",
            _ => "standard",
        });

    let max_iter = task
        .manifest
        .max_iterations
        .or(opts.max_iterations)
        .unwrap_or(harness.max_iterations);

    let config_toml = format!(
        r#"
default_model = "eval/mock"
reflector_enabled = false

[providers.eval]
name = "eval"
base_url = "{base_url}"
api_key = "{api_key}"
max_iterations = {max_iter}
max_context_tokens = 128000
request_timeout_secs = 60

[providers.eval.models]
mock = {{ model_id = "{model_id}", max_context_tokens = 128000 }}

[permissions]
mode = "{perm_mode}"

[memory]
mode = "stateless"
embedding_enabled = false
"#
    );

    let mut config: Config = toml::from_str(&config_toml)?;
    config.rebuild_models();
    // Force default model key
    config.default_model = "eval/mock".into();

    let brain = Brain::from_config(config)?;
    let manager = RunManager::new(brain);

    let prompt = if task.manifest.prompt.trim().is_empty() {
        "Say hello and finish.".to_string()
    } else {
        task.manifest.prompt.clone()
    };

    let t0 = Instant::now();
    let run_id = manager
        .create_run_with_workdir(
            &prompt,
            None,
            Some(workspace.display().to_string()),
            Vec::<Message>::new(),
            None,
            false,
        )
        .await?;

    let mut rx = manager.subscribe(&run_id).await?;
    manager.command(&run_id, RunCommand::Start).await?;

    let approval_policy = task.manifest.approval.clone();
    let cancel_after = task.manifest.actions.cancel_after_event.clone();
    let steer_after = task.manifest.actions.steer_after_event.clone();
    let steer_msg = task
        .manifest
        .actions
        .steer_message
        .clone()
        .unwrap_or_else(|| "Please wrap up.".into());
    let action_delay = task.manifest.actions.action_delay_ms;

    let mut cancel_sent = false;
    let mut steer_sent = false;

    let collect = async {
        let mut events: Vec<Envelope> = Vec::new();
        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Ok(env)) => {
                    // Approvals
                    if let RunEvent::ApprovalRequired { prompt_id, .. } = &env.event {
                        let choice = match approval_policy {
                            ApprovalPolicy::AutoDeny => ApprovalChoice::Deny,
                            ApprovalPolicy::AutoAllow | ApprovalPolicy::Yolo => {
                                ApprovalChoice::AllowOnce
                            }
                        };
                        let _ = manager
                            .resolve_approval(Some(&run_id), prompt_id, choice)
                            .await;
                    }

                    let tag = event_tag_name(&env.event);

                    if !cancel_sent {
                        if let Some(ref after) = cancel_after {
                            if tag == after.as_str() {
                                if action_delay > 0 {
                                    tokio::time::sleep(Duration::from_millis(action_delay)).await;
                                }
                                let _ = manager.command(&run_id, RunCommand::Cancel).await;
                                cancel_sent = true;
                            }
                        }
                    }

                    if !steer_sent {
                        if let Some(ref after) = steer_after {
                            if tag == after.as_str() {
                                if action_delay > 0 {
                                    tokio::time::sleep(Duration::from_millis(action_delay)).await;
                                }
                                let steer_id = uuid::Uuid::new_v4().to_string();
                                let _ = manager
                                    .command(
                                        &run_id,
                                        RunCommand::Steer {
                                            steer_id,
                                            message: steer_msg.clone(),
                                        },
                                    )
                                    .await;
                                steer_sent = true;
                            }
                        }
                    }

                    let terminal = matches!(
                        env.event,
                        RunEvent::RunCompleted { .. }
                            | RunEvent::RunCancelled { .. }
                            | RunEvent::RunFailed { .. }
                    );
                    events.push(env);
                    if terminal {
                        break;
                    }
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(broadcast::error::RecvError::Closed)) => break,
                Err(_) => {
                    // idle timeout slice — check overall
                    if t0.elapsed() > timeout {
                        let _ = manager.command(&run_id, RunCommand::Cancel).await;
                        break;
                    }
                }
            }
        }
        events
    };

    let events = tokio::time::timeout(timeout + Duration::from_secs(10), collect)
        .await
        .unwrap_or_else(|_| Vec::new());

    let wall_ms = t0.elapsed().as_millis() as u64;

    // Persist trace
    let traces_dir = opts.out_dir.join("traces");
    std::fs::create_dir_all(&traces_dir)?;
    let trace_out = traces_dir.join(format!("{}.jsonl", task.manifest.id));
    {
        let mut lines = String::new();
        for e in &events {
            lines.push_str(&serde_json::to_string(e)?);
            lines.push('\n');
        }
        std::fs::write(&trace_out, lines)?;
    }

    if let Some(server) = mock_server {
        server.shutdown().await;
    }

    let (tokens_in, tokens_out, cost_usd, tokens_estimated) =
        derive_tokens_cost(&events, model_info, prices);

    let mut harness_cfg = harness.clone();
    harness_cfg.permission_mode = perm_mode.to_string();
    harness_cfg.max_iterations = max_iter;

    let mut ledger = collect_ledger(
        &events,
        CollectOpts {
            task_id: task.manifest.id.clone(),
            suite: opts
                .suite_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("suite")
                .to_string(),
            mode: opts.mode,
            harness: harness_cfg,
            model: model_info.clone(),
            grader: format!("{:?}", task.manifest.grader.kind).to_ascii_lowercase(),
            grader_pass: None,
            extra_fail_tags: vec![],
            trace_path: Some(trace_out.display().to_string()),
            bucket: task.manifest.bucket.clone(),
            note: None,
            wall_ms_override: Some(wall_ms),
            tokens_in,
            tokens_out,
            cost_usd,
            tokens_estimated,
        },
    );
    ledger.expect_harness_fail = task.manifest.expect_harness_fail;

    let outcome = grade(&task.manifest.grader, &events, &ledger, &workspace)?;
    apply_grade(&mut ledger, outcome);
    let _ = taxonomy::harness_fail_count(&ledger.result.fail_tags);
    Ok(ledger)
}

fn apply_grade(ledger: &mut RunLedger, outcome: GradeOutcome) {
    if let Some(note) = outcome.note {
        ledger.result.note = Some(note);
    }
    if !outcome.pass {
        if !ledger.result.fail_tags.iter().any(|t| t == "grader_fail") {
            ledger.result.fail_tags.push("grader_fail".into());
        }
        ledger.result.pass = false;
    } else {
        // Grader already validated (including optional require_fail_tags self-checks).
        ledger.result.pass = true;
    }
}

fn event_tag_name(ev: &RunEvent) -> &'static str {
    // Keep in sync with grader::event_tag
    match ev {
        RunEvent::RunStarted => "run_started",
        RunEvent::ToolStarted { .. } => "tool_started",
        RunEvent::TurnStarted { .. } => "turn_started",
        RunEvent::TurnEnded { .. } => "turn_ended",
        RunEvent::ModelCallStarted => "model_call_started",
        RunEvent::ApprovalRequired { .. } => "approval_required",
        RunEvent::SteerQueued { .. } => "steer_queued",
        RunEvent::RunCompleted { .. } => "run_completed",
        RunEvent::RunCancelled { .. } => "run_cancelled",
        RunEvent::RunFailed { .. } => "run_failed",
        _ => "",
    }
}

fn parse_model_info(model: &str, price_profile: Option<&PathBuf>) -> ModelInfo {
    let (provider, model_id) = if let Some((p, m)) = model.split_once('/') {
        (p.to_string(), m.to_string())
    } else {
        ("eval".into(), model.to_string())
    };
    ModelInfo {
        provider,
        model_id,
        price_profile: price_profile.map(|p| p.display().to_string()),
    }
}

fn resolve_live_endpoint(model: &str) -> Result<(String, String, String)> {
    // Prefer env overrides for CI/local.
    let base_url = std::env::var("OPENAI_BASE_URL")
        .or_else(|_| std::env::var("EVAL_BASE_URL"))
        .unwrap_or_else(|_| "https://api.openai.com/v1".into());
    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("EVAL_API_KEY"))
        .context("OPENAI_API_KEY or EVAL_API_KEY required for live mode")?;
    let model_id = model
        .split_once('/')
        .map(|(_, m)| m.to_string())
        .unwrap_or_else(|| model.to_string());
    Ok((base_url, api_key, model_id))
}

fn derive_tokens_cost(
    events: &[Envelope],
    model: &ModelInfo,
    prices: Option<&PriceTable>,
) -> (u64, u64, f64, bool) {
    let mut hit = 0u64;
    let mut miss = 0u64;
    for e in events {
        if let RunEvent::CacheSummary {
            total_hit_tokens,
            total_miss_tokens,
            ..
        } = &e.event
        {
            hit = *total_hit_tokens;
            miss = *total_miss_tokens;
        }
    }
    // Without prompt/completion usage, estimate input≈hit+miss, output≈0
    let tokens_in = hit + miss;
    let tokens_out = 0;
    let estimated = true;
    let cost = prices
        .map(|p| estimate_cost_usd(p, &model.model_id, tokens_in, tokens_out, hit))
        .unwrap_or(0.0);
    (tokens_in, tokens_out, cost, estimated)
}

/// Discover suite path relative to CWD or repo root.
pub fn resolve_suite_dir(name_or_path: &str) -> Result<PathBuf> {
    let p = PathBuf::from(name_or_path);
    if p.join("suite.toml").exists() {
        return Ok(p);
    }
    let candidates = [
        PathBuf::from("evals/suites").join(name_or_path),
        PathBuf::from("../evals/suites").join(name_or_path),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../evals/suites")
            .join(name_or_path),
    ];
    for c in candidates {
        if c.join("suite.toml").exists() {
            return Ok(c.canonicalize().unwrap_or(c));
        }
    }
    anyhow::bail!("suite not found: {name_or_path}")
}
