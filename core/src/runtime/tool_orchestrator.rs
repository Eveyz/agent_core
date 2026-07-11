#![allow(deprecated)]
use crate::runtime::input::{
    format_answers_for_model, parse_ask_user_args, validate_answers, ClarificationAnswers,
    InputResolver,
};
use crate::runtime::tool_scheduler::{DepGraph, SchedNode, classify_resources};
use crate::hooks::{HookRegistry, PreToolResult};
use crate::permission::{
    ApprovalChoice, ApprovalScope, PermissionDecision, PermissionPolicy, ToolPermissionPattern,
    WhitelistEntry,
};
use crate::runtime::ApprovalResolver;
use crate::tools::{ToolRegistry, ToolUpdateFn};
use crate::types::{AgentEvent, ToolCall, ToolExecutionMode};
use futures::stream::{FuturesUnordered, StreamExt};
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

pub struct ToolOrchestrator<'a> {
    pub registry: &'a ToolRegistry,
    pub permission_policy: &'a mut PermissionPolicy,
    pub hook_registry: Arc<Mutex<HookRegistry>>,
    pub tool_execution_mode: ToolExecutionMode,
    pub cancel_token: CancellationToken,
    /// Per-Run approval resolver. If `None`, falls back to the global map
    /// (for backward compat with the old Agent path).
    pub approval_resolver: Option<ApprovalResolver>,
    /// Per-Run clarification resolver for `ask_user`.
    pub input_resolver: Option<InputResolver>,
    pub session_id: Option<String>,
    pub working_dir: Option<String>,
}

