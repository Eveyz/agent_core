use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use futures::{stream::FuturesUnordered, StreamExt};
use parking_lot::Mutex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::{
    activity::{ActivityInvocation, ActivityOutcome, ActivityRegistry, RecoveryDisposition},
    model::{
        CommandReceipt, EffectPolicy, NodeKind, NodeSpec, NodeStatus, ObserveRun, ResourceClaim,
        RunId, RunManifest, RunObservation, RunStatus, StartReceipt, StartRun, StoredRun,
        WorkflowCommand, WorkflowEventKind, WorkflowSource,
    },
    reducer::{ready_nodes, replay, resolve_value, validate_spec},
    store::{is_history_conflict, CreateStoredRun, WorkflowStore},
};

#[async_trait]
pub trait WorkflowRuntime: Send + Sync {
    async fn start(&self, request: StartRun) -> Result<StartReceipt>;
    async fn command(&self, run_id: &RunId, command: WorkflowCommand) -> Result<CommandReceipt>;
    async fn observe(&self, query: ObserveRun) -> Result<RunObservation>;
    async fn recover(&self) -> Result<Vec<RunId>>;
}

struct RuntimeInner<S: WorkflowStore> {
    store: Arc<S>,
    activities: ActivityRegistry,
    active: Mutex<HashMap<RunId, CancellationToken>>,
    resources: Mutex<Vec<HeldResources>>,
    resource_changed: tokio::sync::Notify,
}

struct HeldResources {
    run_id: RunId,
    node: super::model::NodeKey,
    claims: Vec<ResourceClaim>,
}

pub struct DurableWorkflowRuntime<S: WorkflowStore> {
    inner: Arc<RuntimeInner<S>>,
}

struct PreparedNode {
    node: NodeSpec,
    attempt: u32,
    execution: PreparedExecution,
    cancel_token: CancellationToken,
}

enum PreparedExecution {
    Activity {
        kind: String,
        invocation: ActivityInvocation,
    },
    Timer {
        fire_at: String,
    },
    Child {
        request: StartRun,
    },
    Immediate(ActivityOutcome),
}

impl<S: WorkflowStore> Clone for DurableWorkflowRuntime<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<S: WorkflowStore> DurableWorkflowRuntime<S> {
    pub fn new(store: Arc<S>, activities: ActivityRegistry) -> Self {
        Self {
            inner: Arc::new(RuntimeInner {
                store,
                activities,
                active: Mutex::new(HashMap::new()),
                resources: Mutex::new(Vec::new()),
                resource_changed: tokio::sync::Notify::new(),
            }),
        }
    }

    fn spawn_drive(&self, run_id: RunId) {
        let cancel_token = {
            let mut active = self.inner.active.lock();
            if active.contains_key(&run_id) {
                return;
            }
            let token = CancellationToken::new();
            active.insert(run_id.clone(), token.clone());
            token
        };
        let runtime = self.clone();
        tokio::spawn(async move {
            if let Err(error) = runtime.drive(&run_id, cancel_token).await {
                let _ = runtime.fail_run(&run_id, error.to_string());
            }
            runtime.release_run_resources(&run_id);
            runtime.inner.active.lock().remove(&run_id);
        });
    }

