use std::collections::{BTreeMap, HashSet, VecDeque};

use anyhow::{Result, bail};
use serde_json::{Map, Value};

use super::model::{
    FailurePolicy, NodeKey, NodeSnapshot, NodeStatus, RunSnapshot, RunStatus, StoredRun, ValueExpr,
    WorkflowEvent, WorkflowEventKind, WorkflowSpec,
};

pub fn validate_spec(spec: &WorkflowSpec) -> Result<()> {
    if spec.schema_version != 1 {
        bail!(
            "unsupported workflow schema version: {}",
            spec.schema_version
        );
    }
    if spec.nodes.is_empty() {
        bail!("workflow must contain at least one node");
    }

    let keys: HashSet<_> = spec.nodes.iter().map(|node| node.key.clone()).collect();
    if keys.len() != spec.nodes.len() {
        bail!("workflow node keys must be unique");
    }

    for node in &spec.nodes {
        for dependency in &node.after {
            if !keys.contains(dependency) {
                bail!(
                    "workflow node '{}' depends on missing node '{}'",
                    node.key.0,
                    dependency.0
                );
            }
        }
        for expression in node.inputs.values() {
            validate_value_expr(expression, &keys, Some((&node.key, &node.after)))?;
        }
    }
    validate_value_expr(&spec.result, &keys, None)?;

    let mut indegree: BTreeMap<NodeKey, usize> = spec
        .nodes
        .iter()
        .map(|node| (node.key.clone(), 0))
        .collect();
    let mut outgoing: BTreeMap<NodeKey, Vec<NodeKey>> = BTreeMap::new();
    for node in &spec.nodes {
        for dependency in &node.after {
            *indegree
                .get_mut(&node.key)
                .ok_or_else(|| anyhow::anyhow!("missing node indegree"))? += 1;
            outgoing
                .entry(dependency.clone())
                .or_default()
                .push(node.key.clone());
        }
    }

    let mut queue: VecDeque<_> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(key, _)| key.clone())
        .collect();
    let mut visited = 0usize;
    while let Some(key) = queue.pop_front() {
        visited += 1;
        for target in outgoing.get(&key).into_iter().flatten() {
            let degree = indegree
                .get_mut(target)
                .ok_or_else(|| anyhow::anyhow!("missing target indegree"))?;
            *degree -= 1;
            if *degree == 0 {
                queue.push_back(target.clone());
            }
        }
    }
    if visited != spec.nodes.len() {
        bail!("workflow contains a cycle");
    }
    Ok(())
}

fn validate_value_expr(
    expression: &ValueExpr,
    keys: &HashSet<NodeKey>,
    consumer: Option<(&NodeKey, &[NodeKey])>,
) -> Result<()> {
    match expression {
        ValueExpr::NodeOutput { node, .. } => {
            if !keys.contains(node) {
                bail!("value expression references missing node '{}'", node.0);
            }
            if let Some((consumer, dependencies)) = consumer {
                if !dependencies.contains(node) {
                    bail!(
                        "node '{}' reads '{}' without declaring it in after",
                        consumer.0,
                        node.0
                    );
                }
            }
        }
        ValueExpr::Object { fields } => {
            for nested in fields.values() {
                validate_value_expr(nested, keys, consumer)?;
            }
        }
        ValueExpr::Array { items } => {
            for nested in items {
                validate_value_expr(nested, keys, consumer)?;
            }
        }
        ValueExpr::Literal { .. } | ValueExpr::RunInput { .. } => {}
    }
    Ok(())
}

