use std::sync::Arc;

use anyhow::{bail, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{permission::PermissionConfig, tools::Tool};

use super::{
    engine::WorkflowRuntime,
    mention::{derive_mention_request_id, MentionManifest, MentionPlan, MentionWorkflowCompiler},
    model::{ObserveRun, RunScope, StartRun, WorkflowCommand, WorkflowSource},
    store::WorkflowStore,
    DurableWorkflowRuntime,
};

pub struct MentionWorkflowTool<S: WorkflowStore> {
    runtime: Arc<DurableWorkflowRuntime<S>>,
    compiler: MentionWorkflowCompiler,
    manifest: MentionManifest,
    caller_permission: PermissionConfig,
    scope: RunScope,
    parent_cancel: CancellationToken,
    description: String,
}

impl<S: WorkflowStore> MentionWorkflowTool<S> {
    pub fn new(
        runtime: Arc<DurableWorkflowRuntime<S>>,
        compiler: MentionWorkflowCompiler,
        manifest: MentionManifest,
        caller_permission: PermissionConfig,
        scope: RunScope,
        parent_cancel: CancellationToken,
        description: String,
    ) -> Self {
        Self {
            runtime,
            compiler,
            manifest,
            caller_permission,
            scope,
            parent_cancel,
            description,
        }
    }
}

#[async_trait]
impl<S: WorkflowStore> Tool for MentionWorkflowTool<S> {
    fn name(&self) -> &str {
        "run_mentioned_agents"
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
                        "required": ["key", "agent_id", "instruction"],
                        "properties": {
                            "key": { "type": "string" },
                            "agent_id": { "type": "string" },
                            "instruction": { "type": "string" },
                            "depends_on": {
                                "type": "array",
                                "items": { "type": "string" },
                                "default": []
                            },
                            "inputs": {
                                "type": "object",
                                "description": "Explicit ValueExpr bindings. Use {\"source\":\"node_output\",\"node\":\"task-key\",\"pointer\":\"/summary\"} for upstream handoff data.",
                                "additionalProperties": true,
                                "default": {}
                            }
                        }
                    }
                },
                "result": {
                    "type": "object",
                    "description": "A ValueExpr selecting the final workflow result.",
                    "additionalProperties": true
                },
                "policy": {
                    "type": "object",
                    "additionalProperties": true
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let plan: MentionPlan = serde_json::from_value(args.clone())
            .map_err(|error| anyhow::anyhow!("invalid mentioned-agent workflow plan: {error}"))?;
        let spec = self
            .compiler
            .compile(&self.manifest, plan, &self.caller_permission)?;
        let encoded = serde_json::to_vec(&args)?;
        let call_hash = hex::encode(Sha256::digest(encoded));
        let request_id = derive_mention_request_id(&self.scope.parent_prompt_id, &call_hash);
        let receipt = self
            .runtime
            .start(StartRun {
                request_id,
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
