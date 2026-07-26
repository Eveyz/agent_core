use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{permission::PermissionConfig, tools::Tool};

use super::{
    DurableWorkflowRuntime,
    authoring::{
        ApplyWorkflowDraft, PublishWorkflowDraft, WorkflowAuthoringService, WorkflowDraftSpec,
    },
    engine::WorkflowRuntime,
    model::{ObserveRun, RunScope, StartRun, WorkflowCommand, WorkflowSource},
    store::WorkflowStore,
};

pub fn workflow_authoring_tool_factory<S: WorkflowStore>(
    service: Arc<WorkflowAuthoringService>,
    runtime: Arc<DurableWorkflowRuntime<S>>,
    caller_permission: PermissionConfig,
    scope: RunScope,
) -> crate::runtime::run::ScopedToolFactory {
    Arc::new(move |registry, cancel_token, parent_run_id| {
        let available_tools = registry
            .list_names()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut bound_scope = scope.clone();
        bound_scope.parent_run_id = parent_run_id.clone();
        if bound_scope.session_id.is_empty() {
            bound_scope.session_id = parent_run_id.clone();
        }
        registry.register(Box::new(WorkflowCatalogTool::new(
            service.clone(),
            available_tools,
        )));
        registry.register(Box::new(WorkflowApplyDraftTool::new(
            service.clone(),
            caller_permission.clone(),
            parent_run_id.clone(),
        )));
        registry.register(Box::new(WorkflowPreviewTool::<S>::new(
            service.clone(),
            runtime.clone(),
            bound_scope,
            cancel_token,
            parent_run_id.clone(),
        )));
        registry.register(Box::new(WorkflowPublishTool::new(
            service.clone(),
            parent_run_id,
        )));
        Some("workflow_catalog".to_string())
    })
}

pub struct WorkflowCatalogTool {
    service: Arc<WorkflowAuthoringService>,
    available_tools: Vec<String>,
}

impl WorkflowCatalogTool {
    pub fn new(service: Arc<WorkflowAuthoringService>, mut available_tools: Vec<String>) -> Self {
        available_tools.sort();
        available_tools.dedup();
        Self {
            service,
            available_tools,
        }
    }
}

#[async_trait]
impl Tool for WorkflowCatalogTool {
    fn name(&self) -> &str {
        "workflow_catalog"
    }

    fn description(&self) -> &str {
        "Inspect saved custom agents, existing workflow drafts, available tools, and the \
         workflow constructs supported by this runtime. Pass draft_id to load a complete draft \
         before revising it. Call this before designing or updating a workflow."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "draft_id": {
                    "type": "string",
                    "description": "Optional draft to load in full for revision."
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let selected_draft = args
            .get("draft_id")
            .and_then(Value::as_str)
            .map(|draft_id| self.service.get_draft(draft_id))
            .transpose()?
            .map(|record| {
                json!({
                    "receipt": record.receipt,
                    "workflow": record.workflow
                })
            });
        let agents = self
            .service
            .list_agents()?
            .into_iter()
            .map(|agent| {
                json!({
                    "id": agent.id,
                    "revision_id": agent.updated_at,
                    "name": agent.name,
                    "description": agent.description,
                    "model": agent.model,
                    "skills": agent.skills,
                    "tools": agent.tools,
                    "permission_mode": agent.permission_mode,
                    "memory_enabled": agent.memory_enabled,
                })
            })
            .collect::<Vec<_>>();
        Ok(serde_json::to_string(&json!({
            "schema": "workflow.authoring_catalog@1",
            "agents": agents,
            "drafts": self.service.list_drafts()?,
            "selected_draft": selected_draft,
            "published_revisions": self.service.list_revisions()?,
            "available_tools": self.available_tools,
            "supported": {
                "node_kinds": ["custom_agent"],
                "agent_bindings": ["saved", "inline"],
                "value_sources": ["literal", "run_input", "node_output", "object", "array"],
                "failure_policies": ["abort", "continue"],
                "preview": true,
                "publish": true,
                "unsupported": ["for_each", "child_workflow", "dynamic_agent_generation"]
            },
            "defaults": {
                "inline_agent_memory": "stateless",
                "max_concurrency": 3,
                "failure_policy": "abort"
            }
        }))?)
    }
}

#[derive(Deserialize)]
struct ApplyArgs {
    #[serde(default)]
    draft_id: Option<String>,
    #[serde(default)]
    expected_version: Option<u64>,
    workflow: WorkflowDraftSpec,
}

pub struct WorkflowApplyDraftTool {
    service: Arc<WorkflowAuthoringService>,
    caller_permission: PermissionConfig,
    parent_run_id: String,
}

impl WorkflowApplyDraftTool {
    pub fn new(
        service: Arc<WorkflowAuthoringService>,
        caller_permission: PermissionConfig,
        parent_run_id: String,
    ) -> Self {
        Self {
            service,
            caller_permission,
            parent_run_id,
        }
    }
}

#[async_trait]
impl Tool for WorkflowApplyDraftTool {
    fn name(&self) -> &str {
        "workflow_apply_draft"
    }

