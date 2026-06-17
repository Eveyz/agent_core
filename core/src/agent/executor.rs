use crate::hooks::{HookRegistry, PreToolResult};
use crate::permission::{
    ApprovalChoice, ApprovalScope, PermissionDecision, PermissionPolicy, ToolPermissionPattern,
    WhitelistEntry,
};
use crate::tools::{ToolRegistry, ToolUpdateFn};
use crate::types::{AgentEvent, ToolCall, ToolExecutionMode};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub struct ToolOrchestrator<'a> {
    pub registry: &'a ToolRegistry,
    pub permission_policy: &'a mut PermissionPolicy,
    pub hook_registry: &'a mut HookRegistry,
    pub tool_execution_mode: ToolExecutionMode,
    pub cancel_token: CancellationToken,
}

impl<'a> ToolOrchestrator<'a> {
    #[tracing::instrument(skip_all, fields(tool_count = calls.len()))]
    pub async fn execute_tools<F>(
        &mut self,
        calls: &[ToolCall],
        on_event: &F,
    ) -> Vec<String> 
    where
        F: Fn(AgentEvent) + Send + Sync,
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
                .and_then(|v| v.as_str());
            let host = args.get("url").or(args.get("host")).and_then(|v| v.as_str());

            let decision = self.permission_policy.check(
                &call.function.name,
                &call.function.arguments,
                command,
                path,
                host,
            );

            match decision {
                PermissionDecision::Deny(reason) => {
                    results[i] = format!("Permission denied: {}", reason);
                    continue;
                }
                PermissionDecision::Ask(_reason, prompt) => {
                    // Create oneshot channel for approval
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    {
                        let pending_arc = crate::permission::global_pending_approvals();
                        let mut pending = pending_arc.lock().unwrap();
                        pending.insert(prompt.prompt_id.clone(), tx);
                    }

                    // Emit approval event
                    on_event(AgentEvent::ApprovalRequired {
                        prompt_id: prompt.prompt_id.clone(),
                        tool_name: prompt.tool_name.clone(),
                        tool_input: prompt.tool_input.clone(),
                        danger_level: format!("{:?}", prompt.danger_level),
                        explanation: prompt.explanation.clone(),
                    });

                    // Wait for user response
                    match rx.await {
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
                            {
                                let pending_arc = crate::permission::global_pending_approvals();
                                let mut pending = pending_arc.lock().unwrap();
                                pending.remove(&prompt.prompt_id);
                            }
                        }
                        Err(_) => {
                            // Channel closed = cancelled/timed out → deny
                            results[i] = format!(
                                "Approval cancelled for tool '{}'",
                                call.function.name
                            );
                            {
                                let pending_arc = crate::permission::global_pending_approvals();
                                let mut pending = pending_arc.lock().unwrap();
                                pending.remove(&prompt.prompt_id);
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
                .fire_pre_tool_use(&call.function.name, &args)
            {
                PreToolResult::Veto(reason) => {
                    results[i] = format!("Hook vetoed: {}", reason);
                    continue;
                }
                PreToolResult::Proceed(modified_args) => {
                    on_event(AgentEvent::ToolExecutionStart {
                        tool_call_id: call.id.clone(),
                        tool_name: call.function.name.clone(),
                        args: modified_args.clone(),
                    });
                    allowed.push((i, call.clone(), modified_args));
                }
            }
        }

        if allowed.is_empty() {
            return results;
        }

        match mode {
            ToolExecutionMode::Sequential => {
                for (i, call, args) in &allowed {
                    let cancel = self.cancel_token.clone();
                    let result = tokio::select! {
                        res = self.execute_single_tool(
                            &call.function.name,
                            &call.id,
                            args.clone(),
                            cancel,
                            on_event,
                        ) => res,
                        _ = self.cancel_token.cancelled() => "Aborted".to_string(),
                    };
                    results[*i] = result;
                }
            }
            ToolExecutionMode::Parallel => {
                // TODO: wrap ToolRegistry in Arc to enable true parallel execution
                // via JoinSet. Currently sequential due to &self borrow constraints.
                for (i, call, args) in &allowed {
                    if self.cancel_token.is_cancelled() {
                        results[*i] = "Aborted".to_string();
                        continue;
                    }
                    let cancel = self.cancel_token.clone();
                    let result = tokio::select! {
                        res = self.execute_single_tool(
                            &call.function.name,
                            &call.id,
                            args.clone(),
                            cancel,
                            on_event,
                        ) => res,
                        _ = self.cancel_token.cancelled() => "Aborted".to_string(),
                    };
                    results[*i] = result;
                }
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
                        .fire_post_tool_use(&call.function.name, args, &output);
                results[*i] = final_output;
            }
        }

        results
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
        F: Fn(AgentEvent) + Send + Sync,
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

        // Collect streaming updates from the tool into a shared buffer.
        let updates: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let updates_clone = updates.clone();
        let cancel_clone = cancel_token.clone();
        let on_update: ToolUpdateFn = Arc::new(move |partial: &str| {
            if !cancel_clone.is_cancelled()
                && let Ok(mut buf) = updates_clone.lock()
            {
                buf.push(partial.to_string());
            }
        });

        // Create event channel for tools that emit structured events (e.g. subagent).
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();

        // Forward tool-internal events to the main event stream in real
        // time so the TUI can render them as they happen.
        let tool_fut = tool.execute_with_stream(args, Some(on_update), Some(event_tx));
        let drain_fut = async {
            while let Some(event) = event_rx.recv().await {
                on_event(event);
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
            on_event(event);
        }

        // Flush buffered streaming updates as ToolExecutionUpdate events
        if let Ok(buf) = updates.lock() {
            for partial in buf.iter() {
                on_event(AgentEvent::ToolExecutionUpdate {
                    tool_call_id: tool_call_id.to_string(),
                    tool_name: tool_name.to_string(),
                    partial_result: partial.clone(),
                });
            }
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