impl<'a> ToolOrchestrator<'a> {
    #[tracing::instrument(skip_all, fields(tool_count = calls.len()))]
    pub async fn execute_tools<F>(&mut self, calls: &[ToolCall], on_event: &F) -> Vec<String>
    where
        F: Fn(AgentEvent, &str) + Send + Sync,
    {
        // If ask_user is in this batch, run it alone as a pre-work gate.
        // Other tools in the same model response are deferred with a clear
        // message so the agent re-issues them after clarification.
        if let Some(ask_idx) = calls.iter().position(|c| c.function.name == "ask_user") {
            let mut results = vec![
                "Deferred: ask_user must complete before other tools. \
                 Re-issue this tool on the next turn after clarification."
                    .to_string();
                calls.len()
            ];
            let ask_call = &calls[ask_idx];
            let args: Value =
                serde_json::from_str(&ask_call.function.arguments).unwrap_or_default();
            results[ask_idx] = self.execute_ask_user(ask_call, args, on_event).await;
            return results;
        }

        // Resolve execution mode (per-tool override wins)
        let mode = self
            .registry
            .resolve_execution_mode(calls, self.tool_execution_mode);

        // Always preflight sequentially (permission + hooks).
        // This is the critical "before any tool body runs" window — UI should
        // already show tool_started once preflight finishes for that call.
        let preflight_t0 = Instant::now();
        tracing::info!(
            tool_count = calls.len(),
            tools = ?calls.iter().map(|c| &c.function.name).collect::<Vec<_>>(),
            "LATENCY: preflight begin"
        );
        let mut allowed: Vec<(usize, ToolCall, Value)> = Vec::new();
        let mut results = vec![String::new(); calls.len()];
        let mut first_tool_started_logged = false;

        for (i, call) in calls.iter().enumerate() {
            let tool_preflight_t0 = Instant::now();
            let args: Value = serde_json::from_str(&call.function.arguments).unwrap_or_default();

            // Permission check — layered: Deny → Ask(with approval) → Allow
            // Extract command/path/host for fine-grained matching; these are
            // also reused below to scope any approval the user grants, so that
            // e.g. allowing one `bash` command does not allow every command.
            let command = args.get("command").and_then(|v| v.as_str());
            let path = args
                .get("path")
                .or(args.get("file_path"))
                .or(args.get("file"))
                .or(args.get("working_dir"))
                .and_then(|v| v.as_str());
            let host = args
                .get("url")
                .or(args.get("host"))
                .and_then(|v| v.as_str());

            let perm_t0 = Instant::now();
            let decision = self.permission_policy.check(
                &call.function.name,
                &call.function.arguments,
                command,
                path,
                host,
            );
            let perm_check_ms = perm_t0.elapsed().as_millis() as u64;

            // debug: check itself is sync/cheap; keep for profiling, not prod noise.
            tracing::debug!(
                tool = %call.function.name,
                call_id = %call.id,
                decision = ?decision,
                check_ms = perm_check_ms,
                has_resolver = self.approval_resolver.is_some(),
                "LATENCY: permission check"
            );

            match decision {
                PermissionDecision::Deny(reason) => {
                    results[i] = format!("Permission denied: {}", reason);
                    continue;
                }
                PermissionDecision::Ask(_reason, prompt) => {
                    // Create oneshot channel for approval
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let using_resolver = self.approval_resolver.is_some();
                    let approval_wait_t0 = Instant::now();
                    tracing::info!(
                        pid = %prompt.prompt_id,
                        tool = %prompt.tool_name,
                        call_id = %call.id,
                        has_resolver = using_resolver,
                        "LATENCY: approval wait begin"
                    );
                    if let Some(ref resolver) = self.approval_resolver {
                        resolver.insert(prompt.prompt_id.clone(), tx);
                    } else {
                        // Fallback: global map (old Agent path)
                        let pending_arc = crate::permission::global_pending_approvals();
                        let mut pending = pending_arc.lock();
                        pending.insert(prompt.prompt_id.clone(), tx);
                    }

                    // Emit approval event
                    on_event(
                        AgentEvent::ApprovalRequired {
                            prompt_id: prompt.prompt_id.clone(),
                            tool_name: prompt.tool_name.clone(),
                            tool_input: prompt.tool_input.clone(),
                            danger_level: format!("{:?}", prompt.danger_level),
                            explanation: prompt.explanation.clone(),
                        },
                        &call.id,
                    );

                    // Wait for user response, but also listen for cancellation so
                    // that an agent stuck on `ApprovalRequired` can be stopped
                    // instead of blocking forever.
                    let outcome: Result<ApprovalChoice, ()> = tokio::select! {
                        choice = rx => choice.map_err(|_| ()),
                        _ = self.cancel_token.cancelled() => Err(()),
                    };

                    tracing::info!(
                        tool = %call.function.name,
                        call_id = %call.id,
                        resolved = outcome.is_ok(),
                        wait_ms = approval_wait_t0.elapsed().as_millis() as u64,
                        "LATENCY: approval wait done"
                    );

                    match outcome {
                        Ok(choice) => {
                            on_event(
                                AgentEvent::ApprovalResolved {
                                    prompt_id: prompt.prompt_id.clone(),
                                    choice: format!("{choice:?}"),
                                },
                                &call.id,
                            );
                            match &choice {
                                ApprovalChoice::Deny | ApprovalChoice::DenyPersistent => {
                                    results[i] = format!(
                                        "Permission denied by user: tool '{}' was not approved",
                                        call.function.name
                                    );

                                    // Add to blacklist if persistent deny
                                    if matches!(choice, ApprovalChoice::DenyPersistent) {
                                        self.permission_policy.add_rule(
                                            crate::permission::ConfigRule {
                                                pattern: scoped_pattern(
                                                    &call.function.name,
                                                    command,
                                                    path,
                                                    host,
                                                ),
                                                level: crate::permission::ApprovalLevel::Deny,
                                            },
                                        );
                                    }
                                    continue;
                                }
                                ApprovalChoice::AllowOnce => {
                                    // One-time allow — just proceed
                                }
                                ApprovalChoice::AllowSession => {
                                    // Add to session whitelist
                                    self.permission_policy.whitelist_mut().add(
                                        WhitelistEntry::new(
                                            scoped_pattern(
                                                &call.function.name,
                                                command,
                                                path,
                                                host,
                                            ),
                                            ApprovalScope::Session,
                                        ),
                                    );
                                }
                                ApprovalChoice::AllowFor(duration) => {
                                    let secs = duration.as_secs();
                                    let dur_str = if secs >= 3600 {
                                        format!("{}h", secs / 3600)
                                    } else if secs >= 60 {
                                        format!("{}m", secs / 60)
                                    } else {
                                        format!("{}s", secs)
                                    };
                                    self.permission_policy.whitelist_mut().add(
                                        WhitelistEntry::new(
                                            scoped_pattern(
                                                &call.function.name,
                                                command,
                                                path,
                                                host,
                                            ),
                                            ApprovalScope::Duration(dur_str),
                                        ),
                                    );
                                }
                                ApprovalChoice::AllowPersistent => {
                                    self.permission_policy.whitelist_mut().add(
                                        WhitelistEntry::new(
                                            scoped_pattern(
                                                &call.function.name,
                                                command,
                                                path,
                                                host,
                                            ),
                                            ApprovalScope::Persistent,
                                        ),
                                    );
                                }
                            }
                            // Clean up pending approval
                            if let Some(ref resolver) = self.approval_resolver {
                                resolver.remove(&prompt.prompt_id);
                            } else {
                                let pending_arc = crate::permission::global_pending_approvals();
                                let mut pending = pending_arc.lock();
                                pending.remove(&prompt.prompt_id);
                            }
                        }
                        Err(_) => {
                            // Channel closed or run cancelled → deny and stop.
                            if let Some(ref resolver) = self.approval_resolver {
                                resolver.remove(&prompt.prompt_id);
                            } else {
                                let pending_arc = crate::permission::global_pending_approvals();
                                let mut pending = pending_arc.lock();
                                pending.remove(&prompt.prompt_id);
                            }
                            if self.cancel_token.is_cancelled() {
                                results[i] = "Aborted".to_string();
                            } else {
                                results[i] =
                                    format!("Approval cancelled for tool '{}'", call.function.name);
                            }
                            continue;
                        }
                    }
                }
                PermissionDecision::Allow => {
                    // Allowed, no action needed
                }
            }

            // Pre-tool hook
            let hook_t0 = Instant::now();
            let hook_result = self
                .hook_registry
                .lock()
                .fire_pre_tool_use(&call.function.name, &args);
            let hook_ms = hook_t0.elapsed().as_millis() as u64;
            tracing::debug!(
                tool = %call.function.name,
                call_id = %call.id,
                hook_ms,
                "LATENCY: pre_tool hook"
            );

            match hook_result {
                PreToolResult::Veto(reason) => {
                    results[i] = format!("Hook vetoed: {}", reason);
                    continue;
                }
                PreToolResult::Proceed(modified_args) => {
                    let preflight_tool_ms = tool_preflight_t0.elapsed().as_millis() as u64;
                    // First tool_started in the batch is the UI inflection point —
                    // keep at info so prod can see "preflight → visible tool".
                    if !first_tool_started_logged {
                        first_tool_started_logged = true;
                        tracing::info!(
                            tool = %call.function.name,
                            call_id = %call.id,
                            batch_index = i,
                            perm_ms = perm_check_ms,
                            hook_ms,
                            preflight_tool_ms,
                            since_preflight_ms = preflight_t0.elapsed().as_millis() as u64,
                            args_chars = call.function.arguments.len(),
                            "LATENCY: first tool_started emit"
                        );
                    } else {
                        tracing::debug!(
                            tool = %call.function.name,
                            call_id = %call.id,
                            batch_index = i,
                            perm_ms = perm_check_ms,
                            hook_ms,
                            preflight_tool_ms,
                            args_chars = call.function.arguments.len(),
                            "LATENCY: tool_started emit"
                        );
                    }
                    on_event(
                        AgentEvent::ToolExecutionStart {
                            tool_call_id: call.id.clone(),
                            tool_name: call.function.name.clone(),
                            args: modified_args.clone(),
                        },
                        &call.id,
                    );
                    allowed.push((i, call.clone(), modified_args));
                }
            }
        }

        tracing::info!(
            allowed = allowed.len(),
            denied_or_skipped = calls.len().saturating_sub(allowed.len()),
            preflight_ms = preflight_t0.elapsed().as_millis() as u64,
            "LATENCY: preflight done"
        );

        if allowed.is_empty() {
            return results;
        }

        // ── Execution stage: DAG-scheduled. ────────────────────────────────
        // We don't blindly parallelize (calls may mutate the same file) nor
        // blindly serialize (independent calls waste wall-clock). We build a
        // dependency graph keyed on the resources each call touches and run
        // topologically: calls with no outstanding predecessors run
        // concurrently on a FuturesUnordered, and each completion releases its
        // dependents. Sequential mode collapses to a linear chain.
        let nodes: Vec<SchedNode> = allowed
            .iter()
            .map(|(idx, call, args)| {
                let (mutations, reads) = classify_resources(&call.function.name, args);
                SchedNode {
                    idx: *idx,
                    tool_name: call.function.name.clone(),
                    tool_call_id: call.id.clone(),
                    args: args.clone(),
                    mutations,
                    reads,
                }
            })
            .collect();
        let graph = DepGraph::build(&nodes, mode);

        let ready_count = graph.indegree.iter().filter(|&&d| d == 0).count();
        tracing::info!(
            allowed = nodes.len(),
            ready_now = ready_count,
            mode = ?mode,
            "LATENCY: tool bodies begin"
        );

        // Each in-flight future returns (node_idx, output), so completions map
        // straight back to the graph without a separate slot table.
        let mut indegree = graph.indegree.clone();
        let dependents = graph.dependents.clone();
        let mut in_flight: FuturesUnordered<_> = FuturesUnordered::new();

        // Seed: launch every node with indegree 0.
        for (node_idx, node) in nodes.iter().enumerate() {
            if self.cancel_token.is_cancelled() {
                results[node.idx] = "Aborted".to_string();
                indegree[node_idx] = usize::MAX; // mark so we never launch
            } else if indegree[node_idx] == 0 {
                in_flight.push(self.run_node(node_idx, node, on_event));
            }
        }

        loop {
            if in_flight.is_empty() {
                break;
            }
            let (done_node_idx, output) = tokio::select! {
                Some((idx, res)) = in_flight.next() => (idx, res),
                _ = self.cancel_token.cancelled() => {
                    // On cancel, stop launching; remaining in-flight will be
                    // drained/aborted below.
                    break;
                }
            };
            results[nodes[done_node_idx].idx] = output;

            // Release dependents whose predecessors are all done.
            for &dep in &dependents[done_node_idx] {
                if indegree[dep] > 0 && indegree[dep] != usize::MAX {
                    indegree[dep] -= 1;
                }
                if indegree[dep] == 0 && !self.cancel_token.is_cancelled() {
                    in_flight.push(self.run_node(dep, &nodes[dep], on_event));
                }
            }
        }

        // Anything still unfinished (cancelled mid-flight) is aborted.
        for node in nodes.iter() {
            if results[node.idx].is_empty() {
                results[node.idx] = "Aborted".to_string();
            }
        }

        // Post-tool hooks
        for (i, call, args) in &allowed {
            let output = results[*i].clone();
            let is_error = crate::runtime::execution::tool_result_is_error(&output);

            if !is_error {
                let final_output =
                    self.hook_registry
                        .lock()
                        .fire_post_tool_use(&call.function.name, args, &output);
                results[*i] = final_output;
            }
        }

        results
    }