    fn description(&self) -> &str {
        "Atomically create or update a durable workflow draft. Supply the complete graph in one \
         call. Use saved bindings for catalog agents and inline bindings for workflow-local, \
         stateless agents. Dependencies and upstream data bindings must be explicit. This tool \
         validates and compiles the graph before persisting anything."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::from_str(
            r#"{
              "type":"object",
              "additionalProperties":false,
              "required":["workflow"],
              "properties":{
                "draft_id":{"type":"string","description":"Existing draft ID. Omit to create."},
                "expected_version":{"type":"integer","minimum":1,"description":"Required with draft_id."},
                "workflow":{
                  "type":"object",
                  "additionalProperties":false,
                  "required":["name","steps","result"],
                  "properties":{
                    "name":{"type":"string","minLength":1},
                    "description":{"type":"string","default":""},
                    "input_schema":{"type":"object","additionalProperties":true,"default":{}},
                    "steps":{
                      "type":"array",
                      "minItems":1,
                      "items":{
                        "type":"object",
                        "additionalProperties":false,
                        "required":["key","agent","instruction"],
                        "properties":{
                          "key":{"type":"string","minLength":1},
                          "agent":{
                            "oneOf":[
                              {
                                "type":"object",
                                "additionalProperties":false,
                                "required":["type","agent_id"],
                                "properties":{
                                  "type":{"const":"saved"},
                                  "agent_id":{"type":"string"},
                                  "revision_id":{"type":"string","default":""}
                                }
                              },
                              {
                                "type":"object",
                                "additionalProperties":false,
                                "required":["type","blueprint"],
                                "properties":{
                                  "type":{"const":"inline"},
                                  "blueprint":{
                                    "type":"object",
                                    "additionalProperties":false,
                                    "required":["name","system_prompt"],
                                    "properties":{
                                      "name":{"type":"string"},
                                      "description":{"type":"string","default":""},
                                      "system_prompt":{"type":"string"},
                                      "model":{"type":"string","default":""},
                                      "skills":{"type":"array","items":{"type":"string"},"default":[]},
                                      "tools":{"type":"array","items":{"type":"string"},"default":[]},
                                      "permission_mode":{"type":"string","enum":["paranoid","standard","developer","permissive","yolo"],"default":"standard"},
                                      "max_iterations":{"type":"integer","minimum":1,"default":50},
                                      "max_context_tokens":{"type":"integer","minimum":1,"default":32000},
                                      "icon":{"type":"string","default":""},
                                      "color":{"type":"string","default":""}
                                    }
                                  }
                                }
                              }
                            ]
                          },
                          "instruction":{"type":"string","minLength":1},
                          "inputs":{"type":"object","description":"Input names to ValueExpr objects.","additionalProperties":true,"default":{}},
                          "after":{"type":"array","items":{"type":"string"},"default":[]},
                          "retry":{
                            "type":"object",
                            "additionalProperties":false,
                            "properties":{
                              "max_attempts":{"type":"integer","minimum":1,"default":1},
                              "backoff_ms":{"type":"integer","minimum":0,"default":0}
                            }
                          },
                          "timeout_ms":{"type":["integer","null"],"minimum":1}
                        }
                      }
                    },
                    "result":{"type":"object","description":"ValueExpr selecting the result.","additionalProperties":true},
                    "policy":{"type":"object","additionalProperties":true}
                  }
                }
              }
            }"#,
        )
        .expect("workflow draft schema must be valid JSON")
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let request_id = request_id("apply", &self.parent_run_id, &args)?;
        let args: ApplyArgs = serde_json::from_value(args)
            .map_err(|error| anyhow::anyhow!("invalid workflow draft: {error}"))?;
        let receipt = self.service.apply_draft(
            ApplyWorkflowDraft {
                request_id,
                draft_id: args.draft_id,
                expected_version: args.expected_version,
                workflow: args.workflow,
            },
            &self.caller_permission,
        )?;
        Ok(serde_json::to_string(&json!({
            "draft": receipt,
            "next_actions": [
                "Use workflow_preview to test this exact draft version.",
                "Use workflow_publish only after the user approves publishing."
            ]
        }))?)
    }
}

#[derive(Deserialize)]
struct PublishArgs {
    draft_id: String,
    expected_version: u64,
}

pub struct WorkflowPublishTool {
    service: Arc<WorkflowAuthoringService>,
    parent_run_id: String,
}

impl WorkflowPublishTool {
    pub fn new(service: Arc<WorkflowAuthoringService>, parent_run_id: String) -> Self {
        Self {
            service,
            parent_run_id,
        }
    }
}

#[async_trait]
impl Tool for WorkflowPublishTool {
    fn name(&self) -> &str {
        "workflow_publish"
    }

