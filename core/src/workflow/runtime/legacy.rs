use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, bail};

use crate::{
    agent_registry,
    memory::storage::Storage,
    permission::{DangerLevel, PermissionConfig, PermissionMode},
    workflow::{NodeType, OnNodeFailure, TrustMode, WorkflowDef},
};

use super::{
    custom_agent::{CUSTOM_AGENT_ACTIVITY_KIND, FrozenCustomAgentConfig},
    mention::intersect_permission_ceiling,
    model::{
        EffectPolicy, FailurePolicy, NodeKey, NodeKind, NodeSpec, ResourceClaim, RetryPolicy,
        ValueExpr, WorkflowPolicy, WorkflowSpec,
    },
    reducer::validate_spec,
};

#[derive(Clone)]
pub struct LegacyWorkflowCompiler {
    storage: Storage,
}

impl LegacyWorkflowCompiler {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }

    pub fn compile(
        &self,
        workflow: &WorkflowDef,
        caller_permission: &PermissionConfig,
    ) -> Result<WorkflowSpec> {
        let effective_ceiling = legacy_permission_ceiling(&workflow.trust_mode, caller_permission);
        let incoming =
            workflow
                .edges
                .iter()
                .fold(HashMap::<&str, Vec<&str>>::new(), |mut map, edge| {
                    map.entry(&edge.target_node_id)
                        .or_default()
                        .push(&edge.source_node_id);
                    map
                });
        let mut nodes = Vec::with_capacity(workflow.nodes.len());
        for node in &workflow.nodes {
            let dependencies: Vec<_> = incoming
                .get(node.id.as_str())
                .into_iter()
                .flatten()
                .map(|source| NodeKey((*source).to_string()))
                .collect();
            let upstream_inputs = || {
                dependencies
                    .iter()
                    .enumerate()
                    .map(|(index, dependency)| {
                        let port = if dependencies.len() == 1 {
                            "upstream".to_string()
                        } else {
                            format!("upstream_{index}")
                        };
                        (
                            port,
                            ValueExpr::NodeOutput {
                                node: dependency.clone(),
                                pointer: String::new(),
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>()
            };
            let (kind, inputs, effect, resources) = match node.node_type {
                NodeType::Input => (
                    NodeKind::Output,
                    BTreeMap::from([(
                        "value".to_string(),
                        ValueExpr::RunInput {
                            pointer: String::new(),
                        },
                    )]),
                    EffectPolicy::Pure,
                    Vec::new(),
                ),
                NodeType::Output => (
                    NodeKind::Output,
                    upstream_inputs(),
                    EffectPolicy::Pure,
                    Vec::new(),
                ),
                NodeType::Transform => (
                    NodeKind::Choice {
                        config: node.config.clone(),
                    },
                    upstream_inputs(),
                    EffectPolicy::Pure,
                    Vec::new(),
                ),
                NodeType::HumanApproval => (
                    NodeKind::WaitSignal {
                        name: format!("approval:{}", node.id),
                    },
                    upstream_inputs(),
                    EffectPolicy::Pure,
                    Vec::new(),
                ),
                NodeType::Agent => {
                    if node.agent_id.is_empty() {
                        bail!("legacy agent node '{}' has no agent_id", node.id);
                    }
                    let agent = agent_registry::get(&self.storage, &node.agent_id)
                        .with_context(|| format!("resolve agent node '{}'", node.id))?;
                    let requested =
                        agent_registry::build_permission_config(&agent, &effective_ceiling);
                    let permission = intersect_permission_ceiling(&effective_ceiling, requested);
                    let effect = classify_agent_effect(&agent.tools, &permission);
                    let mut inputs = upstream_inputs();
                    inputs.insert(
                        "instruction".to_string(),
                        ValueExpr::Literal {
                            value: serde_json::Value::String(
                                node.config
                                    .get("instruction")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or(&node.label)
                                    .to_string(),
                            ),
                        },
                    );
                    let resources = if effect == EffectPolicy::WorkspaceWrite {
                        vec![ResourceClaim {
                            resource: "workspace".to_string(),
                            exclusive: true,
                        }]
                    } else {
                        Vec::new()
                    };
                    (
                        NodeKind::Activity {
                            kind: CUSTOM_AGENT_ACTIVITY_KIND.to_string(),
                            config: serde_json::to_value(FrozenCustomAgentConfig {
                                agent,
                                permission,
                                record_history: true,
                            })?,
                        },
                        inputs,
                        effect,
                        resources,
                    )
                }
            };
            nodes.push(NodeSpec {
                key: NodeKey(node.id.clone()),
                kind,
                inputs,
                after: dependencies,
                retry: RetryPolicy::default(),
                timeout_ms: node
                    .config
                    .get("timeout_ms")
                    .and_then(|value| value.as_u64()),
                effect,
                resources,
            });
        }

        let result_node = workflow
            .nodes
            .iter()
            .find(|node| node.node_type == NodeType::Output)
            .or_else(|| {
                workflow
                    .nodes
                    .iter()
                    .rev()
                    .find(|node| node.node_type == NodeType::Agent)
            })
            .ok_or_else(|| anyhow::anyhow!("workflow has no output-producing node"))?;
        let spec = WorkflowSpec {
            schema_version: 1,
            nodes,
            result: ValueExpr::NodeOutput {
                node: NodeKey(result_node.id.clone()),
                pointer: String::new(),
            },
            policy: WorkflowPolicy {
                max_concurrency: workflow.max_concurrent.max(1),
                on_failure: match workflow.on_node_failure {
                    OnNodeFailure::Abort => FailurePolicy::Abort,
                    OnNodeFailure::Continue | OnNodeFailure::Skip => FailurePolicy::Continue,
                },
            },
        };
        validate_spec(&spec)?;
        Ok(spec)
    }
}

fn legacy_permission_ceiling(trust: &TrustMode, caller: &PermissionConfig) -> PermissionConfig {
    let mut ceiling = caller.clone();
    if matches!(trust, TrustMode::Readonly) {
        ceiling.mode = PermissionMode::Paranoid;
        ceiling.auto_allow_up_to = Some(DangerLevel::ReadOnly);
    }
    // Legacy "trusted" is deliberately treated as inherit: a saved workflow
    // may narrow caller authority, but never raise it to Yolo.
    ceiling
}

pub(crate) fn classify_agent_effect(
    tools: &[String],
    permission: &PermissionConfig,
) -> EffectPolicy {
    const READ_ONLY_TOOLS: &[&str] = &[
        "read_file",
        "grep",
        "glob",
        "list_directory",
        "git_status",
        "git_diff",
        "git_log",
        "git_show",
        "web_search",
        "web_fetch",
    ];
    if permission.auto_allow_up_to == Some(DangerLevel::ReadOnly)
        || (!tools.is_empty()
            && tools
                .iter()
                .all(|tool| READ_ONLY_TOOLS.contains(&tool.as_str())))
    {
        EffectPolicy::ReadOnly
    } else {
        EffectPolicy::WorkspaceWrite
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent_registry::AgentDef,
        workflow::{EdgeDef, NodeDef},
    };

    #[test]
    fn legacy_trusted_workflow_cannot_elevate_caller_to_yolo() {
        let directory = tempfile::tempdir().expect("tempdir");
        let storage =
            Storage::new(directory.path().join("db.sqlite").to_str().expect("path")).expect("db");
        agent_registry::create(
            &storage,
            &AgentDef {
                id: "agent".to_string(),
                name: "Agent".to_string(),
                permission_mode: "yolo".to_string(),
                ..AgentDef::default()
            },
        )
        .expect("agent");
        let workflow = WorkflowDef {
            trust_mode: TrustMode::Trusted,
            nodes: vec![
                NodeDef {
                    id: "input".to_string(),
                    workflow_id: "wf".to_string(),
                    node_type: NodeType::Input,
                    label: "Input".to_string(),
                    agent_id: String::new(),
                    config: serde_json::json!({}),
                    position_x: 0.0,
                    position_y: 0.0,
                    created_at: String::new(),
                },
                NodeDef {
                    id: "agent-node".to_string(),
                    workflow_id: "wf".to_string(),
                    node_type: NodeType::Agent,
                    label: "Do work".to_string(),
                    agent_id: "agent".to_string(),
                    config: serde_json::json!({}),
                    position_x: 0.0,
                    position_y: 0.0,
                    created_at: String::new(),
                },
            ],
            edges: vec![EdgeDef {
                id: "edge".to_string(),
                workflow_id: "wf".to_string(),
                source_node_id: "input".to_string(),
                target_node_id: "agent-node".to_string(),
                source_handle: String::new(),
                target_handle: String::new(),
                label: String::new(),
                condition: String::new(),
                data_mapping: serde_json::json!({"pass_through": true}),
                created_at: String::new(),
            }],
            ..WorkflowDef::default()
        };
        let mut caller = PermissionConfig::default();
        caller.mode = PermissionMode::Standard;
        let spec = LegacyWorkflowCompiler::new(storage)
            .compile(&workflow, &caller)
            .expect("compile");
        let NodeKind::Activity { config, .. } = &spec.nodes[1].kind else {
            panic!("activity")
        };
        assert_eq!(config["permission"]["mode"], "standard");
    }
}