    async fn drive(&self, run_id: &RunId, cancel_token: CancellationToken) -> Result<()> {
        let mut active = FuturesUnordered::new();
        loop {
            let run = self.inner.store.load(run_id)?;
            let history = self.inner.store.history(run_id, None)?;
            let snapshot = replay(&run, &history)?;

            if snapshot.status.is_terminal()
                || matches!(snapshot.status, RunStatus::Paused | RunStatus::Waiting)
            {
                return Ok(());
            }

            if snapshot.status == RunStatus::Pending {
                match self.inner.store.append(
                    run_id,
                    snapshot.last_sequence,
                    vec![WorkflowEventKind::RunStarted],
                ) {
                    Ok(_) => {}
                    Err(error) if is_history_conflict(&error) => continue,
                    Err(error) => return Err(error),
                }
                continue;
            }

            let ready = ready_nodes(&run.manifest.program, &snapshot);
            let max_concurrency = run.manifest.program.policy.max_concurrency.max(1);
            let mut last_sequence = snapshot.last_sequence;
            let mut scheduled_any = false;
            let mut history_conflicted = false;
            let mut next_retry_delay = None;
            let resource_notification = self.inner.resource_changed.notified();
            for node_key in &ready {
                if active.len() >= max_concurrency {
                    break;
                }
                let node = run
                    .manifest
                    .program
                    .nodes
                    .iter()
                    .find(|candidate| &candidate.key == node_key)
                    .cloned()
                    .ok_or_else(|| {
                        anyhow::anyhow!("ready workflow node is missing: {}", node_key.0)
                    })?;
                if let Some(delay) = retry_delay(&history, &node.key) {
                    next_retry_delay = Some(
                        next_retry_delay
                            .map_or(delay, |current: std::time::Duration| current.min(delay)),
                    );
                    continue;
                }
                if !self.try_acquire_resources(&run.run_id, &node) {
                    continue;
                }
                let (prepared, sequence) = match self.prepare_node(
                    &run,
                    &snapshot,
                    &node,
                    last_sequence,
                    cancel_token.clone(),
                ) {
                    Ok(prepared) => prepared,
                    Err(error) if is_history_conflict(&error) => {
                        self.release_node_resources(&run.run_id, &node.key);
                        history_conflicted = true;
                        break;
                    }
                    Err(error) => {
                        self.release_node_resources(&run.run_id, &node.key);
                        return Err(error);
                    }
                };
                last_sequence = sequence;
                active.push(self.invoke_prepared(prepared));
                scheduled_any = true;
            }

            if history_conflicted {
                continue;
            }

            if let Some((node, attempt, outcome)) = active.next().await {
                let latest = replay(&run, &self.inner.store.history(&run.run_id, None)?)?;
                if latest.status.is_terminal() {
                    self.release_node_resources(&run.run_id, &node.key);
                    return Ok(());
                }
                let persisted =
                    outcome.and_then(|outcome| self.persist_outcome(&run, &node, attempt, outcome));
                self.release_node_resources(&run.run_id, &node.key);
                persisted?;
                continue;
            }

            if !ready.is_empty() && !scheduled_any {
                if let Some(delay) = next_retry_delay {
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = resource_notification => {}
                        _ = cancel_token.cancelled() => return Ok(()),
                    }
                } else {
                    tokio::select! {
                        _ = resource_notification => {}
                        _ = cancel_token.cancelled() => return Ok(()),
                    }
                }
                continue;
            }

            let history = self.inner.store.history(run_id, None)?;
            let snapshot = replay(&run, &history)?;
            if snapshot.nodes.values().all(|node| {
                node.status == super::model::NodeStatus::Succeeded
                    || (matches!(
                        run.manifest.program.policy.on_failure,
                        super::model::FailurePolicy::Continue
                    ) && node.status == super::model::NodeStatus::Failed)
            }) {
                let output = resolve_value(&run.manifest.program.result, &run, &snapshot)
                    .context("resolve workflow result")?;
                match self.inner.store.append(
                    run_id,
                    snapshot.last_sequence,
                    vec![WorkflowEventKind::RunCompleted { output }],
                ) {
                    Ok(_) => {}
                    Err(error) if is_history_conflict(&error) => continue,
                    Err(error) => return Err(error),
                }
                return Ok(());
            }

            if matches!(
                run.manifest.program.policy.on_failure,
                super::model::FailurePolicy::Abort
            ) {
                if let Some(failed) = snapshot.nodes.values().find(|node| {
                    matches!(
                        node.status,
                        super::model::NodeStatus::Failed | super::model::NodeStatus::NeedsAttention
                    )
                }) {
                    match self.inner.store.append(
                        run_id,
                        snapshot.last_sequence,
                        vec![WorkflowEventKind::RunFailed {
                            error: failed.error.clone(),
                        }],
                    ) {
                        Ok(_) => {}
                        Err(error) if is_history_conflict(&error) => continue,
                        Err(error) => return Err(error),
                    }
                }
            }
            return Ok(());
        }
    }

    fn try_acquire_resources(&self, run_id: &RunId, node: &NodeSpec) -> bool {
        if node.resources.is_empty() {
            return true;
        }
        let mut held = self.inner.resources.lock();
        let conflicts = node.resources.iter().any(|claim| {
            held.iter().any(|holder| {
                holder.claims.iter().any(|existing| {
                    existing.resource == claim.resource && (existing.exclusive || claim.exclusive)
                })
            })
        });
        if conflicts {
            return false;
        }
        held.push(HeldResources {
            run_id: run_id.clone(),
            node: node.key.clone(),
            claims: node.resources.clone(),
        });
        true
    }

    fn release_node_resources(&self, run_id: &RunId, node: &super::model::NodeKey) {
        self.inner
            .resources
            .lock()
            .retain(|holder| &holder.run_id != run_id || &holder.node != node);
        self.inner.resource_changed.notify_waiters();
    }

    fn release_run_resources(&self, run_id: &RunId) {
        self.inner
            .resources
            .lock()
            .retain(|holder| &holder.run_id != run_id);
        self.inner.resource_changed.notify_waiters();
    }

    async fn reconcile_interrupted(&self, run_id: &RunId) -> Result<()> {
        let run = self.inner.store.load(run_id)?;
        let history = self.inner.store.history(run_id, None)?;
        let snapshot = replay(&run, &history)?;
        let interrupted: Vec<_> = run
            .manifest
            .program
            .nodes
            .iter()
            .filter(|node| {
                snapshot.nodes.get(&node.key).is_some_and(|state| {
                    matches!(state.status, NodeStatus::Scheduled | NodeStatus::Running)
                })
            })
            .cloned()
            .collect();
        let mut last_sequence = snapshot.last_sequence;

        for node in interrupted {
            let state = snapshot
                .nodes
                .get(&node.key)
                .ok_or_else(|| anyhow::anyhow!("interrupted node is missing: {}", node.key.0))?;
            let scheduled = history.iter().rev().find_map(|event| match &event.kind {
                WorkflowEventKind::NodeScheduled {
                    node: scheduled_node,
                    node_instance_id,
                    attempt_id,
                    attempt,
                    effect_key,
                } if scheduled_node == &node.key && attempt == &state.attempt => Some((
                    node_instance_id.clone(),
                    attempt_id.clone(),
                    effect_key.clone(),
                )),
                _ => None,
            });
            let (node_instance_id, attempt_id, effect_key) = scheduled.ok_or_else(|| {
                anyhow::anyhow!("interrupted node '{}' has no scheduled attempt", node.key.0)
            })?;
            let input = self.resolve_inputs(&node.inputs, &run, &snapshot)?;
            let disposition = match &node.kind {
                NodeKind::Activity { kind, config } => {
                    let adapter = self.inner.activities.get(kind)?;
                    adapter
                        .recover(ActivityInvocation {
                            run_id: run.run_id.clone(),
                            node: node.key.clone(),
                            node_instance_id,
                            attempt_id,
                            attempt: state.attempt,
                            effect_key,
                            effect: node.effect,
                            config: config.clone(),
                            input,
                            scope: run.scope.clone(),
                            cancel_token: CancellationToken::new(),
                        })
                        .await?
                }
                _ => RecoveryDisposition::Retry,
            };
            let events = match disposition {
                RecoveryDisposition::Retry => {
                    let can_retry = state.attempt < node.retry.max_attempts.max(1)
                        && matches!(node.effect, EffectPolicy::Pure | EffectPolicy::ReadOnly);
                    let mut events = vec![WorkflowEventKind::NodeFailed {
                        node: node.key.clone(),
                        error: "activity attempt was interrupted".to_string(),
                        retryable: can_retry,
                    }];
                    if can_retry {
                        events.push(WorkflowEventKind::RetryScheduled {
                            node: node.key.clone(),
                            next_attempt: state.attempt + 1,
                            retry_at: retry_at(node.retry.backoff_ms),
                        });
                    }
                    events
                }
                RecoveryDisposition::Wait => vec![WorkflowEventKind::NodeWaiting {
                    node: node.key.clone(),
                    signal: "activity_recovery".to_string(),
                }],
                RecoveryDisposition::NeedsAttention { reason } => {
                    vec![WorkflowEventKind::NodeNeedsAttention {
                        node: node.key.clone(),
                        reason,
                    }]
                }
            };
            let appended = self.inner.store.append(run_id, last_sequence, events)?;
            last_sequence = appended
                .last()
                .map(|event| event.sequence)
                .unwrap_or(last_sequence);
        }
        Ok(())
    }

    fn prepare_node(
        &self,
        run: &StoredRun,
        snapshot: &super::model::RunSnapshot,
        node: &NodeSpec,
        expected_last_sequence: u64,
        cancel_token: CancellationToken,
    ) -> Result<(PreparedNode, u64)> {
        let attempt = snapshot
            .nodes
            .get(&node.key)
            .map(|state| state.attempt + 1)
            .unwrap_or(1);
        let node_instance_id = format!("{}:{}", run.run_id.0, node.key.0);
        let attempt_id = format!("{node_instance_id}:{attempt}");
        let effect_key = format!("{}:{}", run.request_id, node.key.0);
        let mut scheduled_events = vec![
            WorkflowEventKind::NodeScheduled {
                node: node.key.clone(),
                node_instance_id: node_instance_id.clone(),
                attempt_id: attempt_id.clone(),
                attempt,
                effect_key: effect_key.clone(),
            },
            WorkflowEventKind::AttemptStarted {
                node: node.key.clone(),
                attempt_id: attempt_id.clone(),
            },
        ];
        let timer_fire_at = if let NodeKind::Timer { delay_ms } = &node.kind {
            let prior = self
                .inner
                .store
                .history(&run.run_id, None)?
                .into_iter()
                .rev()
                .find_map(|event| match event.kind {
                    WorkflowEventKind::TimerScheduled {
                        node: scheduled_node,
                        fire_at,
                    } if scheduled_node == node.key => Some(fire_at),
                    _ => None,
                });
            let fire_at = prior.unwrap_or_else(|| {
                (Utc::now() + chrono::Duration::milliseconds(*delay_ms as i64)).to_rfc3339()
            });
            scheduled_events.push(WorkflowEventKind::TimerScheduled {
                node: node.key.clone(),
                fire_at: fire_at.clone(),
            });
            Some(fire_at)
        } else {
            None
        };
        let appended =
            self.inner
                .store
                .append(&run.run_id, expected_last_sequence, scheduled_events)?;

        let input = self.resolve_inputs(&node.inputs, run, snapshot)?;
        let execution = match &node.kind {
            NodeKind::Activity { kind, config } => PreparedExecution::Activity {
                kind: kind.clone(),
                invocation: ActivityInvocation {
                    run_id: run.run_id.clone(),
                    node: node.key.clone(),
                    node_instance_id,
                    attempt_id,
                    attempt,
                    effect_key,
                    effect: node.effect,
                    config: config.clone(),
                    input,
                    scope: run.scope.clone(),
                    cancel_token: cancel_token.child_token(),
                },
            },
            NodeKind::Output => PreparedExecution::Immediate(ActivityOutcome::Completed {
                output: match input {
                    Value::Object(mut fields) if fields.len() == 1 => {
                        fields.remove("value").unwrap_or(Value::Object(fields))
                    }
                    other => other,
                },
                artifacts: Vec::new(),
            }),
            NodeKind::Choice { config } => {
                let output = apply_transform(config, input);
                PreparedExecution::Immediate(ActivityOutcome::Completed {
                    output,
                    artifacts: Vec::new(),
                })
            }
            NodeKind::WaitSignal { name } => {
                PreparedExecution::Immediate(ActivityOutcome::Suspended {
                    signal: name.clone(),
                })
            }
            NodeKind::Timer { .. } => PreparedExecution::Timer {
                fire_at: timer_fire_at
                    .ok_or_else(|| anyhow::anyhow!("timer fire time was not scheduled"))?,
            },
            NodeKind::ChildWorkflow { revision_id } => {
                let mut child_scope = run.scope.clone();
                child_scope.parent_run_id = run.run_id.0.clone();
                child_scope.continuation_key = node.key.0.clone();
                child_scope.trigger = "child_workflow".to_string();
                PreparedExecution::Child {
                    request: StartRun {
                        request_id: format!("child:{}:{}", run.request_id, node.key.0),
                        source: WorkflowSource::Published(revision_id.clone()),
                        input,
                        scope: child_scope,
                    },
                }
            }
            NodeKind::ForEach { .. } => {
                PreparedExecution::Immediate(ActivityOutcome::Failed {
                    error: format!(
                        "workflow node kind is not enabled in V1: {}",
                        node_kind_name(&node.kind)
                    ),
                    retryable: false,
                })
            }
        };
        Ok((
            PreparedNode {
                node: node.clone(),
                attempt,
                execution,
                cancel_token,
            },
            appended
                .last()
                .map(|event| event.sequence)
                .unwrap_or(expected_last_sequence),
        ))
    }

    async fn invoke_prepared(
        &self,
        prepared: PreparedNode,
    ) -> (NodeSpec, u32, Result<ActivityOutcome>) {
        let cancellation = prepared.cancel_token.clone();
        let child_handles_cancellation = matches!(&prepared.execution, PreparedExecution::Child { .. });
        let execution = async {
            match prepared.execution {
                PreparedExecution::Activity { kind, invocation } => {
                    let adapter = match self.inner.activities.get(&kind) {
                        Ok(adapter) => adapter,
                        Err(error) => return Err(error),
                    };
                    let result = if let Some(timeout_ms) = prepared.node.timeout_ms {
                        let activity_cancel = invocation.cancel_token.clone();
                        match tokio::time::timeout(
                            std::time::Duration::from_millis(timeout_ms),
                            adapter.invoke(invocation),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(_) => {
                                activity_cancel.cancel();
                                Ok(ActivityOutcome::Failed {
                                    error: format!(
                                        "workflow activity '{}' timed out after {timeout_ms}ms",
                                        prepared.node.key.0
                                    ),
                                    retryable: true,
                                })
                            }
                        }
                    } else {
                        adapter.invoke(invocation).await
                    };
                    match result {
                        Ok(outcome) => Ok(outcome),
                        Err(error) => Ok(ActivityOutcome::Failed {
                            error: error.to_string(),
                            retryable: true,
                        }),
                    }
                }
                PreparedExecution::Immediate(outcome) => Ok(outcome),
                PreparedExecution::Timer { fire_at } => {
                    let deadline = chrono::DateTime::parse_from_rfc3339(&fire_at)
                        .map(|value| value.with_timezone(&Utc))
                        .context("invalid persisted timer deadline");
                    match deadline {
                        Ok(deadline) => {
                            if let Ok(remaining) = (deadline - Utc::now()).to_std() {
                                tokio::time::sleep(remaining).await;
                            }
                            Ok(ActivityOutcome::Completed {
                                output: serde_json::json!({ "fired_at": Utc::now().to_rfc3339() }),
                                artifacts: Vec::new(),
                            })
                        }
                        Err(error) => Err(error),
                    }
                }
                PreparedExecution::Child { request } => {
                    let receipt = self.start(request).await?;
                    loop {
                        let observation = self
                            .observe(ObserveRun {
                                run_id: receipt.run_id.clone(),
                                after_sequence: None,
                            })
                            .await?;
                        if observation.snapshot.status.is_terminal() {
                            return Ok(match observation.snapshot.status {
                                RunStatus::Succeeded => ActivityOutcome::Completed {
                                    output: observation.snapshot.output,
                                    artifacts: Vec::new(),
                                },
                                RunStatus::NeedsAttention => ActivityOutcome::OutcomeUnknown {
                                    reason: observation.snapshot.error,
                                },
                                RunStatus::Failed | RunStatus::Cancelled => {
                                    ActivityOutcome::Failed {
                                        error: observation.snapshot.error,
                                        retryable: false,
                                    }
                                }
                                _ => unreachable!("terminal status handled above"),
                            });
                        }
                        tokio::select! {
                            _ = cancellation.cancelled() => {
                                let _ = self.command(
                                    &receipt.run_id,
                                    WorkflowCommand::Cancel {
                                        command_id: format!(
                                            "parent-cancel:{}:{}",
                                            prepared.node.key.0,
                                            receipt.run_id.0
                                        ),
                                        reason: "parent workflow was cancelled".to_string(),
                                    },
                                ).await;
                                return Err(anyhow::anyhow!("parent workflow was cancelled"));
                            }
                            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                        }
                    }
                }
            }
        };
        let outcome = if child_handles_cancellation {
            execution.await
        } else {
            tokio::select! {
                _ = cancellation.cancelled() => Err(anyhow::anyhow!("workflow run cancelled")),
                outcome = execution => outcome,
            }
        };
        (prepared.node, prepared.attempt, outcome)
    }

    fn persist_outcome(
        &self,
        run: &StoredRun,
        node: &NodeSpec,
        attempt: u32,
        outcome: ActivityOutcome,
    ) -> Result<()> {
        let retry_deadline = retry_at(node.retry.backoff_ms);
        for _ in 0..16 {
            let history = self.inner.store.history(&run.run_id, None)?;
            let snapshot = replay(run, &history)?;
            if snapshot.status.is_terminal()
                || snapshot.nodes.get(&node.key).is_some_and(|state| {
                    matches!(
                        state.status,
                        NodeStatus::Succeeded | NodeStatus::NeedsAttention
                    )
                })
            {
                return Ok(());
            }
            let latest = snapshot.last_sequence;
            let events = match &outcome {
                ActivityOutcome::Completed { output, artifacts } => {
                    let mut events = Vec::new();
                    if matches!(node.kind, NodeKind::Timer { .. }) {
                        events.push(WorkflowEventKind::TimerFired {
                            node: node.key.clone(),
                            fired_at: Utc::now().to_rfc3339(),
                        });
                    }
                    events.push(WorkflowEventKind::NodeCompleted {
                        node: node.key.clone(),
                        output: output.clone(),
                        artifacts: artifacts.clone(),
                    });
                    events
                }
                ActivityOutcome::Failed { error, retryable } => {
                    let can_retry = *retryable
                        && attempt < node.retry.max_attempts.max(1)
                        && matches!(node.effect, EffectPolicy::Pure | EffectPolicy::ReadOnly);
                    let mut events = vec![WorkflowEventKind::NodeFailed {
                        node: node.key.clone(),
                        error: error.clone(),
                        retryable: can_retry,
                    }];
                    if can_retry {
                        events.push(WorkflowEventKind::RetryScheduled {
                            node: node.key.clone(),
                            next_attempt: attempt + 1,
                            retry_at: retry_deadline.clone(),
                        });
                    }
                    events
                }
                ActivityOutcome::Suspended { signal } => {
                    if let Some((command_id, name, payload)) =
                        pending_signal(&history, &node.key, signal)
                    {
                        vec![
                            WorkflowEventKind::SignalConsumed {
                                node: node.key.clone(),
                                command_id: command_id.clone(),
                                name: name.clone(),
                            },
                            WorkflowEventKind::NodeCompleted {
                                node: node.key.clone(),
                                output: serde_json::json!({
                                    "signal": name,
                                    "payload": payload,
                                    "command_id": command_id,
                                }),
                                artifacts: Vec::new(),
                            },
                        ]
                    } else {
                        vec![WorkflowEventKind::NodeWaiting {
                            node: node.key.clone(),
                            signal: signal.clone(),
                        }]
                    }
                }
                ActivityOutcome::OutcomeUnknown { reason } => {
                    vec![WorkflowEventKind::NodeNeedsAttention {
                        node: node.key.clone(),
                        reason: reason.clone(),
                    }]
                }
            };
            match self.inner.store.append(&run.run_id, latest, events) {
                Ok(_) => return Ok(()),
                Err(error) if is_history_conflict(&error) => continue,
                Err(error) => return Err(error),
            }
        }
        anyhow::bail!(
            "workflow outcome for node '{}' could not be persisted after repeated history conflicts",
            node.key.0
        )
    }

    fn resolve_inputs(
        &self,
        inputs: &BTreeMap<String, super::model::ValueExpr>,
        run: &StoredRun,
        snapshot: &super::model::RunSnapshot,
    ) -> Result<Value> {
        let fields: Result<Map<String, Value>> = inputs
            .iter()
            .map(|(key, expr)| Ok((key.clone(), resolve_value(expr, run, snapshot)?)))
            .collect();
        Ok(Value::Object(fields?))
    }

    fn fail_run(&self, run_id: &RunId, error: String) -> Result<()> {
        let run = self.inner.store.load(run_id)?;
        let history = self.inner.store.history(run_id, None)?;
        let snapshot = replay(&run, &history)?;
        if snapshot.status.is_terminal() {
            return Ok(());
        }
        self.inner.store.append(
            run_id,
            snapshot.last_sequence,
            vec![WorkflowEventKind::RunFailed { error }],
        )?;
        Ok(())
    }
}

