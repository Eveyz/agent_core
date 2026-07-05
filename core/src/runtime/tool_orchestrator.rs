#![allow(deprecated)]
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
    pub session_id: Option<String>,
}

impl<'a> ToolOrchestrator<'a> {
    #[tracing::instrument(skip_all, fields(tool_count = calls.len()))]
    pub async fn execute_tools<F>(&mut self, calls: &[ToolCall], on_event: &F) -> Vec<String>
    where
        F: Fn(AgentEvent, &str) + Send + Sync,
    {
        // Resolve execution mode (per-tool override wins)
        let mode = self
            .registry
            .resolve_execution_mode(calls, self.tool_execution_mode);

        // Always preflight sequentially (permission + hooks)
        let mut allowed: Vec<(usize, ToolCall, Value)> = Vec::new();
        let mut results = vec![String::new(); calls.len()];

        for (i, call) in calls.iter().enumerate() {
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

            let decision = self.permission_policy.check(
                &call.function.name,
                &call.function.arguments,
                command,
                path,
                host,
            );

            tracing::debug!(
                tool = %call.function.name,
                decision = ?decision,
                has_resolver = self.approval_resolver.is_some(),
                "tool permission check"
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
                    tracing::debug!(
                        pid = %prompt.prompt_id,
                        tool = %prompt.tool_name,
                        has_resolver = using_resolver,
                        "approval required"
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

                    tracing::debug!(
                        tool = %call.function.name,
                        resolved = outcome.is_ok(),
                        "approval outcome"
                    );

                    match outcome {
                        Ok(choice) => {
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
            match self
                .hook_registry
                .lock()
                .fire_pre_tool_use(&call.function.name, &args)
            {
                PreToolResult::Veto(reason) => {
                    results[i] = format!("Hook vetoed: {}", reason);
                    continue;
                }
                PreToolResult::Proceed(modified_args) => {
                    tracing::debug!(
                        tool = %call.function.name,
                        call_id = %call.id,
                        "tool execution start"
                    );
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
            let is_error = output.starts_with("Error")
                || output.starts_with("Permission denied")
                || output.starts_with("Hook vetoed");

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

        match result {
            Ok(output) => output,
            Err(e) => format!("Error executing tool '{}': {}", tool_name, e),
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