    /// Block on `ask_user`: emit InputRequested, await InputResolver.
    async fn execute_ask_user<F>(
        &self,
        call: &ToolCall,
        args: Value,
        on_event: &F,
    ) -> String
    where
        F: Fn(AgentEvent, &str) + Send + Sync,
    {
        let request = match parse_ask_user_args(&args) {
            Ok(r) => r,
            Err(e) => return format!("Error: {e}"),
        };

        let Some(ref resolver) = self.input_resolver else {
            return "Error: ask_user requires a live input channel (not available in this context). \
                    Ask clarifying questions in your next assistant message instead."
                .to_string();
        };

        on_event(
            AgentEvent::ToolExecutionStart {
                tool_call_id: call.id.clone(),
                tool_name: call.function.name.clone(),
                args: args.clone(),
            },
            &call.id,
        );

        let (tx, rx) = tokio::sync::oneshot::channel::<ClarificationAnswers>();
        resolver.insert(request.prompt_id.clone(), tx);

        let ask_wait_t0 = Instant::now();
        tracing::info!(
            call_id = %call.id,
            prompt_id = %request.prompt_id,
            question_count = request.questions.len(),
            "LATENCY: ask_user wait begin"
        );

        on_event(
            AgentEvent::InputRequested {
                prompt_id: request.prompt_id.clone(),
                title: request.title.clone(),
                questions: request.questions.clone(),
            },
            &call.id,
        );

        let outcome: Result<ClarificationAnswers, ()> = tokio::select! {
            answers = rx => answers.map_err(|_| ()),
            _ = self.cancel_token.cancelled() => Err(()),
        };

        tracing::info!(
            call_id = %call.id,
            prompt_id = %request.prompt_id,
            resolved = outcome.is_ok(),
            wait_ms = ask_wait_t0.elapsed().as_millis() as u64,
            "LATENCY: ask_user wait done"
        );

        resolver.remove(&request.prompt_id);

        match outcome {
            Ok(answers) => match validate_answers(&request, &answers) {
                Ok(cleaned) => format_answers_for_model(&request, &cleaned),
                Err(e) => format!("Error: invalid clarification answers: {e}"),
            },
            Err(_) => {
                if self.cancel_token.is_cancelled() {
                    "Aborted".to_string()
                } else {
                    "Error: clarification cancelled — no answers received from the user.".to_string()
                }
            }
        }
    }