fn retry_at(backoff_ms: u64) -> String {
    (Utc::now() + chrono::Duration::milliseconds(backoff_ms as i64)).to_rfc3339()
}

fn retry_delay(
    history: &[super::model::WorkflowEvent],
    node: &super::model::NodeKey,
) -> Option<std::time::Duration> {
    let retry_at = history.iter().rev().find_map(|event| match &event.kind {
        WorkflowEventKind::RetryScheduled {
            node: retry_node,
            retry_at,
            ..
        } if retry_node == node => Some(retry_at.as_str()),
        WorkflowEventKind::NodeScheduled {
            node: scheduled_node,
            ..
        } if scheduled_node == node => None,
        _ => None,
    })?;
    if retry_at.is_empty() {
        return None;
    }
    let deadline = chrono::DateTime::parse_from_rfc3339(retry_at)
        .ok()?
        .with_timezone(&Utc);
    (deadline - Utc::now()).to_std().ok()
}

fn pending_signal(
    history: &[super::model::WorkflowEvent],
    node: &super::model::NodeKey,
    name: &str,
) -> Option<(String, String, Value)> {
    let consumed = history
        .iter()
        .filter_map(|event| match &event.kind {
            WorkflowEventKind::SignalConsumed {
                node: consumed_node,
                command_id,
                ..
            } if consumed_node == node => Some(command_id.as_str()),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    history.iter().find_map(|event| match &event.kind {
        WorkflowEventKind::SignalReceived {
            command_id,
            name: received_name,
            payload,
        } if received_name == name && !consumed.contains(command_id.as_str()) => {
            Some((command_id.clone(), received_name.clone(), payload.clone()))
        }
        _ => None,
    })
}

fn node_kind_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Activity { .. } => "activity",
        NodeKind::Output => "output",
        NodeKind::Choice { .. } => "choice",
        NodeKind::WaitSignal { .. } => "wait_signal",
        NodeKind::Timer { .. } => "timer",
        NodeKind::ChildWorkflow { .. } => "child_workflow",
        NodeKind::ForEach { .. } => "for_each",
    }
}