    fn description(&self) -> &str {
        "Publish a validated workflow draft as a new immutable runtime revision. Call only when \
         the user explicitly asks to publish or confirms the presented draft."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["draft_id", "expected_version"],
            "properties": {
                "draft_id": {"type": "string"},
                "expected_version": {"type": "integer", "minimum": 1}
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let request_id = request_id("publish", &self.parent_run_id, &args)?;
        let args: PublishArgs = serde_json::from_value(args)
            .map_err(|error| anyhow::anyhow!("invalid workflow publish request: {error}"))?;
        Ok(serde_json::to_string(&self.service.publish(
            PublishWorkflowDraft {
                request_id,
                draft_id: args.draft_id,
                expected_version: args.expected_version,
            },
        )?)?)
    }
}

#[derive(Deserialize)]
struct PreviewArgs {
    draft_id: String,
    expected_version: u64,
    #[serde(default = "empty_object")]
    input: Value,
}

fn empty_object() -> Value {
    json!({})
}

pub struct WorkflowPreviewTool<S: WorkflowStore> {
    service: Arc<WorkflowAuthoringService>,
    runtime: Arc<DurableWorkflowRuntime<S>>,
    scope: RunScope,
    parent_cancel: CancellationToken,
    parent_run_id: String,
}

impl<S: WorkflowStore> WorkflowPreviewTool<S> {
    pub fn new(
        service: Arc<WorkflowAuthoringService>,
        runtime: Arc<DurableWorkflowRuntime<S>>,
        scope: RunScope,
        parent_cancel: CancellationToken,
        parent_run_id: String,
    ) -> Self {
        Self {
            service,
            runtime,
            scope,
            parent_cancel,
            parent_run_id,
        }
    }
}

#[async_trait]
impl<S: WorkflowStore> Tool for WorkflowPreviewTool<S> {
    fn name(&self) -> &str {
        "workflow_preview"
    }

    fn description(&self) -> &str {
        "Run a specific durable workflow draft version without publishing it. Returns only after \
         the preview reaches a terminal state."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["draft_id", "expected_version"],
            "properties": {
                "draft_id": {"type": "string"},
                "expected_version": {"type": "integer", "minimum": 1},
                "input": {
                    "description": "Workflow run input matching the draft's declared input schema.",
                    "default": {}
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let request_id = request_id("preview", &self.parent_run_id, &args)?;
        let args: PreviewArgs = serde_json::from_value(args)
            .map_err(|error| anyhow::anyhow!("invalid workflow preview request: {error}"))?;
        let draft = self.service.get_draft(&args.draft_id)?;
        if draft.receipt.version != args.expected_version {
            bail!(
                "workflow draft version conflict: expected {}, actual {}",
                args.expected_version,
                draft.receipt.version
            );
        }
        let input_validator = jsonschema::validator_for(&draft.workflow.input_schema)
            .context("invalid workflow input_schema")?;
        if let Err(error) = input_validator.validate(&args.input) {
            bail!("workflow preview input does not match input_schema: {error}");
        }
        let receipt = self
            .runtime
            .start(StartRun {
                request_id,
                source: WorkflowSource::Inline(draft.compiled),
                input: args.input,
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
                            command_id: format!("authoring-parent-cancel:{}", self.parent_run_id),
                            reason: "workflow authoring run was cancelled".to_string(),
                        },
                    ).await;
                    bail!("workflow authoring run was cancelled");
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            }
        }
    }
}

fn request_id(operation: &str, parent_run_id: &str, args: &Value) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(b"workflow-authoring@1\0");
    digest.update(operation.as_bytes());
    digest.update(b"\0");
    digest.update(parent_run_id.as_bytes());
    digest.update(b"\0");
    digest.update(serde_json::to_vec(args)?);
    Ok(format!("authoring:{}", hex::encode(digest.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        memory::storage::Storage,
        workflow::runtime::{InMemoryWorkflowStore, WorkflowAuthoringService},
    };

    #[tokio::test]
    async fn apply_tool_creates_an_inline_agent_draft_atomically() {
        let directory = tempfile::tempdir().expect("tempdir");
        let storage =
            Storage::new(directory.path().join("db.sqlite").to_str().expect("path")).expect("db");
        let service = Arc::new(
            WorkflowAuthoringService::new(storage, Arc::new(InMemoryWorkflowStore::default()))
                .expect("service"),
        );
        let tool = WorkflowApplyDraftTool::new(
            service.clone(),
            PermissionConfig::default(),
            "parent-run".to_string(),
        );
        let args = json!({
            "workflow": {
                "name": "Research",
                "input_schema": {"type": "object"},
                "steps": [{
                    "key": "research",
                    "agent": {
                        "type": "inline",
                        "blueprint": {
                            "name": "Researcher",
                            "system_prompt": "Research the assigned topic.",
                            "tools": ["web_fetch"]
                        }
                    },
                    "instruction": "Research the topic.",
                    "inputs": {
                        "topic": {"source": "run_input", "pointer": "/topic"}
                    }
                }],
                "result": {
                    "source": "node_output",
                    "node": "research",
                    "pointer": ""
                }
            }
        });
        jsonschema::validator_for(&tool.parameters_schema())
            .expect("schema")
            .validate(&args)
            .expect("valid args");
        let output: Value =
            serde_json::from_str(&tool.execute(args).await.expect("execute")).expect("tool output");
        assert_eq!(output["draft"]["version"], 1);
        assert_eq!(service.list_drafts().expect("list").len(), 1);
    }
}