    /// Run one scheduled node to completion, respecting cancellation. Returns
    /// `(node_idx, output)` so the scheduler can fan completions back to the
    /// right slot without a separate index map.
    async fn run_node<F>(&self, node_idx: usize, node: &SchedNode, on_event: &F) -> (usize, String)
    where
        F: Fn(AgentEvent, &str) + Send + Sync,
    {
        let cancel = self.cancel_token.clone();
        let out = tokio::select! {
            res = self.execute_single_tool(
                &node.tool_name,
                &node.tool_call_id,
                node.args.clone(),
                cancel,
                on_event,
            ) => res,
            _ = self.cancel_token.cancelled() => "Aborted".to_string(),
        };
        (node_idx, out)
    }

    #[tracing::instrument(skip_all, fields(tool = %tool_name, id = %tool_call_id))]
    async fn execute_single_tool<F>(
        &self,
        tool_name: &str,
        tool_call_id: &str,
        args: serde_json::Value,
        cancel_token: CancellationToken,
        on_event: &F,
    ) -> String
    where
        F: Fn(AgentEvent, &str) + Send + Sync,
    {
        let body_t0 = Instant::now();
        tracing::debug!(
            tool = %tool_name,
            call_id = %tool_call_id,
            "LATENCY: tool body begin"
        );

        let tool = match self.registry.get(tool_name) {
            Some(t) => t,
            None => {
                return format!(
                    "Tool '{}' not found. Available: {}",
                    tool_name,
                    self.registry.list_names().join(", ")
                );
            }
        };

        let mut modified_args = args;
        if let Some(ref sid) = self.session_id {
            if let Some(obj) = modified_args.as_object_mut() {
                obj.insert("_session_id".to_string(), serde_json::Value::String(sid.clone()));
            }
        }
        if let Some(ref wd) = self.working_dir {
            if let Some(obj) = modified_args.as_object_mut() {
                obj.insert("_working_dir".to_string(), serde_json::Value::String(wd.clone()));
            }
        }

        // Create event channel for tools that emit structured events (e.g. subagent) and streaming updates.
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();

        let event_tx_clone = event_tx.clone();
        let tool_call_id_clone = tool_call_id.to_string();
        let tool_name_clone = tool_name.to_string();
        let cancel_clone = cancel_token.clone();

        let on_update: ToolUpdateFn = Arc::new(move |partial: &str| {
            if !cancel_clone.is_cancelled() {
                let _ = event_tx_clone.send(AgentEvent::ToolExecutionUpdate {
                    tool_call_id: tool_call_id_clone.clone(),
                    tool_name: tool_name_clone.clone(),
                    partial_result: partial.to_string(),
                });
            }
        });

        // Forward tool-internal events to the main event stream in real
        // time so the TUI can render them as they happen.
        let tool_fut = tool.execute_with_stream(modified_args, Some(on_update), Some(event_tx));
        let drain_fut = async {
            while let Some(event) = event_rx.recv().await {
                on_event(event, tool_call_id);
            }
        };

        let result = tokio::select! {
            res = async { tokio::join!(tool_fut, drain_fut) } => res.0,
            _ = cancel_token.cancelled() => {
                Err(anyhow::anyhow!("Tool execution was cancelled by the user."))
            }
        };

        // Drain any remaining events.
        while let Ok(event) = event_rx.try_recv() {
            on_event(event, tool_call_id);
        }

        let body_ms = body_t0.elapsed().as_millis() as u64;
        match result {
            Ok(output) => {
                tracing::debug!(
                    tool = %tool_name,
                    call_id = %tool_call_id,
                    body_ms,
                    ok = true,
                    "LATENCY: tool body done"
                );
                output
            }
            Err(e) => {
                tracing::debug!(
                    tool = %tool_name,
                    call_id = %tool_call_id,
                    body_ms,
                    ok = false,
                    "LATENCY: tool body done"
                );
                format!("Error executing tool '{}': {}", tool_name, e)
            }
        }
    }
}

/// Build a `ToolPermissionPattern` for an approval that is scoped to the exact
/// invocation the user approved — the `command` (bash), `path` (file tools),
/// and `host` (network) — rather than the bare tool name.
///
/// Without this, approving a single `bash` call would whitelist every `bash`
/// command for the rest of the session (including `rm -rf ~`).
fn scoped_pattern(
    tool_name: &str,
    command: Option<&str>,
    path: Option<&str>,
    host: Option<&str>,
) -> ToolPermissionPattern {
    let mut pattern = ToolPermissionPattern::simple(tool_name);
    if let Some(cmd) = command {
        pattern = pattern.with_commands(vec![cmd.to_string()]);
    }
    if let Some(p) = path {
        pattern = pattern.with_paths(vec![p.to_string()]);
    }
    if let Some(h) = host {
        pattern = pattern.with_hosts(vec![h.to_string()]);
    }
    pattern
}