fn apply_transform(config: &Value, input: Value) -> Value {
    let value = match input {
        Value::Object(mut fields) if fields.len() == 1 => {
            fields.remove("value").unwrap_or(Value::Object(fields))
        }
        other => other,
    };
    config
        .get("extract")
        .and_then(Value::as_str)
        .and_then(|field| value.get(field))
        .cloned()
        .unwrap_or(value)
}

#[async_trait]
impl<S: WorkflowStore> WorkflowRuntime for DurableWorkflowRuntime<S> {
    async fn start(&self, request: StartRun) -> Result<StartReceipt> {
        let spec = match request.source {
            WorkflowSource::Inline(spec) => spec,
            WorkflowSource::Published(revision) => self.inner.store.load_revision(&revision)?,
        };
        validate_spec(&spec)?;
        let encoded = serde_json::to_vec(&spec)?;
        let program_hash = hex::encode(Sha256::digest(encoded));
        let run = StoredRun {
            run_id: RunId(uuid::Uuid::new_v4().to_string()),
            request_id: request.request_id,
            manifest: RunManifest {
                program_hash,
                program: spec,
                adapter_versions: self.inner.activities.versions(),
            },
            input: request.input,
            scope: request.scope,
        };
        let created = self.inner.store.create_or_get(CreateStoredRun { run })?;
        self.spawn_drive(created.run_id.clone());
        Ok(StartReceipt {
            run_id: created.run_id,
            created: created.created,
        })
    }

    async fn command(&self, run_id: &RunId, command: WorkflowCommand) -> Result<CommandReceipt> {
        let run = self.inner.store.load(run_id)?;
        let command_id = command.command_id().to_string();
        for _ in 0..16 {
            let history = self.inner.store.history(run_id, None)?;
            let snapshot = replay(&run, &history)?;
            if snapshot.status.is_terminal() {
                return Ok(CommandReceipt {
                    accepted: false,
                    sequence: snapshot.last_sequence,
                });
            }
            if history.iter().any(|event| match &event.kind {
                WorkflowEventKind::SignalReceived {
                    command_id: existing,
                    ..
                }
                | WorkflowEventKind::RunPaused {
                    command_id: existing,
                }
                | WorkflowEventKind::RunResumed {
                    command_id: existing,
                }
                | WorkflowEventKind::RunCancelled {
                    command_id: existing,
                    ..
                } => existing == &command_id,
                _ => false,
            }) {
                return Ok(CommandReceipt {
                    accepted: false,
                    sequence: snapshot.last_sequence,
                });
            }

            let events = match &command {
                WorkflowCommand::Signal {
                    command_id,
                    name,
                    payload,
                } => {
                    let mut events = vec![WorkflowEventKind::SignalReceived {
                        command_id: command_id.clone(),
                        name: name.clone(),
                        payload: payload.clone(),
                    }];
                    for node in &run.manifest.program.nodes {
                        if snapshot
                            .nodes
                            .get(&node.key)
                            .is_some_and(|state| state.status == NodeStatus::Waiting)
                            && matches!(
                                &node.kind,
                                NodeKind::WaitSignal { name: awaited } if awaited == name
                            )
                        {
                            events.push(WorkflowEventKind::SignalConsumed {
                                node: node.key.clone(),
                                command_id: command_id.clone(),
                                name: name.clone(),
                            });
                            events.push(WorkflowEventKind::NodeCompleted {
                                node: node.key.clone(),
                                output: serde_json::json!({
                                    "signal": name,
                                    "payload": payload,
                                    "command_id": command_id,
                                }),
                                artifacts: Vec::new(),
                            });
                        }
                    }
                    if events.len() > 1 {
                        events.push(WorkflowEventKind::RunResumed {
                            command_id: command_id.clone(),
                        });
                    }
                    events
                }
                WorkflowCommand::Pause { command_id } => {
                    vec![WorkflowEventKind::RunPaused {
                        command_id: command_id.clone(),
                    }]
                }
                WorkflowCommand::Resume { command_id } => {
                    vec![WorkflowEventKind::RunResumed {
                        command_id: command_id.clone(),
                    }]
                }
                WorkflowCommand::Cancel { command_id, reason } => {
                    vec![WorkflowEventKind::RunCancelled {
                        command_id: command_id.clone(),
                        reason: reason.clone(),
                    }]
                }
            };
            let appended = match self
                .inner
                .store
                .append(run_id, snapshot.last_sequence, events)
            {
                Ok(appended) => appended,
                Err(error) if is_history_conflict(&error) => continue,
                Err(error) => return Err(error),
            };
            if matches!(command, WorkflowCommand::Cancel { .. }) {
                if let Some(token) = self.inner.active.lock().get(run_id) {
                    token.cancel();
                }
            } else {
                self.spawn_drive(run_id.clone());
            }
            return Ok(CommandReceipt {
                accepted: true,
                sequence: appended
                    .last()
                    .map(|event| event.sequence)
                    .unwrap_or(snapshot.last_sequence),
            });
        }
        anyhow::bail!(
            "workflow command '{}' could not be persisted after repeated history conflicts",
            command_id
        )
    }

    async fn observe(&self, query: ObserveRun) -> Result<RunObservation> {
        let run = self.inner.store.load(&query.run_id)?;
        let all_events = self.inner.store.history(&query.run_id, None)?;
        let snapshot = replay(&run, &all_events)?;
        let events = all_events
            .into_iter()
            .filter(|event| {
                query
                    .after_sequence
                    .is_none_or(|after| event.sequence > after)
            })
            .collect();
        Ok(RunObservation { snapshot, events })
    }

