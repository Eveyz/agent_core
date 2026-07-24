use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    agent_registry::{self, AgentDef},
    memory::storage::Storage,
    permission::PermissionConfig,
};

use super::{
    custom_agent::{CUSTOM_AGENT_ACTIVITY_KIND, FrozenCustomAgentConfig},
    legacy::classify_agent_effect,
    mention::intersect_permission_ceiling,
    model::{
        EffectPolicy, NodeKey, NodeKind, NodeSpec, ResourceClaim, RetryPolicy, ValueExpr,
        WorkflowPolicy, WorkflowRevisionId, WorkflowSpec,
    },
    reducer::validate_spec,
    store::WorkflowStore,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDraftSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_input_schema")]
    pub input_schema: Value,
    pub steps: Vec<DraftStep>,
    pub result: ValueExpr,
    #[serde(default)]
    pub policy: WorkflowPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftStep {
    pub key: NodeKey,
    pub agent: AgentBinding,
    pub instruction: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, ValueExpr>,
    #[serde(default)]
    pub after: Vec<NodeKey>,
    #[serde(default)]
    pub retry: RetryPolicy,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentBinding {
    Saved {
        agent_id: String,
        #[serde(default)]
        revision_id: String,
    },
    Inline {
        blueprint: InlineAgentBlueprint,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineAgentBlueprint {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub system_prompt: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub color: String,
}

fn default_permission_mode() -> String {
    "standard".to_string()
}

fn default_input_schema() -> Value {
    Value::Object(serde_json::Map::new())
}

fn default_max_iterations() -> usize {
    50
}

fn default_max_context_tokens() -> usize {
    32_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyWorkflowDraft {
    pub request_id: String,
    #[serde(default)]
    pub draft_id: Option<String>,
    #[serde(default)]
    pub expected_version: Option<u64>,
    pub workflow: WorkflowDraftSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDraftReceipt {
    pub workflow_id: String,
    pub draft_id: String,
    pub version: u64,
    pub status: String,
    pub program_hash: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDraftRecord {
    pub receipt: WorkflowDraftReceipt,
    pub workflow: WorkflowDraftSpec,
    pub compiled: WorkflowSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishWorkflowDraft {
    pub request_id: String,
    pub draft_id: String,
    pub expected_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishedWorkflowReceipt {
    pub workflow_id: String,
    pub draft_id: String,
    pub draft_version: u64,
    pub revision_id: WorkflowRevisionId,
    pub revision_number: u64,
    pub program_hash: String,
    pub published_at: String,
}

#[derive(Clone)]
pub struct WorkflowAuthoringService {
    storage: Storage,
    revision_store: Arc<dyn WorkflowStore>,
}

impl WorkflowAuthoringService {
    pub fn new(storage: Storage, revision_store: Arc<dyn WorkflowStore>) -> Result<Self> {
        {
            let conn = storage.conn();
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS workflow_authoring_definitions (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    description TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS workflow_authoring_drafts (
                    id TEXT PRIMARY KEY,
                    workflow_id TEXT NOT NULL,
                    version INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    draft_spec TEXT NOT NULL,
                    compiled_spec TEXT NOT NULL,
                    program_hash TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    FOREIGN KEY (workflow_id) REFERENCES workflow_authoring_definitions(id)
                        ON DELETE RESTRICT
                );

                CREATE INDEX IF NOT EXISTS idx_workflow_authoring_drafts_workflow
                    ON workflow_authoring_drafts(workflow_id);

                CREATE TABLE IF NOT EXISTS workflow_authoring_revisions (
                    revision_id TEXT PRIMARY KEY,
                    workflow_id TEXT NOT NULL,
                    revision_number INTEGER NOT NULL,
                    draft_id TEXT NOT NULL,
                    draft_version INTEGER NOT NULL,
                    program_hash TEXT NOT NULL,
                    published_at TEXT NOT NULL,
                    UNIQUE(workflow_id, revision_number),
                    FOREIGN KEY (workflow_id) REFERENCES workflow_authoring_definitions(id)
                        ON DELETE RESTRICT,
                    FOREIGN KEY (draft_id) REFERENCES workflow_authoring_drafts(id)
                        ON DELETE RESTRICT
                );

                CREATE TABLE IF NOT EXISTS workflow_authoring_requests (
                    request_id TEXT PRIMARY KEY,
                    operation TEXT NOT NULL,
                    response TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );
                "#,
            )
            .context("failed to initialize workflow authoring tables")?;
        }
        Ok(Self {
            storage,
            revision_store,
        })
    }

    pub fn list_agents(&self) -> Result<Vec<AgentDef>> {
        agent_registry::list(&self.storage)
    }

    pub fn list_drafts(&self) -> Result<Vec<WorkflowDraftReceipt>> {
        let conn = self.storage.conn();
        let mut statement = conn.prepare(
            r#"
            SELECT workflow_id, id, version, status, program_hash, updated_at
            FROM workflow_authoring_drafts
            ORDER BY updated_at DESC
            "#,
        )?;
        let rows = statement.query_map([], |row| {
            Ok(WorkflowDraftReceipt {
                workflow_id: row.get(0)?,
                draft_id: row.get(1)?,
                version: row.get::<_, i64>(2)? as u64,
                status: row.get(3)?,
                program_hash: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_revisions(&self) -> Result<Vec<PublishedWorkflowReceipt>> {
        let conn = self.storage.conn();
        let mut statement = conn.prepare(
            r#"
            SELECT workflow_id, draft_id, draft_version, revision_id,
                   revision_number, program_hash, published_at
            FROM workflow_authoring_revisions
            ORDER BY published_at DESC
            "#,
        )?;
        let rows = statement.query_map([], |row| {
            Ok(PublishedWorkflowReceipt {
                workflow_id: row.get(0)?,
                draft_id: row.get(1)?,
                draft_version: row.get::<_, i64>(2)? as u64,
                revision_id: WorkflowRevisionId(row.get(3)?),
                revision_number: row.get::<_, i64>(4)? as u64,
                program_hash: row.get(5)?,
                published_at: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn apply_draft(
        &self,
        request: ApplyWorkflowDraft,
        caller_permission: &PermissionConfig,
    ) -> Result<WorkflowDraftReceipt> {
        ensure_request_id(&request.request_id)?;
        if let Some(receipt) = self.load_idempotent_response(&request.request_id, "apply_draft")? {
            return serde_json::from_str(&receipt).context("invalid stored draft receipt");
        }
        let compiled = self.compile(&request.workflow, caller_permission)?;
        let compiled_json = serde_json::to_string(&compiled)?;
        let program_hash = hash_json(&compiled)?;
        let draft_json = serde_json::to_string(&request.workflow)?;
        let now = Utc::now().to_rfc3339();
        let mut conn = self.storage.conn();
        let transaction = conn.transaction()?;

        let (workflow_id, draft_id, version) = if let Some(draft_id) = request.draft_id {
            let existing = transaction
                .query_row(
                    "SELECT workflow_id, version FROM workflow_authoring_drafts WHERE id = ?1",
                    params![draft_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)),
                )
                .optional()?
                .ok_or_else(|| anyhow::anyhow!("workflow draft not found: {draft_id}"))?;
            let expected = request.expected_version.ok_or_else(|| {
                anyhow::anyhow!("expected_version is required when updating a workflow draft")
            })?;
            if existing.1 != expected {
                bail!(
                    "workflow draft version conflict: expected {}, actual {}",
                    expected,
                    existing.1
                );
            }
            let next_version = existing.1 + 1;
            transaction.execute(
                r#"
                UPDATE workflow_authoring_drafts
                SET version = ?2, status = 'valid', draft_spec = ?3,
                    compiled_spec = ?4, program_hash = ?5, updated_at = ?6
                WHERE id = ?1
                "#,
                params![
                    draft_id,
                    next_version as i64,
                    draft_json,
                    compiled_json,
                    program_hash,
                    now,
                ],
            )?;
            transaction.execute(
                r#"
                UPDATE workflow_authoring_definitions
                SET name = ?2, description = ?3, updated_at = ?4
                WHERE id = ?1
                "#,
                params![
                    existing.0,
                    request.workflow.name,
                    request.workflow.description,
                    now,
                ],
            )?;
            (existing.0, draft_id, next_version)
        } else {
            if request.expected_version.is_some() {
                bail!("expected_version must be omitted when creating a workflow draft");
            }
            let workflow_id = uuid::Uuid::new_v4().to_string();
            let draft_id = uuid::Uuid::new_v4().to_string();
            transaction.execute(
                r#"
                INSERT INTO workflow_authoring_definitions
                    (id, name, description, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?4)
                "#,
                params![
                    workflow_id,
                    request.workflow.name,
                    request.workflow.description,
                    now,
                ],
            )?;
            transaction.execute(
                r#"
                INSERT INTO workflow_authoring_drafts
                    (id, workflow_id, version, status, draft_spec, compiled_spec,
                     program_hash, created_at, updated_at)
                VALUES (?1, ?2, 1, 'valid', ?3, ?4, ?5, ?6, ?6)
                "#,
                params![
                    draft_id,
                    workflow_id,
                    draft_json,
                    compiled_json,
                    program_hash,
                    now,
                ],
            )?;
            (workflow_id, draft_id, 1)
        };

        let receipt = WorkflowDraftReceipt {
            workflow_id,
            draft_id,
            version,
            status: "valid".to_string(),
            program_hash,
            updated_at: now.clone(),
        };
        transaction.execute(
            r#"
            INSERT INTO workflow_authoring_requests
                (request_id, operation, response, created_at)
            VALUES (?1, 'apply_draft', ?2, ?3)
            "#,
            params![request.request_id, serde_json::to_string(&receipt)?, now],
        )?;
        transaction.commit()?;
        Ok(receipt)
    }

    pub fn get_draft(&self, draft_id: &str) -> Result<WorkflowDraftRecord> {
        let conn = self.storage.conn();
        let row = conn
            .query_row(
                r#"
                SELECT workflow_id, id, version, status, program_hash, updated_at,
                       draft_spec, compiled_spec
                FROM workflow_authoring_drafts
                WHERE id = ?1
                "#,
                params![draft_id],
                |row| {
                    Ok((
                        WorkflowDraftReceipt {
                            workflow_id: row.get(0)?,
                            draft_id: row.get(1)?,
                            version: row.get::<_, i64>(2)? as u64,
                            status: row.get(3)?,
                            program_hash: row.get(4)?,
                            updated_at: row.get(5)?,
                        },
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("workflow draft not found: {draft_id}"))?;
        Ok(WorkflowDraftRecord {
            receipt: row.0,
            workflow: serde_json::from_str(&row.1).context("invalid stored workflow draft")?,
            compiled: serde_json::from_str(&row.2)
                .context("invalid stored compiled workflow draft")?,
        })
    }

    pub fn publish(&self, request: PublishWorkflowDraft) -> Result<PublishedWorkflowReceipt> {
        ensure_request_id(&request.request_id)?;
        if let Some(receipt) = self.load_idempotent_response(&request.request_id, "publish")? {
            return serde_json::from_str(&receipt).context("invalid stored publish receipt");
        }
        let draft = self.get_draft(&request.draft_id)?;
        if draft.receipt.version != request.expected_version {
            bail!(
                "workflow draft version conflict: expected {}, actual {}",
                request.expected_version,
                draft.receipt.version
            );
        }
        validate_spec(&draft.compiled)?;
        let conn = self.storage.conn();
        let revision_number = conn.query_row(
            r#"
            SELECT COALESCE(MAX(revision_number), 0) + 1
            FROM workflow_authoring_revisions
            WHERE workflow_id = ?1
            "#,
            params![draft.receipt.workflow_id],
            |row| row.get::<_, i64>(0),
        )? as u64;
        drop(conn);
        let revision_id = WorkflowRevisionId(format!(
            "{}:r{}:{}",
            draft.receipt.workflow_id,
            revision_number,
            &draft.receipt.program_hash[..16]
        ));
        self.revision_store
            .publish_revision(&revision_id, &draft.compiled)?;

        let published_at = Utc::now().to_rfc3339();
        let receipt = PublishedWorkflowReceipt {
            workflow_id: draft.receipt.workflow_id.clone(),
            draft_id: draft.receipt.draft_id.clone(),
            draft_version: draft.receipt.version,
            revision_id,
            revision_number,
            program_hash: draft.receipt.program_hash.clone(),
            published_at: published_at.clone(),
        };
        let mut conn = self.storage.conn();
        let transaction = conn.transaction()?;
        transaction.execute(
            r#"
            INSERT INTO workflow_authoring_revisions
                (revision_id, workflow_id, revision_number, draft_id, draft_version,
                 program_hash, published_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                receipt.revision_id.0,
                receipt.workflow_id,
                receipt.revision_number as i64,
                receipt.draft_id,
                receipt.draft_version as i64,
                receipt.program_hash,
                receipt.published_at,
            ],
        )?;
        transaction.execute(
            "UPDATE workflow_authoring_drafts SET status = 'published' WHERE id = ?1",
            params![request.draft_id],
        )?;
        transaction.execute(
            r#"
            INSERT INTO workflow_authoring_requests
                (request_id, operation, response, created_at)
            VALUES (?1, 'publish', ?2, ?3)
            "#,
            params![
                request.request_id,
                serde_json::to_string(&receipt)?,
                published_at
            ],
        )?;
        transaction.commit()?;
        Ok(receipt)
    }

    pub fn compile(
        &self,
        draft: &WorkflowDraftSpec,
        caller_permission: &PermissionConfig,
    ) -> Result<WorkflowSpec> {
        if draft.name.trim().is_empty() {
            bail!("workflow name must not be empty");
        }
        if draft.steps.is_empty() {
            bail!("workflow must contain at least one agent step");
        }
        jsonschema::validator_for(&draft.input_schema).context("invalid workflow input_schema")?;
        if draft.policy.max_concurrency == 0 {
            bail!("workflow max_concurrency must be greater than zero");
        }
        let mut nodes = Vec::with_capacity(draft.steps.len());
        for step in &draft.steps {
            if step.key.0.trim().is_empty() {
                bail!("workflow step key must not be empty");
            }
            if step.instruction.trim().is_empty() {
                bail!(
                    "workflow step '{}' instruction must not be empty",
                    step.key.0
                );
            }
            if step.retry.max_attempts == 0 {
                bail!(
                    "workflow step '{}' max_attempts must be greater than zero",
                    step.key.0
                );
            }
            if step.timeout_ms == Some(0) {
                bail!(
                    "workflow step '{}' timeout_ms must be greater than zero",
                    step.key.0
                );
            }
            let (agent, record_history) = match &step.agent {
                AgentBinding::Saved {
                    agent_id,
                    revision_id,
                } => {
                    let agent = agent_registry::get(&self.storage, agent_id)
                        .with_context(|| format!("resolve workflow agent '{agent_id}'"))?;
                    if !revision_id.is_empty() && revision_id != &agent.updated_at {
                        bail!(
                            "workflow agent '{}' changed after selection; refresh the catalog",
                            agent_id
                        );
                    }
                    (agent, true)
                }
                AgentBinding::Inline { blueprint } => (inline_agent(blueprint)?, false),
            };
            let requested = agent_registry::build_permission_config(&agent, caller_permission);
            let permission = intersect_permission_ceiling(caller_permission, requested);
            let effect = classify_agent_effect(&agent.tools, &permission);
            let mut inputs = step.inputs.clone();
            if inputs
                .insert(
                    "instruction".to_string(),
                    ValueExpr::Literal {
                        value: Value::String(step.instruction.clone()),
                    },
                )
                .is_some()
            {
                bail!(
                    "workflow step '{}' may not override reserved input 'instruction'",
                    step.key.0
                );
            }
            nodes.push(NodeSpec {
                key: step.key.clone(),
                kind: NodeKind::Activity {
                    kind: CUSTOM_AGENT_ACTIVITY_KIND.to_string(),
                    config: serde_json::to_value(FrozenCustomAgentConfig {
                        agent,
                        permission,
                        record_history,
                    })?,
                },
                inputs,
                after: step.after.clone(),
                retry: step.retry.clone(),
                timeout_ms: step.timeout_ms,
                effect,
                resources: if effect == EffectPolicy::WorkspaceWrite {
                    vec![ResourceClaim {
                        resource: "workspace".to_string(),
                        exclusive: true,
                    }]
                } else {
                    Vec::new()
                },
            });
        }
        let spec = WorkflowSpec {
            schema_version: 1,
            nodes,
            result: draft.result.clone(),
            policy: draft.policy.clone(),
        };
        validate_spec(&spec)?;
        Ok(spec)
    }

    fn load_idempotent_response(
        &self,
        request_id: &str,
        operation: &str,
    ) -> Result<Option<String>> {
        let conn = self.storage.conn();
        let response = conn
            .query_row(
                r#"
                SELECT operation, response
                FROM workflow_authoring_requests
                WHERE request_id = ?1
                "#,
                params![request_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        match response {
            Some((stored_operation, response)) if stored_operation == operation => {
                Ok(Some(response))
            }
            Some((stored_operation, _)) => bail!(
                "workflow authoring request '{}' was already used for '{}'",
                request_id,
                stored_operation
            ),
            None => Ok(None),
        }
    }
}

fn ensure_request_id(request_id: &str) -> Result<()> {
    if request_id.trim().is_empty() {
        bail!("workflow authoring request_id must not be empty");
    }
    Ok(())
}

fn hash_json(value: &impl Serialize) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

fn inline_agent(blueprint: &InlineAgentBlueprint) -> Result<AgentDef> {
    if blueprint.name.trim().is_empty() {
        bail!("inline agent name must not be empty");
    }
    if blueprint.system_prompt.trim().is_empty() {
        bail!("inline agent system_prompt must not be empty");
    }
    if blueprint.max_iterations == 0 {
        bail!("inline agent max_iterations must be greater than zero");
    }
    if blueprint.max_context_tokens == 0 {
        bail!("inline agent max_context_tokens must be greater than zero");
    }
    if agent_registry::definition::parse_permission_mode(&blueprint.permission_mode).is_none() {
        bail!(
            "inline agent permission_mode '{}' is invalid",
            blueprint.permission_mode
        );
    }
    let hash = hash_json(blueprint)?;
    let identity = format!("inline:{}", &hash[..24]);
    Ok(AgentDef {
        id: identity.clone(),
        name: blueprint.name.clone(),
        description: blueprint.description.clone(),
        system_prompt: blueprint.system_prompt.clone(),
        model: blueprint.model.clone(),
        skills: blueprint.skills.clone(),
        tools: blueprint.tools.clone(),
        permission_mode: blueprint.permission_mode.clone(),
        permission_rules: Value::Array(Vec::new()),
        max_iterations: blueprint.max_iterations,
        max_context_tokens: blueprint.max_context_tokens,
        memory_enabled: 0,
        memory_group: String::new(),
        icon: blueprint.icon.clone(),
        color: blueprint.color.clone(),
        created_at: identity.clone(),
        updated_at: identity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        permission::PermissionMode,
        workflow::runtime::{InMemoryWorkflowStore, NodeKind},
    };
    use serde_json::json;

    fn service() -> (
        tempfile::TempDir,
        WorkflowAuthoringService,
        Arc<InMemoryWorkflowStore>,
    ) {
        let directory = tempfile::tempdir().expect("tempdir");
        let storage =
            Storage::new(directory.path().join("db.sqlite").to_str().expect("path")).expect("db");
        let store = Arc::new(InMemoryWorkflowStore::default());
        let service =
            WorkflowAuthoringService::new(storage, store.clone()).expect("authoring service");
        (directory, service, store)
    }

    fn inline_draft() -> WorkflowDraftSpec {
        WorkflowDraftSpec {
            name: "Summarize".to_string(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
            steps: vec![DraftStep {
                key: NodeKey::from("summarize"),
                agent: AgentBinding::Inline {
                    blueprint: InlineAgentBlueprint {
                        name: "Summarizer".to_string(),
                        description: String::new(),
                        system_prompt: "Summarize the supplied material.".to_string(),
                        model: String::new(),
                        skills: Vec::new(),
                        tools: vec!["read_file".to_string()],
                        permission_mode: "standard".to_string(),
                        max_iterations: 10,
                        max_context_tokens: 8_000,
                        icon: String::new(),
                        color: String::new(),
                    },
                },
                instruction: "Create the summary.".to_string(),
                inputs: BTreeMap::from([(
                    "material".to_string(),
                    ValueExpr::RunInput {
                        pointer: "/material".to_string(),
                    },
                )]),
                after: Vec::new(),
                retry: RetryPolicy::default(),
                timeout_ms: None,
            }],
            result: ValueExpr::NodeOutput {
                node: NodeKey::from("summarize"),
                pointer: String::new(),
            },
            policy: WorkflowPolicy::default(),
        }
    }

    #[test]
    fn inline_agents_are_frozen_and_stateless() {
        let (_directory, service, _store) = service();
        let mut permission = PermissionConfig::default();
        permission.mode = PermissionMode::Developer;
        let compiled = service
            .compile(&inline_draft(), &permission)
            .expect("compile");
        let NodeKind::Activity { config, .. } = &compiled.nodes[0].kind else {
            panic!("expected activity");
        };
        assert_eq!(config["agent"]["memory_enabled"], 0);
        assert!(
            config["agent"]["id"]
                .as_str()
                .expect("id")
                .starts_with("inline:")
        );
        assert_eq!(config["record_history"], false);
        assert_eq!(compiled.nodes[0].effect, EffectPolicy::ReadOnly);
    }

    #[test]
    fn draft_updates_are_optimistic_and_idempotent() {
        let (_directory, service, _store) = service();
        let permission = PermissionConfig::default();
        let first = service
            .apply_draft(
                ApplyWorkflowDraft {
                    request_id: "create-1".to_string(),
                    draft_id: None,
                    expected_version: None,
                    workflow: inline_draft(),
                },
                &permission,
            )
            .expect("create");
        let replay = service
            .apply_draft(
                ApplyWorkflowDraft {
                    request_id: "create-1".to_string(),
                    draft_id: None,
                    expected_version: None,
                    workflow: inline_draft(),
                },
                &permission,
            )
            .expect("replay");
        assert_eq!(first.draft_id, replay.draft_id);
        assert_eq!(first.version, 1);

        let mut changed = inline_draft();
        changed.description = "Updated".to_string();
        let second = service
            .apply_draft(
                ApplyWorkflowDraft {
                    request_id: "update-1".to_string(),
                    draft_id: Some(first.draft_id.clone()),
                    expected_version: Some(1),
                    workflow: changed,
                },
                &permission,
            )
            .expect("update");
        assert_eq!(second.version, 2);
        assert!(
            service
                .apply_draft(
                    ApplyWorkflowDraft {
                        request_id: "stale".to_string(),
                        draft_id: Some(first.draft_id),
                        expected_version: Some(1),
                        workflow: inline_draft(),
                    },
                    &permission,
                )
                .unwrap_err()
                .to_string()
                .contains("version conflict")
        );
    }

    #[test]
    fn publishing_creates_immutable_runtime_revision() {
        let (_directory, service, store) = service();
        let permission = PermissionConfig::default();
        let draft = service
            .apply_draft(
                ApplyWorkflowDraft {
                    request_id: "create".to_string(),
                    draft_id: None,
                    expected_version: None,
                    workflow: inline_draft(),
                },
                &permission,
            )
            .expect("draft");
        let published = service
            .publish(PublishWorkflowDraft {
                request_id: "publish".to_string(),
                draft_id: draft.draft_id,
                expected_version: draft.version,
            })
            .expect("publish");
        let revision = store
            .load_revision(&published.revision_id)
            .expect("runtime revision");
        assert_eq!(revision.nodes.len(), 1);
        assert_eq!(service.list_revisions().expect("catalog").len(), 1);
    }

    #[test]
    fn invalid_graph_is_not_persisted() {
        let (_directory, service, _store) = service();
        let mut invalid = inline_draft();
        invalid.steps[0].after = vec![NodeKey::from("missing")];
        assert!(
            service
                .apply_draft(
                    ApplyWorkflowDraft {
                        request_id: "invalid".to_string(),
                        draft_id: None,
                        expected_version: None,
                        workflow: invalid,
                    },
                    &PermissionConfig::default(),
                )
                .is_err()
        );
        assert!(service.list_drafts().expect("list").is_empty());
    }
}
