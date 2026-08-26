use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::tools::Tool;

use super::{
    DurableWorkflowRuntime, NodeKey, NodeKind, NodeSpec, ObserveRun, RetryPolicy, RunScope,
    StartRun, ValueExpr, WorkflowAuthoringService, WorkflowCommand, WorkflowPolicy,
    WorkflowRuntime, WorkflowSource, WorkflowSpec, WorkflowStore, reducer::validate_spec,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowMention {
    pub workflow_id: String,
    pub revision_id: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub display_token: String,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowMentionManifest {
    #[serde(default)]
    pub mentions: Vec<WorkflowMention>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowMentionTask {
    pub key: NodeKey,
    pub workflow_id: String,
    #[serde(default)]
    pub depends_on: Vec<NodeKey>,
    #[serde(default)]
    pub inputs: BTreeMap<String, ValueExpr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowMentionPlan {
    pub tasks: Vec<WorkflowMentionTask>,
    pub result: ValueExpr,
    #[serde(default)]
    pub policy: WorkflowPolicy,
}

pub struct WorkflowMentionCompiler {
    authoring: Arc<WorkflowAuthoringService>,
}

impl WorkflowMentionCompiler {
    pub fn new(authoring: Arc<WorkflowAuthoringService>) -> Self {
        Self { authoring }
    }

    pub fn compile(
        &self,
        manifest: &WorkflowMentionManifest,
        plan: WorkflowMentionPlan,
    ) -> Result<WorkflowSpec> {
        if manifest.mentions.is_empty() {
            bail!("workflow mention manifest must not be empty");
        }
        if plan.tasks.is_empty() {
            bail!("mentioned workflow plan must contain at least one task");
        }
        let mut mentioned = HashMap::new();
        for mention in &manifest.mentions {
            let receipt = self
                .authoring
                .resolve_published_revision(&mention.workflow_id, Some(&mention.revision_id))?;
            let spec = self.authoring.load_published_spec(&receipt.revision_id)?;
            if mentioned
                .insert(mention.workflow_id.as_str(), (mention, spec))
                .is_some()
            {
                bail!(
                    "workflow '{}' is mentioned more than once",
                    mention.workflow_id
                );
            }
        }

        let mut used = HashSet::new();
        let mut nodes = Vec::with_capacity(plan.tasks.len());
        for task in plan.tasks {
            let (mention, child_spec) =
                mentioned.get(task.workflow_id.as_str()).ok_or_else(|| {
                    anyhow::anyhow!(
                        "task '{}' references workflow '{}' outside the mention manifest",
                        task.key.0,
                        task.workflow_id
                    )
                })?;
            if let Some(required) = child_spec
                .nodes
                .iter()
                .flat_map(|node| node.inputs.values())
                .filter_map(|expr| match expr {
                    ValueExpr::RunInput { pointer } => pointer
                        .strip_prefix('/')
                        .and_then(|field| field.split('/').next())
                        .filter(|field| !field.is_empty()),
                    _ => None,
                })
                .find(|field| !task.inputs.contains_key(*field))
            {
                bail!(
                    "task '{}' is missing required input '{}' for workflow '{}'",
                    task.key.0,
                    required,
                    task.workflow_id
                );
            }
            used.insert(task.workflow_id.clone());
            nodes.push(NodeSpec {
                key: task.key,
                kind: NodeKind::ChildWorkflow {
                    revision_id: super::WorkflowRevisionId(mention.revision_id.clone()),
                },
                inputs: task.inputs,
                after: task.depends_on,
                retry: RetryPolicy {
                    max_attempts: 2,
                    backoff_ms: 0,
                },
                timeout_ms: None,
                effect: super::EffectPolicy::ReadOnly,
                resources: Vec::new(),
            });
        }
        for mention in &manifest.mentions {
            if !mention.optional && !used.contains(&mention.workflow_id) {
                bail!(
                    "mentioned workflow '{}' is not assigned to any task",
                    mention.workflow_id
                );
            }
        }
        let spec = WorkflowSpec {
            schema_version: 1,
            nodes,
            result: plan.result,
            policy: plan.policy,
        };
        validate_spec(&spec)?;
        Ok(spec)
    }
}

pub struct WorkflowMentionTool<S: WorkflowStore> {
    runtime: Arc<DurableWorkflowRuntime<S>>,
    compiler: WorkflowMentionCompiler,
    manifest: WorkflowMentionManifest,
    scope: RunScope,
    parent_cancel: CancellationToken,
    description: String,
}

impl<S: WorkflowStore> WorkflowMentionTool<S> {
    pub fn new(
        runtime: Arc<DurableWorkflowRuntime<S>>,
        authoring: Arc<WorkflowAuthoringService>,
        manifest: WorkflowMentionManifest,
        scope: RunScope,
        parent_cancel: CancellationToken,
        description: String,
    ) -> Self {
        Self {
            runtime,
            compiler: WorkflowMentionCompiler::new(authoring),
            manifest,
            scope,
            parent_cancel,
            description,
        }
    }
}

#[async_trait]
impl<S: WorkflowStore> Tool for WorkflowMentionTool<S> {
    fn name(&self) -> &str {
        "run_mentioned_workflows"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["tasks", "result"],
            "properties": {
                "tasks": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["key", "workflow_id"],
                        "properties": {
                            "key": {"type": "string"},
                            "workflow_id": {"type": "string"},
                            "depends_on": {
                                "type": "array",
                                "items": {"type": "string"},
                                "default": []
                            },
                            "inputs": {
                                "type": "object",
                                "description": "Input fields expressed as ValueExpr objects. Bind upstream output with source=node_output.",
                                "additionalProperties": true,
                                "default": {}
                            }
                        }
                    }
                },
                "result": {
                    "type": "object",
                    "description": "ValueExpr selecting the final parent workflow result.",
                    "additionalProperties": true
                },
                "policy": {"type": "object", "additionalProperties": true}
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let plan: WorkflowMentionPlan = serde_json::from_value(args.clone())
            .map_err(|error| anyhow::anyhow!("invalid mentioned workflow plan: {error}"))?;
        let spec = self.compiler.compile(&self.manifest, plan)?;
        let call_hash = hex::encode(Sha256::digest(serde_json::to_vec(&args)?));
        let receipt = self
            .runtime
            .start(StartRun {
                request_id: format!(
                    "workflow-mention:{}:{}",
                    self.scope.parent_prompt_id, call_hash
                ),
                source: WorkflowSource::Inline(spec),
                input: json!({}),
                scope: self.scope.clone(),
            })
            .await?;
        loop {
            let observation = self
                .runtime
                .observe(ObserveRun {
                    run_id: receipt.run_id.clone(),
                    after_sequence: None,
                })
                .await?;
            if observation.snapshot.status.is_terminal() {
                return Ok(serde_json::to_string(&json!({
                    "run_id": receipt.run_id,
                    "status": observation.snapshot.status,
                    "output": observation.snapshot.output,
                    "error": observation.snapshot.error,
                }))?);
            }
            tokio::select! {
                _ = self.parent_cancel.cancelled() => {
                    let _ = self.runtime.command(
                        &receipt.run_id,
                        WorkflowCommand::Cancel {
                            command_id: format!("parent-cancel:{}", self.scope.parent_prompt_id),
                            reason: "parent agent run was cancelled".to_string(),
                        },
                    ).await;
                    bail!("parent agent run was cancelled");
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            }
        }
    }
}

pub fn workflow_mention_tool_factory<S: WorkflowStore>(
    runtime: Arc<DurableWorkflowRuntime<S>>,
    authoring: Arc<WorkflowAuthoringService>,
    manifest: WorkflowMentionManifest,
    scope: RunScope,
) -> crate::runtime::run::ScopedToolFactory {
    Arc::new(move |registry, cancel_token, parent_run_id| {
        let mut bound_scope = scope.clone();
        bound_scope.parent_run_id = parent_run_id;
        let allowed = manifest
            .mentions
            .iter()
            .map(|mention| format!("{}@{}", mention.workflow_id, mention.revision_id))
            .collect::<Vec<_>>()
            .join(", ");
        registry.register(Box::new(WorkflowMentionTool::new(
            runtime.clone(),
            authoring.clone(),
            manifest.clone(),
            bound_scope,
            cancel_token,
            format!(
                "Plan and run the published workflows explicitly mentioned by the user. \
                 This call is REQUIRED before answering. Use only the pinned workflow revisions \
                 in this manifest: {allowed}. For multiple workflows, express dependencies with \
                 depends_on and bind upstream outputs through explicit inputs. Ask the user when \
                 a required workflow input cannot be derived from the message."
            ),
        )));
        Some("run_mentioned_workflows".to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        memory::storage::Storage,
        permission::PermissionConfig,
        workflow::runtime::{
            AgentBinding, ApplyWorkflowDraft, DraftStep, InMemoryWorkflowStore,
            InlineAgentBlueprint, PublishWorkflowDraft, WorkflowDraftSpec,
        },
    };

    #[test]
    fn compiles_pinned_workflows_into_child_nodes() {
        let directory = tempfile::tempdir().expect("tempdir");
        let storage =
            Storage::new(directory.path().join("db.sqlite").to_str().expect("path")).expect("db");
        let store = Arc::new(InMemoryWorkflowStore::default());
        let authoring = Arc::new(WorkflowAuthoringService::new(storage, store).expect("authoring"));
        let draft = authoring
            .apply_draft(
                ApplyWorkflowDraft {
                    request_id: "create".into(),
                    draft_id: None,
                    expected_version: None,
                    workflow: WorkflowDraftSpec {
                        name: "Research".into(),
                        description: String::new(),
                        input_schema: json!({}),
                        steps: vec![DraftStep {
                            key: NodeKey::from("work"),
                            agent: AgentBinding::Inline {
                                blueprint: InlineAgentBlueprint {
                                    name: "Researcher".into(),
                                    description: String::new(),
                                    system_prompt: "Research".into(),
                                    model: String::new(),
                                    skills: Vec::new(),
                                    tools: Vec::new(),
                                    permission_mode: "standard".into(),
                                    max_iterations: 5,
                                    max_context_tokens: 1000,
                                    icon: String::new(),
                                    color: String::new(),
                                },
                            },
                            instruction: "Research".into(),
                            inputs: BTreeMap::new(),
                            after: Vec::new(),
                            retry: RetryPolicy::default(),
                            timeout_ms: None,
                        }],
                        result: ValueExpr::NodeOutput {
                            node: NodeKey::from("work"),
                            pointer: String::new(),
                        },
                        policy: WorkflowPolicy::default(),
                    },
                },
                &PermissionConfig::default(),
            )
            .expect("draft");
        let published = authoring
            .publish(PublishWorkflowDraft {
                request_id: "publish".into(),
                draft_id: draft.draft_id,
                expected_version: draft.version,
            })
            .expect("publish");
        let compiler = WorkflowMentionCompiler::new(authoring);
        let spec = compiler
            .compile(
                &WorkflowMentionManifest {
                    mentions: vec![WorkflowMention {
                        workflow_id: published.workflow_id.clone(),
                        revision_id: published.revision_id.0.clone(),
                        scope: "user".into(),
                        display_token: "@workflow:Research".into(),
                        optional: false,
                    }],
                },
                WorkflowMentionPlan {
                    tasks: vec![WorkflowMentionTask {
                        key: NodeKey::from("research"),
                        workflow_id: published.workflow_id,
                        depends_on: Vec::new(),
                        inputs: BTreeMap::new(),
                    }],
                    result: ValueExpr::NodeOutput {
                        node: NodeKey::from("research"),
                        pointer: String::new(),
                    },
                    policy: WorkflowPolicy::default(),
                },
            )
            .expect("compile");
        assert!(matches!(spec.nodes[0].kind, NodeKind::ChildWorkflow { .. }));
    }
}