    async fn recover(&self) -> Result<Vec<RunId>> {
        let runs = self.inner.store.recoverable_runs()?;
        let mut recovered = Vec::new();
        for run_id in runs {
            self.reconcile_interrupted(&run_id).await?;
            let observation = self
                .observe(ObserveRun {
                    run_id: run_id.clone(),
                    after_sequence: None,
                })
                .await?;
            if !observation.snapshot.status.is_terminal() {
                self.spawn_drive(run_id.clone());
                recovered.push(run_id);
            }
        }
        Ok(recovered)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::Notify;

    use super::*;
    use crate::workflow::runtime::{
        ActivityAdapter, ActivityDescriptor, ActivityInvocation, ActivityOutcome, EffectPolicy,
        InMemoryWorkflowStore, NodeKey, NodeSpec, RetryPolicy, RunScope, ValueExpr, WorkflowPolicy,
        WorkflowRevisionId, WorkflowSpec,
    };

    struct EchoActivity;

    #[async_trait]
    impl ActivityAdapter for EchoActivity {
        fn descriptor(&self) -> ActivityDescriptor {
            ActivityDescriptor {
                kind: "test.echo".to_string(),
                version: "1".to_string(),
            }
        }

        async fn invoke(&self, invocation: ActivityInvocation) -> Result<ActivityOutcome> {
            Ok(ActivityOutcome::Completed {
                output: json!({ "echo": invocation.input }),
                artifacts: Vec::new(),
            })
        }
    }

    struct GateActivity {
        release_blocked: Arc<Notify>,
        dependent_started: Arc<Notify>,
    }

    struct RecoveryActivity {
        invokes: Arc<AtomicUsize>,
    }

    struct ConcurrencyActivity {
        current: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
    }

    struct DelayedActivity {
        invokes: Arc<AtomicUsize>,
        delay: Duration,
        fail_first: bool,
    }

    struct ErrorActivity;

    #[async_trait]
    impl ActivityAdapter for ErrorActivity {
        fn descriptor(&self) -> ActivityDescriptor {
            ActivityDescriptor {
                kind: "test.error".to_string(),
                version: "1".to_string(),
            }
        }

        async fn invoke(&self, _invocation: ActivityInvocation) -> Result<ActivityOutcome> {
            anyhow::bail!("adapter failed")
        }
    }

    #[async_trait]
    impl ActivityAdapter for DelayedActivity {
        fn descriptor(&self) -> ActivityDescriptor {
            ActivityDescriptor {
                kind: "test.delayed".to_string(),
                version: "1".to_string(),
            }
        }

        async fn invoke(&self, _invocation: ActivityInvocation) -> Result<ActivityOutcome> {
            let invocation = self.invokes.fetch_add(1, Ordering::SeqCst) + 1;
            tokio::time::sleep(self.delay).await;
            if self.fail_first && invocation == 1 {
                Ok(ActivityOutcome::Failed {
                    error: "retry me".to_string(),
                    retryable: true,
                })
            } else {
                Ok(ActivityOutcome::Completed {
                    output: json!("done"),
                    artifacts: Vec::new(),
                })
            }
        }
    }

    #[async_trait]
    impl ActivityAdapter for RecoveryActivity {
        fn descriptor(&self) -> ActivityDescriptor {
            ActivityDescriptor {
                kind: "test.recovery".to_string(),
                version: "1".to_string(),
            }
        }

        async fn invoke(&self, _invocation: ActivityInvocation) -> Result<ActivityOutcome> {
            self.invokes.fetch_add(1, Ordering::SeqCst);
            Ok(ActivityOutcome::Completed {
                output: json!("recovered"),
                artifacts: Vec::new(),
            })
        }
    }

    #[async_trait]
    impl ActivityAdapter for ConcurrencyActivity {
        fn descriptor(&self) -> ActivityDescriptor {
            ActivityDescriptor {
                kind: "test.concurrency".to_string(),
                version: "1".to_string(),
            }
        }

        async fn invoke(&self, _invocation: ActivityInvocation) -> Result<ActivityOutcome> {
            let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(15)).await;
            self.current.fetch_sub(1, Ordering::SeqCst);
            Ok(ActivityOutcome::Completed {
                output: json!("done"),
                artifacts: Vec::new(),
            })
        }
    }

    #[async_trait]
    impl ActivityAdapter for GateActivity {
        fn descriptor(&self) -> ActivityDescriptor {
            ActivityDescriptor {
                kind: "test.gate".to_string(),
                version: "1".to_string(),
            }
        }

        async fn invoke(&self, invocation: ActivityInvocation) -> Result<ActivityOutcome> {
            match invocation
                .config
                .get("mode")
                .and_then(|value| value.as_str())
            {
                Some("blocked") => self.release_blocked.notified().await,
                Some("dependent") => self.dependent_started.notify_one(),
                _ => {}
            }
            Ok(ActivityOutcome::Completed {
                output: invocation.input,
                artifacts: Vec::new(),
            })
        }
    }

    fn echo_spec() -> WorkflowSpec {
        let mut inputs = BTreeMap::new();
        inputs.insert(
            "value".to_string(),
            ValueExpr::RunInput {
                pointer: "/value".to_string(),
            },
        );
        WorkflowSpec {
            schema_version: 1,
            nodes: vec![NodeSpec {
                key: NodeKey::from("echo"),
                kind: NodeKind::Activity {
                    kind: "test.echo".to_string(),
                    config: json!({}),
                },
                inputs,
                after: Vec::new(),
                retry: RetryPolicy::default(),
                timeout_ms: None,
                effect: EffectPolicy::ReadOnly,
                resources: Vec::new(),
            }],
            result: ValueExpr::NodeOutput {
                node: NodeKey::from("echo"),
                pointer: "/echo/value".to_string(),
            },
            policy: WorkflowPolicy::default(),
        }
    }