pub fn replay(run: &StoredRun, events: &[WorkflowEvent]) -> Result<RunSnapshot> {
    let mut snapshot = RunSnapshot {
        run_id: run.run_id.clone(),
        request_id: run.request_id.clone(),
        status: RunStatus::Pending,
        last_sequence: 0,
        nodes: run
            .manifest
            .program
            .nodes
            .iter()
            .map(|node| {
                (
                    node.key.clone(),
                    NodeSnapshot {
                        status: NodeStatus::Pending,
                        attempt: 0,
                        output: Value::Null,
                        artifacts: Vec::new(),
                        error: String::new(),
                        retryable: false,
                    },
                )
            })
            .collect(),
        output: Value::Null,
        error: String::new(),
    };

    for event in events {
        snapshot.last_sequence = event.sequence;
        match &event.kind {
            WorkflowEventKind::RunCreated => {}
            WorkflowEventKind::RunStarted => snapshot.status = RunStatus::Running,
            WorkflowEventKind::NodeScheduled { node, attempt, .. } => {
                let state = node_mut(&mut snapshot, node)?;
                state.status = NodeStatus::Scheduled;
                state.attempt = *attempt;
                state.error.clear();
            }
            WorkflowEventKind::AttemptStarted { node, .. } => {
                node_mut(&mut snapshot, node)?.status = NodeStatus::Running;
            }
            WorkflowEventKind::NodeCompleted {
                node,
                output,
                artifacts,
            } => {
                let state = node_mut(&mut snapshot, node)?;
                state.status = NodeStatus::Succeeded;
                state.output = output.clone();
                state.artifacts = artifacts.clone();
                state.error.clear();
                state.retryable = false;
            }
            WorkflowEventKind::NodeFailed {
                node,
                error,
                retryable,
            } => {
                let state = node_mut(&mut snapshot, node)?;
                state.status = NodeStatus::Failed;
                state.error = error.clone();
                state.retryable = *retryable;
            }
            WorkflowEventKind::RetryScheduled { node, .. } => {
                node_mut(&mut snapshot, node)?.status = NodeStatus::Pending;
            }
            WorkflowEventKind::NodeWaiting { node, .. } => {
                node_mut(&mut snapshot, node)?.status = NodeStatus::Waiting;
            }
            WorkflowEventKind::TimerScheduled { .. } | WorkflowEventKind::TimerFired { .. } => {}
            WorkflowEventKind::NodeNeedsAttention { node, reason } => {
                let state = node_mut(&mut snapshot, node)?;
                state.status = NodeStatus::NeedsAttention;
                state.error = reason.clone();
                snapshot.status = RunStatus::NeedsAttention;
                snapshot.error = reason.clone();
            }
            WorkflowEventKind::SignalReceived { .. } | WorkflowEventKind::SignalConsumed { .. } => {
            }
            WorkflowEventKind::RunPaused { .. } => snapshot.status = RunStatus::Paused,
            WorkflowEventKind::RunResumed { .. } => snapshot.status = RunStatus::Running,
            WorkflowEventKind::RunCompleted { output } => {
                snapshot.status = RunStatus::Succeeded;
                snapshot.output = output.clone();
            }
            WorkflowEventKind::RunFailed { error } => {
                snapshot.status = RunStatus::Failed;
                snapshot.error = error.clone();
            }
            WorkflowEventKind::RunCancelled { .. } => snapshot.status = RunStatus::Cancelled,
        }
    }
    if snapshot.status == RunStatus::Running
        && snapshot
            .nodes
            .values()
            .any(|node| node.status == NodeStatus::Waiting)
        && !snapshot
            .nodes
            .values()
            .any(|node| matches!(node.status, NodeStatus::Scheduled | NodeStatus::Running))
        && ready_nodes(&run.manifest.program, &snapshot).is_empty()
    {
        snapshot.status = RunStatus::Waiting;
    }
    Ok(snapshot)
}

fn node_mut<'a>(snapshot: &'a mut RunSnapshot, node: &NodeKey) -> Result<&'a mut NodeSnapshot> {
    snapshot
        .nodes
        .get_mut(node)
        .ok_or_else(|| anyhow::anyhow!("event references missing workflow node: {}", node.0))
}

pub fn ready_nodes(spec: &WorkflowSpec, snapshot: &RunSnapshot) -> Vec<NodeKey> {
    if snapshot.status != RunStatus::Running {
        return Vec::new();
    }
    spec.nodes
        .iter()
        .filter(|node| {
            snapshot
                .nodes
                .get(&node.key)
                .is_some_and(|state| state.status == NodeStatus::Pending)
                && node.after.iter().all(|dependency| {
                    snapshot.nodes.get(dependency).is_some_and(|state| {
                        state.status == NodeStatus::Succeeded
                            || (matches!(spec.policy.on_failure, FailurePolicy::Continue)
                                && state.status == NodeStatus::Failed)
                    })
                })
        })
        .map(|node| node.key.clone())
        .collect()
}

pub fn resolve_value(expr: &ValueExpr, run: &StoredRun, snapshot: &RunSnapshot) -> Result<Value> {
    match expr {
        ValueExpr::Literal { value } => Ok(value.clone()),
        ValueExpr::RunInput { pointer } => resolve_pointer(&run.input, pointer),
        ValueExpr::NodeOutput { node, pointer } => {
            let output = snapshot
                .nodes
                .get(node)
                .ok_or_else(|| {
                    anyhow::anyhow!("value expression references missing node: {}", node.0)
                })?
                .output
                .clone();
            resolve_pointer(&output, pointer)
        }
        ValueExpr::Object { fields } => {
            let fields: Result<Map<String, Value>> = fields
                .iter()
                .map(|(key, value)| Ok((key.clone(), resolve_value(value, run, snapshot)?)))
                .collect();
            Ok(Value::Object(fields?))
        }
        ValueExpr::Array { items } => items
            .iter()
            .map(|item| resolve_value(item, run, snapshot))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
    }
}

fn resolve_pointer(value: &Value, pointer: &str) -> Result<Value> {
    if pointer.is_empty() {
        return Ok(value.clone());
    }
    value
        .pointer(pointer)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("JSON pointer not found: {pointer}"))
}