    #[tokio::test]
    async fn repeated_start_returns_same_completed_run() {
        let store = Arc::new(InMemoryWorkflowStore::default());
        let activities =
            ActivityRegistry::new([Arc::new(EchoActivity) as Arc<dyn ActivityAdapter>])
                .expect("activity registry");
        let runtime = DurableWorkflowRuntime::new(store, activities);
        let request = StartRun {
            request_id: "prompt-1/tool-1".to_string(),
            source: WorkflowSource::Inline(echo_spec()),
            input: json!({ "value": 42 }),
            scope: RunScope::default(),
        };

        let first = runtime.start(request.clone()).await.expect("first start");
        let second = runtime.start(request).await.expect("idempotent start");
        assert!(first.created);
        assert!(!second.created);
        assert_eq!(first.run_id, second.run_id);

        let observation = loop {
            let observation = runtime
                .observe(ObserveRun {
                    run_id: first.run_id.clone(),
                    after_sequence: None,
                })
                .await
                .expect("observe");
            if observation.snapshot.status.is_terminal() {
                break observation;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };

        assert_eq!(observation.snapshot.status, RunStatus::Succeeded);
        assert_eq!(observation.snapshot.output, json!(42));
        assert!(observation
            .events
            .iter()
            .any(|event| { matches!(event.kind, WorkflowEventKind::NodeCompleted { .. }) }));
    }

    #[tokio::test]
    async fn child_workflow_runs_pinned_revision_and_is_idempotent() {
        let store = Arc::new(InMemoryWorkflowStore::default());
        let revision_id = WorkflowRevisionId("child:r1".into());
        store
            .publish_revision(&revision_id, &echo_spec())
            .expect("publish child");
        let activities =
            ActivityRegistry::new([Arc::new(EchoActivity) as Arc<dyn ActivityAdapter>])
                .expect("activity registry");
        let runtime = DurableWorkflowRuntime::new(store, activities);
        let parent = WorkflowSpec {
            schema_version: 1,
            nodes: vec![NodeSpec {
                key: NodeKey::from("child"),
                kind: NodeKind::ChildWorkflow {
                    revision_id: revision_id.clone(),
                },
                inputs: BTreeMap::from([(
                    "value".into(),
                    ValueExpr::RunInput {
                        pointer: "/value".into(),
                    },
                )]),
                after: Vec::new(),
                retry: RetryPolicy::default(),
                timeout_ms: None,
                effect: EffectPolicy::ReadOnly,
                resources: Vec::new(),
            }],
            result: ValueExpr::NodeOutput {
                node: NodeKey::from("child"),
                pointer: String::new(),
            },
            policy: WorkflowPolicy::default(),
        };
        let request = StartRun {
            request_id: "parent-with-child".into(),
            source: WorkflowSource::Inline(parent),
            input: json!({ "value": 42 }),
            scope: RunScope::default(),
        };
        let first = runtime.start(request.clone()).await.expect("first parent");
        let second = runtime.start(request).await.expect("same parent");
        assert_eq!(first.run_id, second.run_id);
        assert!(!second.created);
        loop {
            let observed = runtime
                .observe(ObserveRun {
                    run_id: first.run_id.clone(),
                    after_sequence: None,
                })
                .await
                .expect("observe parent");
            if observed.snapshot.status.is_terminal() {
                assert_eq!(observed.snapshot.status, RunStatus::Succeeded);
                assert_eq!(observed.snapshot.output, json!(42));
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn ready_dependent_does_not_wait_for_unrelated_slow_node() {
        let release_blocked = Arc::new(Notify::new());
        let dependent_started = Arc::new(Notify::new());
        let activities = ActivityRegistry::new([Arc::new(GateActivity {
            release_blocked: release_blocked.clone(),
            dependent_started: dependent_started.clone(),
        }) as Arc<dyn ActivityAdapter>])
        .expect("activity registry");
        let runtime =
            DurableWorkflowRuntime::new(Arc::new(InMemoryWorkflowStore::default()), activities);
        let activity = |key: &str, mode: &str, after: Vec<NodeKey>| NodeSpec {
            key: NodeKey::from(key),
            kind: NodeKind::Activity {
                kind: "test.gate".to_string(),
                config: json!({ "mode": mode }),
            },
            inputs: BTreeMap::new(),
            after,
            retry: RetryPolicy::default(),
            timeout_ms: None,
            effect: EffectPolicy::ReadOnly,
            resources: Vec::new(),
        };
        let spec = WorkflowSpec {
            schema_version: 1,
            nodes: vec![
                activity("fast", "fast", Vec::new()),
                activity("blocked", "blocked", Vec::new()),
                activity("dependent", "dependent", vec![NodeKey::from("fast")]),
            ],
            result: ValueExpr::NodeOutput {
                node: NodeKey::from("dependent"),
                pointer: String::new(),
            },
            policy: WorkflowPolicy {
                max_concurrency: 2,
                ..WorkflowPolicy::default()
            },
        };

        runtime
            .start(StartRun {
                request_id: "event-driven".to_string(),
                source: WorkflowSource::Inline(spec),
                input: json!({}),
                scope: RunScope::default(),
            })
            .await
            .expect("start");

        let dependent_progressed =
            tokio::time::timeout(Duration::from_millis(150), dependent_started.notified()).await;
        release_blocked.notify_waiters();

        assert!(
            dependent_progressed.is_ok(),
            "dependent node should start before the unrelated blocked node completes"
        );
    }

    #[tokio::test]
    async fn recovery_retries_interrupted_read_only_attempt() {
        let invokes = Arc::new(AtomicUsize::new(0));
        let store = Arc::new(InMemoryWorkflowStore::default());
        let activities = ActivityRegistry::new([Arc::new(RecoveryActivity {
            invokes: invokes.clone(),
        }) as Arc<dyn ActivityAdapter>])
        .expect("activity registry");
        let runtime = DurableWorkflowRuntime::new(store.clone(), activities);
        let mut spec = echo_spec();
        spec.nodes[0].kind = NodeKind::Activity {
            kind: "test.recovery".to_string(),
            config: json!({}),
        };
        spec.nodes[0].retry.max_attempts = 2;
        spec.result = ValueExpr::NodeOutput {
            node: NodeKey::from("echo"),
            pointer: String::new(),
        };
        let run_id = RunId("interrupted-read".to_string());
        store
            .create_or_get(CreateStoredRun {
                run: StoredRun {
                    run_id: run_id.clone(),
                    request_id: "interrupted-read-request".to_string(),
                    manifest: RunManifest {
                        program_hash: "frozen".to_string(),
                        program: spec,
                        adapter_versions: BTreeMap::new(),
                    },
                    input: json!({ "value": 1 }),
                    scope: RunScope::default(),
                },
            })
            .expect("create interrupted run");
        store
            .append(
                &run_id,
                0,
                vec![
                    WorkflowEventKind::RunStarted,
                    WorkflowEventKind::NodeScheduled {
                        node: NodeKey::from("echo"),
                        node_instance_id: "interrupted-read:echo".to_string(),
                        attempt_id: "interrupted-read:echo:1".to_string(),
                        attempt: 1,
                        effect_key: "interrupted-read-request:echo".to_string(),
                    },
                    WorkflowEventKind::AttemptStarted {
                        node: NodeKey::from("echo"),
                        attempt_id: "interrupted-read:echo:1".to_string(),
                    },
                ],
            )
            .expect("persist interrupted attempt");

        let recovered = runtime.recover().await.expect("recover runtime");
        assert_eq!(recovered, vec![run_id.clone()]);
        let observation = loop {
            let observation = runtime
                .observe(ObserveRun {
                    run_id: run_id.clone(),
                    after_sequence: None,
                })
                .await
                .expect("observe recovery");
            if observation.snapshot.status.is_terminal() {
                break observation;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };

        assert_eq!(observation.snapshot.status, RunStatus::Succeeded);
        assert_eq!(observation.snapshot.output, json!("recovered"));
        assert_eq!(invokes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn recovery_does_not_blindly_retry_interrupted_workspace_write() {
        let invokes = Arc::new(AtomicUsize::new(0));
        let store = Arc::new(InMemoryWorkflowStore::default());
        let activities = ActivityRegistry::new([Arc::new(RecoveryActivity {
            invokes: invokes.clone(),
        }) as Arc<dyn ActivityAdapter>])
        .expect("activity registry");
        let runtime = DurableWorkflowRuntime::new(store.clone(), activities);
        let mut spec = echo_spec();
        spec.nodes[0].kind = NodeKind::Activity {
            kind: "test.recovery".to_string(),
            config: json!({}),
        };
        spec.nodes[0].effect = EffectPolicy::WorkspaceWrite;
        let run_id = RunId("interrupted-write".to_string());
        store
            .create_or_get(CreateStoredRun {
                run: StoredRun {
                    run_id: run_id.clone(),
                    request_id: "interrupted-write-request".to_string(),
                    manifest: RunManifest {
                        program_hash: "frozen".to_string(),
                        program: spec,
                        adapter_versions: BTreeMap::new(),
                    },
                    input: json!({ "value": 1 }),
                    scope: RunScope::default(),
                },
            })
            .expect("create interrupted run");
        store
            .append(
                &run_id,
                0,
                vec![
                    WorkflowEventKind::RunStarted,
                    WorkflowEventKind::NodeScheduled {
                        node: NodeKey::from("echo"),
                        node_instance_id: "interrupted-write:echo".to_string(),
                        attempt_id: "interrupted-write:echo:1".to_string(),
                        attempt: 1,
                        effect_key: "interrupted-write-request:echo".to_string(),
                    },
                    WorkflowEventKind::AttemptStarted {
                        node: NodeKey::from("echo"),
                        attempt_id: "interrupted-write:echo:1".to_string(),
                    },
                ],
            )
            .expect("persist interrupted attempt");

        runtime.recover().await.expect("recover runtime");
        let observation = runtime
            .observe(ObserveRun {
                run_id,
                after_sequence: None,
            })
            .await
            .expect("observe recovery");

        assert_eq!(observation.snapshot.status, RunStatus::NeedsAttention);
        assert_eq!(invokes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn durable_signal_resumes_waiting_node_idempotently() {
        let runtime = DurableWorkflowRuntime::new(
            Arc::new(InMemoryWorkflowStore::default()),
            ActivityRegistry::default(),
        );
        let receipt = runtime
            .start(StartRun {
                request_id: "signal-run".to_string(),
                source: WorkflowSource::Inline(WorkflowSpec {
                    schema_version: 1,
                    nodes: vec![NodeSpec {
                        key: NodeKey::from("approval"),
                        kind: NodeKind::WaitSignal {
                            name: "approve".to_string(),
                        },
                        inputs: BTreeMap::new(),
                        after: Vec::new(),
                        retry: RetryPolicy::default(),
                        timeout_ms: None,
                        effect: EffectPolicy::Pure,
                        resources: Vec::new(),
                    }],
                    result: ValueExpr::NodeOutput {
                        node: NodeKey::from("approval"),
                        pointer: "/payload".to_string(),
                    },
                    policy: WorkflowPolicy::default(),
                }),
                input: json!({}),
                scope: RunScope::default(),
            })
            .await
            .expect("start signal workflow");
        loop {
            let observed = runtime
                .observe(ObserveRun {
                    run_id: receipt.run_id.clone(),
                    after_sequence: None,
                })
                .await
                .expect("observe wait");
            if observed.snapshot.status == RunStatus::Waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let first = runtime
            .command(
                &receipt.run_id,
                WorkflowCommand::Signal {
                    command_id: "approve-once".to_string(),
                    name: "approve".to_string(),
                    payload: json!({ "approved": true }),
                },
            )
            .await
            .expect("signal");
        let duplicate = runtime
            .command(
                &receipt.run_id,
                WorkflowCommand::Signal {
                    command_id: "approve-once".to_string(),
                    name: "approve".to_string(),
                    payload: json!({ "approved": false }),
                },
            )
            .await
            .expect("duplicate signal");
        assert!(first.accepted);
        assert!(!duplicate.accepted);

        let observed = loop {
            let observed = runtime
                .observe(ObserveRun {
                    run_id: receipt.run_id.clone(),
                    after_sequence: None,
                })
                .await
                .expect("observe completion");
            if observed.snapshot.status.is_terminal() {
                break observed;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(observed.snapshot.status, RunStatus::Succeeded);
        assert_eq!(observed.snapshot.output, json!({ "approved": true }));
    }

    #[tokio::test]
    async fn timer_records_schedule_and_fire_events() {
        let runtime = DurableWorkflowRuntime::new(
            Arc::new(InMemoryWorkflowStore::default()),
            ActivityRegistry::default(),
        );
        let receipt = runtime
            .start(StartRun {
                request_id: "timer-run".to_string(),
                source: WorkflowSource::Inline(WorkflowSpec {
                    schema_version: 1,
                    nodes: vec![NodeSpec {
                        key: NodeKey::from("timer"),
                        kind: NodeKind::Timer { delay_ms: 5 },
                        inputs: BTreeMap::new(),
                        after: Vec::new(),
                        retry: RetryPolicy::default(),
                        timeout_ms: None,
                        effect: EffectPolicy::Pure,
                        resources: Vec::new(),
                    }],
                    result: ValueExpr::NodeOutput {
                        node: NodeKey::from("timer"),
                        pointer: String::new(),
                    },
                    policy: WorkflowPolicy::default(),
                }),
                input: json!({}),
                scope: RunScope::default(),
            })
            .await
            .expect("start timer");
        let observed = loop {
            let observed = runtime
                .observe(ObserveRun {
                    run_id: receipt.run_id.clone(),
                    after_sequence: None,
                })
                .await
                .expect("observe timer");
            if observed.snapshot.status.is_terminal() {
                break observed;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(observed.snapshot.status, RunStatus::Succeeded);
        assert!(observed
            .events
            .iter()
            .any(|event| matches!(event.kind, WorkflowEventKind::TimerScheduled { .. })));
        assert!(observed
            .events
            .iter()
            .any(|event| matches!(event.kind, WorkflowEventKind::TimerFired { .. })));
    }

    #[tokio::test]
    async fn exclusive_resource_claims_serialize_independent_nodes() {
        let current = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let runtime = DurableWorkflowRuntime::new(
            Arc::new(InMemoryWorkflowStore::default()),
            ActivityRegistry::new([Arc::new(ConcurrencyActivity {
                current: current.clone(),
                maximum: maximum.clone(),
            }) as Arc<dyn ActivityAdapter>])
            .expect("activities"),
        );
        let node = |key: &str| NodeSpec {
            key: NodeKey::from(key),
            kind: NodeKind::Activity {
                kind: "test.concurrency".to_string(),
                config: json!({}),
            },
            inputs: BTreeMap::new(),
            after: Vec::new(),
            retry: RetryPolicy::default(),
            timeout_ms: None,
            effect: EffectPolicy::WorkspaceWrite,
            resources: vec![super::super::model::ResourceClaim {
                resource: "workspace".to_string(),
                exclusive: true,
            }],
        };
        let receipt = runtime
            .start(StartRun {
                request_id: "exclusive-resources".to_string(),
                source: WorkflowSource::Inline(WorkflowSpec {
                    schema_version: 1,
                    nodes: vec![node("one"), node("two")],
                    result: ValueExpr::NodeOutput {
                        node: NodeKey::from("two"),
                        pointer: String::new(),
                    },
                    policy: WorkflowPolicy {
                        max_concurrency: 2,
                        ..WorkflowPolicy::default()
                    },
                }),
                input: json!({}),
                scope: RunScope::default(),
            })
            .await
            .expect("start");
        loop {
            let observed = runtime
                .observe(ObserveRun {
                    run_id: receipt.run_id.clone(),
                    after_sequence: None,
                })
                .await
                .expect("observe");
            if observed.snapshot.status.is_terminal() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn waiting_node_does_not_cancel_unrelated_running_activity() {
        let invokes = Arc::new(AtomicUsize::new(0));
        let runtime = DurableWorkflowRuntime::new(
            Arc::new(InMemoryWorkflowStore::default()),
            ActivityRegistry::new([Arc::new(DelayedActivity {
                invokes: invokes.clone(),
                delay: Duration::from_millis(25),
                fail_first: false,
            }) as Arc<dyn ActivityAdapter>])
            .expect("activities"),
        );
        let receipt = runtime
            .start(StartRun {
                request_id: "wait-with-active".to_string(),
                source: WorkflowSource::Inline(WorkflowSpec {
                    schema_version: 1,
                    nodes: vec![
                        NodeSpec {
                            key: NodeKey::from("approval"),
                            kind: NodeKind::WaitSignal {
                                name: "approve".to_string(),
                            },
                            inputs: BTreeMap::new(),
                            after: Vec::new(),
                            retry: RetryPolicy::default(),
                            timeout_ms: None,
                            effect: EffectPolicy::Pure,
                            resources: Vec::new(),
                        },
                        NodeSpec {
                            key: NodeKey::from("work"),
                            kind: NodeKind::Activity {
                                kind: "test.delayed".to_string(),
                                config: json!({}),
                            },
                            inputs: BTreeMap::new(),
                            after: Vec::new(),
                            retry: RetryPolicy::default(),
                            timeout_ms: None,
                            effect: EffectPolicy::ReadOnly,
                            resources: Vec::new(),
                        },
                    ],
                    result: ValueExpr::NodeOutput {
                        node: NodeKey::from("work"),
                        pointer: String::new(),
                    },
                    policy: WorkflowPolicy {
                        max_concurrency: 2,
                        ..WorkflowPolicy::default()
                    },
                }),
                input: json!({}),
                scope: RunScope::default(),
            })
            .await
            .expect("start");

        tokio::time::sleep(Duration::from_millis(75)).await;
        let observed = runtime
            .observe(ObserveRun {
                run_id: receipt.run_id,
                after_sequence: None,
            })
            .await
            .expect("observe");
        assert_eq!(observed.snapshot.status, RunStatus::Waiting);
        assert_eq!(
            observed.snapshot.nodes[&NodeKey::from("work")].status,
            NodeStatus::Succeeded
        );
        assert_eq!(invokes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_backoff_is_persisted_and_honored() {
        let invokes = Arc::new(AtomicUsize::new(0));
        let runtime = DurableWorkflowRuntime::new(
            Arc::new(InMemoryWorkflowStore::default()),
            ActivityRegistry::new([Arc::new(DelayedActivity {
                invokes: invokes.clone(),
                delay: Duration::ZERO,
                fail_first: true,
            }) as Arc<dyn ActivityAdapter>])
            .expect("activities"),
        );
        let mut spec = echo_spec();
        spec.nodes[0].kind = NodeKind::Activity {
            kind: "test.delayed".to_string(),
            config: json!({}),
        };
        spec.nodes[0].retry = RetryPolicy {
            max_attempts: 2,
            backoff_ms: 50,
        };
        spec.result = ValueExpr::NodeOutput {
            node: NodeKey::from("echo"),
            pointer: String::new(),
        };
        let started = std::time::Instant::now();
        let receipt = runtime
            .start(StartRun {
                request_id: "retry-backoff".to_string(),
                source: WorkflowSource::Inline(spec),
                input: json!({ "value": 1 }),
                scope: RunScope::default(),
            })
            .await
            .expect("start");
        let observed = loop {
            let observed = runtime
                .observe(ObserveRun {
                    run_id: receipt.run_id.clone(),
                    after_sequence: None,
                })
                .await
                .expect("observe");
            if observed.snapshot.status.is_terminal() {
                break observed;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(observed.snapshot.status, RunStatus::Succeeded);
        assert!(started.elapsed() >= Duration::from_millis(40));
        assert_eq!(invokes.load(Ordering::SeqCst), 2);
        assert!(observed.events.iter().any(|event| matches!(
            &event.kind,
            WorkflowEventKind::RetryScheduled { retry_at, .. } if !retry_at.is_empty()
        )));
    }

    #[tokio::test]
    async fn recovery_respects_max_attempts() {
        let invokes = Arc::new(AtomicUsize::new(0));
        let store = Arc::new(InMemoryWorkflowStore::default());
        let runtime = DurableWorkflowRuntime::new(
            store.clone(),
            ActivityRegistry::new([Arc::new(RecoveryActivity {
                invokes: invokes.clone(),
            }) as Arc<dyn ActivityAdapter>])
            .expect("activities"),
        );
        let mut spec = echo_spec();
        spec.nodes[0].kind = NodeKind::Activity {
            kind: "test.recovery".to_string(),
            config: json!({}),
        };
        spec.nodes[0].retry.max_attempts = 1;
        let run_id = RunId("interrupted-at-limit".to_string());
        store
            .create_or_get(CreateStoredRun {
                run: StoredRun {
                    run_id: run_id.clone(),
                    request_id: "interrupted-at-limit-request".to_string(),
                    manifest: RunManifest {
                        program_hash: "frozen".to_string(),
                        program: spec,
                        adapter_versions: BTreeMap::new(),
                    },
                    input: json!({ "value": 1 }),
                    scope: RunScope::default(),
                },
            })
            .expect("create");
        store
            .append(
                &run_id,
                0,
                vec![
                    WorkflowEventKind::RunStarted,
                    WorkflowEventKind::NodeScheduled {
                        node: NodeKey::from("echo"),
                        node_instance_id: "interrupted-at-limit:echo".to_string(),
                        attempt_id: "interrupted-at-limit:echo:1".to_string(),
                        attempt: 1,
                        effect_key: "interrupted-at-limit-request:echo".to_string(),
                    },
                    WorkflowEventKind::AttemptStarted {
                        node: NodeKey::from("echo"),
                        attempt_id: "interrupted-at-limit:echo:1".to_string(),
                    },
                ],
            )
            .expect("seed");

        runtime.recover().await.expect("recover");
        tokio::time::sleep(Duration::from_millis(20)).await;
        let observed = runtime
            .observe(ObserveRun {
                run_id,
                after_sequence: None,
            })
            .await
            .expect("observe");
        assert_eq!(observed.snapshot.status, RunStatus::Failed);
        assert_eq!(invokes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancelling_timer_stops_driver_promptly() {
        let runtime = DurableWorkflowRuntime::new(
            Arc::new(InMemoryWorkflowStore::default()),
            ActivityRegistry::default(),
        );
        let receipt = runtime
            .start(StartRun {
                request_id: "cancel-timer".to_string(),
                source: WorkflowSource::Inline(WorkflowSpec {
                    schema_version: 1,
                    nodes: vec![NodeSpec {
                        key: NodeKey::from("timer"),
                        kind: NodeKind::Timer { delay_ms: 5_000 },
                        inputs: BTreeMap::new(),
                        after: Vec::new(),
                        retry: RetryPolicy::default(),
                        timeout_ms: None,
                        effect: EffectPolicy::Pure,
                        resources: Vec::new(),
                    }],
                    result: ValueExpr::NodeOutput {
                        node: NodeKey::from("timer"),
                        pointer: String::new(),
                    },
                    policy: WorkflowPolicy::default(),
                }),
                input: json!({}),
                scope: RunScope::default(),
            })
            .await
            .expect("start");
        tokio::time::sleep(Duration::from_millis(10)).await;
        runtime
            .command(
                &receipt.run_id,
                WorkflowCommand::Cancel {
                    command_id: "cancel-timer-once".to_string(),
                    reason: "test".to_string(),
                },
            )
            .await
            .expect("cancel");
        tokio::time::timeout(Duration::from_millis(100), async {
            loop {
                if !runtime.inner.active.lock().contains_key(&receipt.run_id) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("driver should stop after cancellation");
    }

    #[tokio::test]
    async fn signal_received_before_wait_node_is_consumed() {
        let runtime = DurableWorkflowRuntime::new(
            Arc::new(InMemoryWorkflowStore::default()),
            ActivityRegistry::default(),
        );
        let receipt = runtime
            .start(StartRun {
                request_id: "early-signal".to_string(),
                source: WorkflowSource::Inline(WorkflowSpec {
                    schema_version: 1,
                    nodes: vec![
                        NodeSpec {
                            key: NodeKey::from("delay"),
                            kind: NodeKind::Timer { delay_ms: 30 },
                            inputs: BTreeMap::new(),
                            after: Vec::new(),
                            retry: RetryPolicy::default(),
                            timeout_ms: None,
                            effect: EffectPolicy::Pure,
                            resources: Vec::new(),
                        },
                        NodeSpec {
                            key: NodeKey::from("approval"),
                            kind: NodeKind::WaitSignal {
                                name: "approve".to_string(),
                            },
                            inputs: BTreeMap::new(),
                            after: vec![NodeKey::from("delay")],
                            retry: RetryPolicy::default(),
                            timeout_ms: None,
                            effect: EffectPolicy::Pure,
                            resources: Vec::new(),
                        },
                    ],
                    result: ValueExpr::NodeOutput {
                        node: NodeKey::from("approval"),
                        pointer: "/payload".to_string(),
                    },
                    policy: WorkflowPolicy::default(),
                }),
                input: json!({}),
                scope: RunScope::default(),
            })
            .await
            .expect("start");
        runtime
            .command(
                &receipt.run_id,
                WorkflowCommand::Signal {
                    command_id: "early-approve".to_string(),
                    name: "approve".to_string(),
                    payload: json!({ "approved": true }),
                },
            )
            .await
            .expect("signal");

        let observed = tokio::time::timeout(Duration::from_millis(250), async {
            loop {
                let observed = runtime
                    .observe(ObserveRun {
                        run_id: receipt.run_id.clone(),
                        after_sequence: None,
                    })
                    .await
                    .expect("observe");
                if observed.snapshot.status.is_terminal() {
                    break observed;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("early signal should not be lost");
        assert_eq!(observed.snapshot.status, RunStatus::Succeeded);
        assert_eq!(observed.snapshot.output, json!({ "approved": true }));
        assert!(observed.events.iter().any(|event| matches!(
            &event.kind,
            WorkflowEventKind::SignalConsumed { command_id, .. }
                if command_id == "early-approve"
        )));
    }

    #[tokio::test]
    async fn adapter_error_is_recorded_as_node_failure() {
        let runtime = DurableWorkflowRuntime::new(
            Arc::new(InMemoryWorkflowStore::default()),
            ActivityRegistry::new([Arc::new(ErrorActivity) as Arc<dyn ActivityAdapter>])
                .expect("activities"),
        );
        let mut spec = echo_spec();
        spec.nodes[0].kind = NodeKind::Activity {
            kind: "test.error".to_string(),
            config: json!({}),
        };
        let receipt = runtime
            .start(StartRun {
                request_id: "adapter-error".to_string(),
                source: WorkflowSource::Inline(spec),
                input: json!({ "value": 1 }),
                scope: RunScope::default(),
            })
            .await
            .expect("start");
        let observed = loop {
            let observed = runtime
                .observe(ObserveRun {
                    run_id: receipt.run_id.clone(),
                    after_sequence: None,
                })
                .await
                .expect("observe");
            if observed.snapshot.status.is_terminal() {
                break observed;
            }
            tokio::task::yield_now().await;
        };
        assert_eq!(observed.snapshot.status, RunStatus::Failed);
        assert_eq!(
            observed.snapshot.nodes[&NodeKey::from("echo")].status,
            NodeStatus::Failed
        );
        assert!(observed.events.iter().any(|event| matches!(
            &event.kind,
            WorkflowEventKind::NodeFailed { error, .. } if error.contains("adapter failed")
        )));
    }
}
