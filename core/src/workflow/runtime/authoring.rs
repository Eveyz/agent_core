use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

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
        EffectPolicy, NodeKey, NodeKind, NodeSpec, ResourceClaim, RetryPolicy, RunScope,
        ValueExpr, WorkflowEventKind, WorkflowPolicy, WorkflowRevisionId, WorkflowSpec,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowScope {
    Session { session_id: String },
    Project { project_id: String, workspace: String },
    User,
}

impl Default for WorkflowScope {
    fn default() -> Self {
        Self::User
    }
}

impl WorkflowScope {
    fn columns(&self) -> (&'static str, String, String, String) {
        match self {
            Self::Session { session_id } => (
                "session",
                session_id.clone(),
                String::new(),
                session_id.clone(),
            ),
            Self::Project {
                project_id,
                workspace,
            } => (
                "project",
                project_id.clone(),
                workspace.clone(),
                String::new(),
            ),
            Self::User => (
                "user",
                String::new(),
                String::new(),
                String::new(),
            ),
        }
    }

    fn from_columns(kind: String, id: String, workspace: String, owner: String) -> Self {
        match kind.as_str() {
            "session" => Self::Session {
                session_id: if owner.is_empty() { id } else { owner },
            },
            "project" => Self::Project {
                project_id: id,
                workspace,
            },
            _ => Self::User,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowLifecycle {
    Transient,
    Draft,
    Published,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowSourceKind {
    Runtime,
    Legacy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDraftReceipt {
    pub workflow_id: String,
    pub draft_id: String,
    pub version: u64,
    pub status: String,
    pub program_hash: String,
    pub updated_at: String,
    #[serde(default)]
    pub scope: WorkflowScope,
    #[serde(default = "default_draft_lifecycle")]
    pub lifecycle: WorkflowLifecycle,
}

fn default_draft_lifecycle() -> WorkflowLifecycle {
    WorkflowLifecycle::Draft
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
pub struct SaveWorkflowDraft {
    pub request_id: String,
    pub draft_id: String,
    pub expected_version: u64,
    pub scope: WorkflowScope,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowLibraryEntry {
    pub workflow_id: String,
    pub name: String,
    pub description: String,
    pub scope: WorkflowScope,
    pub lifecycle: WorkflowLifecycle,
    pub source_kind: WorkflowSourceKind,
    pub draft_id: String,
    pub draft_version: u64,
    pub draft_status: String,
    pub program_hash: String,
    #[serde(default)]
    pub latest_revision: Option<PublishedWorkflowReceipt>,
    pub updated_at: String,
    #[serde(default)]
    pub workflow: Option<WorkflowDraftSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRuntimeRunSummary {
    pub run_id: String,
    pub status: String,
    pub trigger: String,
    pub created_at: String,
    pub updated_at: String,
    pub failed_nodes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectWorkflowPackage {
    schema_version: u32,
    definition: WorkflowLibraryEntry,
    draft: WorkflowDraftSpec,
    #[serde(default)]
    program: Option<WorkflowSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectWorkflowRevision {
    schema_version: u32,
    receipt: PublishedWorkflowReceipt,
    program: WorkflowSpec,
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
                    lifecycle TEXT NOT NULL DEFAULT 'draft',
                    scope_kind TEXT NOT NULL DEFAULT 'user',
                    scope_id TEXT NOT NULL DEFAULT '',
                    workspace TEXT NOT NULL DEFAULT '',
                    owner_session_id TEXT NOT NULL DEFAULT '',
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
            ensure_column(
                &conn,
                "workflow_authoring_definitions",
                "lifecycle",
                "TEXT NOT NULL DEFAULT 'draft'",
            )?;
            ensure_column(
                &conn,
                "workflow_authoring_definitions",
                "scope_kind",
                "TEXT NOT NULL DEFAULT 'user'",
            )?;
            ensure_column(
                &conn,
                "workflow_authoring_definitions",
                "scope_id",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_column(
                &conn,
                "workflow_authoring_definitions",
                "workspace",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_column(
                &conn,
                "workflow_authoring_definitions",
                "owner_session_id",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            conn.execute(
                r#"
                UPDATE workflow_authoring_definitions
                SET lifecycle = 'published'
                WHERE EXISTS (
                    SELECT 1 FROM workflow_authoring_revisions r
                    WHERE r.workflow_id = workflow_authoring_definitions.id
                )
                "#,
                [],
            )?;
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
            SELECT d.workflow_id, d.id, d.version, d.status, d.program_hash, d.updated_at,
                   f.scope_kind, f.scope_id, f.workspace, f.owner_session_id, f.lifecycle
            FROM workflow_authoring_drafts d
            JOIN workflow_authoring_definitions f ON f.id = d.workflow_id
            ORDER BY d.updated_at DESC
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
                scope: WorkflowScope::from_columns(
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ),
                lifecycle: lifecycle_from_str(&row.get::<_, String>(10)?),
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

    pub fn revisions_for_workflow(
        &self,
        workflow_id: &str,
    ) -> Result<Vec<PublishedWorkflowReceipt>> {
        Ok(self
            .list_revisions()?
            .into_iter()
            .filter(|receipt| receipt.workflow_id == workflow_id)
            .collect())
    }

    pub fn runtime_history(
        &self,
        workflow_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkflowRuntimeRunSummary>> {
        let conn = self.storage.conn();
        let mut statement = conn.prepare(
            r#"
            SELECT r.id, r.status, r.scope, r.created_at, r.updated_at
            FROM durable_workflow_runs r
            WHERE r.program_hash IN (
                SELECT program_hash FROM workflow_authoring_revisions WHERE workflow_id = ?1
            )
            ORDER BY r.updated_at DESC
            LIMIT ?2
            "#,
        )?;
        let rows = statement.query_map(params![workflow_id, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            let (run_id, status, scope_json, created_at, updated_at) = row?;
            let scope: RunScope = serde_json::from_str(&scope_json).unwrap_or_default();
            let mut event_statement = conn.prepare(
                "SELECT event FROM durable_workflow_events WHERE run_id = ?1 ORDER BY sequence",
            )?;
            let events =
                event_statement.query_map(params![run_id], |row| row.get::<_, String>(0))?;
            let failed_nodes = events
                .filter_map(|event| event.ok())
                .filter_map(|event| serde_json::from_str::<WorkflowEventKind>(&event).ok())
                .filter_map(|event| match event {
                    WorkflowEventKind::NodeFailed { node, .. }
                    | WorkflowEventKind::NodeNeedsAttention { node, .. } => Some(node.0),
                    _ => None,
                })
                .collect();
            result.push(WorkflowRuntimeRunSummary {
                run_id,
                status,
                trigger: scope.trigger,
                created_at,
                updated_at,
                failed_nodes,
            });
        }
        Ok(result)
    }

    pub fn apply_draft(
        &self,
        request: ApplyWorkflowDraft,
        caller_permission: &PermissionConfig,
    ) -> Result<WorkflowDraftReceipt> {
        self.apply_draft_in_scope(request, caller_permission, WorkflowScope::User)
    }

    pub fn apply_draft_in_scope(
        &self,
        request: ApplyWorkflowDraft,
        caller_permission: &PermissionConfig,
        initial_scope: WorkflowScope,
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

        let (workflow_id, draft_id, version, scope, lifecycle) =
            if let Some(draft_id) = request.draft_id {
            let existing = transaction
                .query_row(
                    r#"
                    SELECT d.workflow_id, d.version, f.scope_kind, f.scope_id,
                           f.workspace, f.owner_session_id, f.lifecycle, d.status
                    FROM workflow_authoring_drafts d
                    JOIN workflow_authoring_definitions f ON f.id = d.workflow_id
                    WHERE d.id = ?1
                    "#,
                    params![draft_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)? as u64,
                            WorkflowScope::from_columns(
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                            ),
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                        ))
                    },
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
            let (draft_id, next_version) = if existing.4 == "published" {
                let derived_draft_id = uuid::Uuid::new_v4().to_string();
                transaction.execute(
                    r#"
                    INSERT INTO workflow_authoring_drafts
                        (id, workflow_id, version, status, draft_spec, compiled_spec,
                         program_hash, created_at, updated_at)
                    VALUES (?1, ?2, 1, 'valid', ?3, ?4, ?5, ?6, ?6)
                    "#,
                    params![
                        derived_draft_id,
                        existing.0,
                        draft_json,
                        compiled_json,
                        program_hash,
                        now,
                    ],
                )?;
                (derived_draft_id, 1)
            } else {
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
                (draft_id, next_version)
            };
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
            (
                existing.0,
                draft_id,
                next_version,
                existing.2,
                lifecycle_from_str(&existing.3),
            )
        } else {
            if request.expected_version.is_some() {
                bail!("expected_version must be omitted when creating a workflow draft");
            }
            let workflow_id = uuid::Uuid::new_v4().to_string();
            let draft_id = uuid::Uuid::new_v4().to_string();
            let (scope_kind, scope_id, workspace, owner_session_id) = initial_scope.columns();
            let lifecycle = if matches!(initial_scope, WorkflowScope::Session { .. }) {
                WorkflowLifecycle::Transient
            } else {
                WorkflowLifecycle::Draft
            };
            transaction.execute(
                r#"
                INSERT INTO workflow_authoring_definitions
                    (id, name, description, lifecycle, scope_kind, scope_id,
                     workspace, owner_session_id, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
                "#,
                params![
                    workflow_id,
                    request.workflow.name,
                    request.workflow.description,
                    lifecycle_as_str(lifecycle),
                    scope_kind,
                    scope_id,
                    workspace,
                    owner_session_id,
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
            (workflow_id, draft_id, 1, initial_scope, lifecycle)
        };

        let receipt = WorkflowDraftReceipt {
            workflow_id,
            draft_id,
            version,
            status: "valid".to_string(),
            program_hash,
            updated_at: now.clone(),
            scope,
            lifecycle,
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
                SELECT d.workflow_id, d.id, d.version, d.status, d.program_hash, d.updated_at,
                       d.draft_spec, d.compiled_spec, f.scope_kind, f.scope_id, f.workspace,
                       f.owner_session_id, f.lifecycle
                FROM workflow_authoring_drafts d
                JOIN workflow_authoring_definitions f ON f.id = d.workflow_id
                WHERE d.id = ?1
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
                            scope: WorkflowScope::from_columns(
                                row.get(8)?,
                                row.get(9)?,
                                row.get(10)?,
                                row.get(11)?,
                            ),
                            lifecycle: lifecycle_from_str(&row.get::<_, String>(12)?),
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

    pub fn save_draft(&self, request: SaveWorkflowDraft) -> Result<WorkflowDraftReceipt> {
        ensure_request_id(&request.request_id)?;
        if let Some(receipt) = self.load_idempotent_response(&request.request_id, "save_draft")? {
            return serde_json::from_str(&receipt).context("invalid stored save receipt");
        }
        if matches!(request.scope, WorkflowScope::Session { .. }) {
            bail!("saving a workflow requires project or user scope");
        }
        let draft = self.get_draft(&request.draft_id)?;
        if draft.receipt.version != request.expected_version {
            bail!(
                "workflow draft version conflict: expected {}, actual {}",
                request.expected_version,
                draft.receipt.version
            );
        }
        let (scope_kind, scope_id, workspace, owner_session_id) = request.scope.columns();
        let now = Utc::now().to_rfc3339();
        let mut conn = self.storage.conn();
        let transaction = conn.transaction()?;
        transaction.execute(
            r#"
            UPDATE workflow_authoring_definitions
            SET lifecycle = 'draft', scope_kind = ?2, scope_id = ?3, workspace = ?4,
                owner_session_id = ?5, updated_at = ?6
            WHERE id = ?1
            "#,
            params![
                draft.receipt.workflow_id,
                scope_kind,
                scope_id,
                workspace,
                owner_session_id,
                now,
            ],
        )?;
        let receipt = WorkflowDraftReceipt {
            scope: request.scope.clone(),
            lifecycle: WorkflowLifecycle::Draft,
            updated_at: now.clone(),
            ..draft.receipt
        };
        transaction.execute(
            r#"
            INSERT INTO workflow_authoring_requests
                (request_id, operation, response, created_at)
            VALUES (?1, 'save_draft', ?2, ?3)
            "#,
            params![
                request.request_id,
                serde_json::to_string(&receipt)?,
                now
            ],
        )?;
        transaction.commit()?;
        drop(conn);
        if matches!(request.scope, WorkflowScope::Project { .. }) {
            self.write_project_package(&receipt.workflow_id)?;
        }
        Ok(receipt)
    }

    pub fn catalog(
        &self,
        project_id: Option<&str>,
        include_workflow: bool,
    ) -> Result<Vec<WorkflowLibraryEntry>> {
        let conn = self.storage.conn();
        let mut statement = conn.prepare(
            r#"
            SELECT f.id, f.name, f.description, f.scope_kind, f.scope_id, f.workspace,
                   f.owner_session_id, f.lifecycle, f.updated_at,
                   d.id, d.version, d.status, d.program_hash, d.draft_spec
            FROM workflow_authoring_definitions f
            JOIN workflow_authoring_drafts d ON d.workflow_id = f.id
            WHERE f.lifecycle != 'transient'
              AND d.id = (
                  SELECT d2.id FROM workflow_authoring_drafts d2
                  WHERE d2.workflow_id = f.id
                  ORDER BY d2.updated_at DESC, d2.rowid DESC
                  LIMIT 1
              )
              AND (f.scope_kind = 'user' OR (?1 IS NOT NULL AND f.scope_kind = 'project'
                   AND f.scope_id = ?1))
            ORDER BY f.updated_at DESC
            "#,
        )?;
        let rows = statement.query_map(params![project_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                WorkflowScope::from_columns(
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ),
                lifecycle_from_str(&row.get::<_, String>(7)?),
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, i64>(10)? as u64,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(conn);
        let revisions = self.list_revisions()?;
        let latest_by_workflow =
            revisions
                .into_iter()
                .fold(BTreeMap::<String, PublishedWorkflowReceipt>::new(), |mut map, receipt| {
                    map.entry(receipt.workflow_id.clone())
                        .or_insert(receipt);
                    map
                });
        let mut entries = Vec::new();
        for row in rows {
            let (
                workflow_id,
                name,
                description,
                scope,
                lifecycle,
                updated_at,
                draft_id,
                draft_version,
                draft_status,
                program_hash,
                draft_json,
            ) = row;
            entries.push(WorkflowLibraryEntry {
                latest_revision: latest_by_workflow.get(&workflow_id).cloned(),
                workflow: if include_workflow {
                    Some(
                        serde_json::from_str(&draft_json)
                            .context("invalid stored workflow draft")?,
                    )
                } else {
                    None
                },
                workflow_id,
                name,
                description,
                scope,
                lifecycle,
                source_kind: WorkflowSourceKind::Runtime,
                draft_id,
                draft_version,
                draft_status,
                program_hash,
                updated_at,
            });
        }
        Ok(entries)
    }

    pub fn sync_project_library(
        &self,
        project_id: &str,
        workspace: &str,
        caller_permission: &PermissionConfig,
    ) -> Result<usize> {
        let root = Path::new(workspace).join(".agverse").join("workflows");
        if !root.exists() {
            return Ok(0);
        }
        let mut imported = 0;
        for child in fs::read_dir(&root)
            .with_context(|| format!("read project workflow library {}", root.display()))?
        {
            let package_path = child?.path().join("workflow.json");
            if !package_path.is_file() {
                continue;
            }
            let encoded = fs::read(&package_path)
                .with_context(|| format!("read workflow package {}", package_path.display()))?;
            let package: ProjectWorkflowPackage = serde_json::from_slice(&encoded)
                .with_context(|| format!("invalid workflow package {}", package_path.display()))?;
            if package.schema_version != 1 {
                bail!(
                    "workflow package {} uses unsupported schema version {}",
                    package_path.display(),
                    package.schema_version
                );
            }
            let compiled = if let Some(program) = package.program {
                cap_workflow_permission(program, caller_permission)?
            } else {
                self.compile(&package.draft, caller_permission).with_context(|| {
                    format!(
                        "validate project workflow '{}' from {}",
                        package.definition.name,
                        package_path.display()
                    )
                })?
            };
            validate_spec(&compiled)?;
            let compiled_json = serde_json::to_string(&compiled)?;
            let draft_json = serde_json::to_string(&package.draft)?;
            let program_hash = hash_json(&compiled)?;
            let entry = package.definition;
            let existing_version = {
                let conn = self.storage.conn();
                conn.query_row(
                    "SELECT version FROM workflow_authoring_drafts WHERE id = ?1",
                    params![entry.draft_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
            };
            if existing_version.is_some_and(|version| version >= entry.draft_version as i64) {
                continue;
            }
            let now = Utc::now().to_rfc3339();
            let mut conn = self.storage.conn();
            let transaction = conn.transaction()?;
            transaction.execute(
                r#"
                INSERT INTO workflow_authoring_definitions
                    (id, name, description, lifecycle, scope_kind, scope_id, workspace,
                     owner_session_id, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, 'project', ?5, ?6, '', ?7, ?7)
                ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    description = excluded.description,
                    lifecycle = excluded.lifecycle,
                    scope_kind = 'project',
                    scope_id = excluded.scope_id,
                    workspace = excluded.workspace,
                    updated_at = excluded.updated_at
                "#,
                params![
                    entry.workflow_id,
                    entry.name,
                    entry.description,
                    lifecycle_as_str(entry.lifecycle),
                    project_id,
                    workspace,
                    now,
                ],
            )?;
            transaction.execute(
                r#"
                INSERT INTO workflow_authoring_drafts
                    (id, workflow_id, version, status, draft_spec, compiled_spec,
                     program_hash, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                ON CONFLICT(id) DO UPDATE SET
                    version = excluded.version,
                    status = excluded.status,
                    draft_spec = excluded.draft_spec,
                    compiled_spec = excluded.compiled_spec,
                    program_hash = excluded.program_hash,
                    updated_at = excluded.updated_at
                "#,
                params![
                    entry.draft_id,
                    entry.workflow_id,
                    entry.draft_version as i64,
                    if entry.lifecycle == WorkflowLifecycle::Published {
                        "published"
                    } else {
                        "valid"
                    },
                    draft_json,
                    compiled_json,
                    program_hash,
                    now,
                ],
            )?;
            transaction.commit()?;
            drop(conn);
            if entry.lifecycle == WorkflowLifecycle::Published {
                let revisions_directory = package_path
                    .parent()
                    .expect("workflow package has a parent")
                    .join("revisions");
                let mut imported_revision = false;
                if revisions_directory.is_dir() {
                    for revision_file in fs::read_dir(&revisions_directory)? {
                        let revision_file = revision_file?.path();
                        if revision_file.extension().and_then(|value| value.to_str()) != Some("json")
                        {
                            continue;
                        }
                        let encoded = fs::read(&revision_file)?;
                        let package_revision: ProjectWorkflowRevision =
                            serde_json::from_slice(&encoded).with_context(|| {
                                format!("invalid workflow revision {}", revision_file.display())
                            })?;
                        if package_revision.schema_version != 1 {
                            bail!(
                                "workflow revision {} uses unsupported schema version {}",
                                revision_file.display(),
                                package_revision.schema_version
                            );
                        }
                        let program =
                            cap_workflow_permission(package_revision.program, caller_permission)?;
                        validate_spec(&program)?;
                        let hash = hash_json(&program)?;
                        let revision_id = WorkflowRevisionId(format!(
                            "{}:r{}:{}",
                            entry.workflow_id,
                            package_revision.receipt.revision_number,
                            &hash[..16]
                        ));
                        self.revision_store.publish_revision(&revision_id, &program)?;
                        let conn = self.storage.conn();
                        conn.execute(
                            r#"
                            INSERT OR IGNORE INTO workflow_authoring_revisions
                                (revision_id, workflow_id, revision_number, draft_id,
                                 draft_version, program_hash, published_at)
                            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                            "#,
                            params![
                                revision_id.0,
                                entry.workflow_id,
                                package_revision.receipt.revision_number as i64,
                                entry.draft_id,
                                package_revision.receipt.draft_version as i64,
                                hash,
                                package_revision.receipt.published_at,
                            ],
                        )?;
                        imported_revision = true;
                    }
                }
                if !imported_revision {
                    let revision_number = entry
                        .latest_revision
                        .as_ref()
                        .map_or(1, |receipt| receipt.revision_number);
                    let revision_id = WorkflowRevisionId(format!(
                        "{}:r{}:{}",
                        entry.workflow_id,
                        revision_number,
                        &program_hash[..16]
                    ));
                    self.revision_store.publish_revision(&revision_id, &compiled)?;
                    let conn = self.storage.conn();
                    conn.execute(
                        r#"
                        INSERT OR IGNORE INTO workflow_authoring_revisions
                            (revision_id, workflow_id, revision_number, draft_id, draft_version,
                             program_hash, published_at)
                        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                        "#,
                        params![
                            revision_id.0,
                            entry.workflow_id,
                            revision_number as i64,
                            entry.draft_id,
                            entry.draft_version as i64,
                            program_hash,
                            now,
                        ],
                    )?;
                }
            }
            imported += 1;
        }
        Ok(imported)
    }

    pub fn get_library_entry(&self, workflow_id: &str) -> Result<WorkflowLibraryEntry> {
        self.catalog(None, true)?
            .into_iter()
            .find(|entry| entry.workflow_id == workflow_id)
            .or_else(|| {
                let conn = self.storage.conn();
                let project_id = conn
                    .query_row(
                        "SELECT scope_id FROM workflow_authoring_definitions WHERE id = ?1",
                        params![workflow_id],
                        |row| row.get::<_, String>(0),
                    )
                    .ok()?;
                drop(conn);
                self.catalog(Some(&project_id), true)
                    .ok()?
                    .into_iter()
                    .find(|entry| entry.workflow_id == workflow_id)
            })
            .ok_or_else(|| anyhow::anyhow!("workflow library entry not found: {workflow_id}"))
    }

    pub fn resolve_published_revision(
        &self,
        workflow_id: &str,
        revision_id: Option<&str>,
    ) -> Result<PublishedWorkflowReceipt> {
        let revisions = self.list_revisions()?;
        revisions
            .into_iter()
            .find(|receipt| {
                receipt.workflow_id == workflow_id
                    && revision_id.is_none_or(|id| receipt.revision_id.0 == id)
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "published workflow revision not found for workflow '{}'",
                    workflow_id
                )
            })
    }

    pub fn load_published_spec(&self, revision_id: &WorkflowRevisionId) -> Result<WorkflowSpec> {
        self.revision_store.load_revision(revision_id)
    }

    pub fn delete_transient_for_session(&self, session_id: &str) -> Result<usize> {
        let mut conn = self.storage.conn();
        let transaction = conn.transaction()?;
        let workflow_ids = {
            let mut statement = transaction.prepare(
                r#"
                SELECT id FROM workflow_authoring_definitions
                WHERE lifecycle = 'transient' AND owner_session_id = ?1
                "#,
            )?;
            let rows = statement.query_map(params![session_id], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for workflow_id in &workflow_ids {
            transaction.execute(
                "DELETE FROM workflow_authoring_drafts WHERE workflow_id = ?1",
                params![workflow_id],
            )?;
            transaction.execute(
                "DELETE FROM workflow_authoring_definitions WHERE id = ?1",
                params![workflow_id],
            )?;
        }
        let has_durable_runs: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'durable_workflow_runs')",
            [],
            |row| row.get(0),
        )?;
        if has_durable_runs {
            transaction.execute(
                r#"
                DELETE FROM durable_workflow_events
                WHERE run_id IN (
                    SELECT id FROM durable_workflow_runs
                    WHERE json_extract(scope, '$.session_id') = ?1
                      AND json_extract(scope, '$.trigger') IN
                          ('workflow_authoring', 'workflow_preview')
                )
                "#,
                params![session_id],
            )?;
            transaction.execute(
                r#"
                DELETE FROM durable_workflow_runs
                WHERE json_extract(scope, '$.session_id') = ?1
                  AND json_extract(scope, '$.trigger') IN
                      ('workflow_authoring', 'workflow_preview')
                "#,
                params![session_id],
            )?;
        }
        transaction.commit()?;
        Ok(workflow_ids.len())
    }

    pub fn delete_library_entry(&self, workflow_id: &str, expected_version: u64) -> Result<()> {
        let entry = self.get_library_entry(workflow_id)?;
        if entry.draft_version != expected_version {
            bail!(
                "workflow draft version conflict: expected {}, actual {}",
                expected_version,
                entry.draft_version
            );
        }
        let project_directory = match &entry.scope {
            WorkflowScope::Project { workspace, .. } => {
                Some(project_workflow_directory(workspace, &entry.name, workflow_id)?)
            }
            _ => None,
        };
        let mut conn = self.storage.conn();
        let transaction = conn.transaction()?;
        transaction.execute(
            "DELETE FROM workflow_authoring_revisions WHERE workflow_id = ?1",
            params![workflow_id],
        )?;
        transaction.execute(
            "DELETE FROM workflow_authoring_drafts WHERE workflow_id = ?1",
            params![workflow_id],
        )?;
        let deleted = transaction.execute(
            "DELETE FROM workflow_authoring_definitions WHERE id = ?1",
            params![workflow_id],
        )?;
        if deleted == 0 {
            bail!("workflow library entry not found: {workflow_id}");
        }
        transaction.commit()?;
        if let Some(directory) = project_directory {
            if directory.exists() {
                fs::remove_dir_all(&directory).with_context(|| {
                    format!("delete project workflow package {}", directory.display())
                })?;
            }
        }
        Ok(())
    }

    pub fn publish_imported_program(
        &self,
        request_id: &str,
        name: String,
        description: String,
        input_schema: Value,
        program: WorkflowSpec,
        scope: WorkflowScope,
    ) -> Result<PublishedWorkflowReceipt> {
        ensure_request_id(request_id)?;
        if matches!(scope, WorkflowScope::Session { .. }) {
            bail!("an imported workflow must use project or user scope");
        }
        validate_spec(&program)?;
        let workflow_id = uuid::Uuid::new_v4().to_string();
        let draft_id = uuid::Uuid::new_v4().to_string();
        let program_hash = hash_json(&program)?;
        let revision_id = WorkflowRevisionId(format!(
            "{}:r1:{}",
            workflow_id,
            &program_hash[..16]
        ));
        self.revision_store
            .publish_revision(&revision_id, &program)?;
        let now = Utc::now().to_rfc3339();
        let draft = WorkflowDraftSpec {
            name: name.clone(),
            description: description.clone(),
            input_schema,
            steps: Vec::new(),
            result: ValueExpr::Literal { value: Value::Null },
            policy: program.policy.clone(),
        };
        let (scope_kind, scope_id, workspace, owner_session_id) = scope.columns();
        let receipt = PublishedWorkflowReceipt {
            workflow_id: workflow_id.clone(),
            draft_id: draft_id.clone(),
            draft_version: 1,
            revision_id,
            revision_number: 1,
            program_hash: program_hash.clone(),
            published_at: now.clone(),
        };
        let mut conn = self.storage.conn();
        let transaction = conn.transaction()?;
        transaction.execute(
            r#"
            INSERT INTO workflow_authoring_definitions
                (id, name, description, lifecycle, scope_kind, scope_id, workspace,
                 owner_session_id, created_at, updated_at)
            VALUES (?1, ?2, ?3, 'published', ?4, ?5, ?6, ?7, ?8, ?8)
            "#,
            params![
                workflow_id,
                name,
                description,
                scope_kind,
                scope_id,
                workspace,
                owner_session_id,
                now,
            ],
        )?;
        transaction.execute(
            r#"
            INSERT INTO workflow_authoring_drafts
                (id, workflow_id, version, status, draft_spec, compiled_spec,
                 program_hash, created_at, updated_at)
            VALUES (?1, ?2, 1, 'published', ?3, ?4, ?5, ?6, ?6)
            "#,
            params![
                draft_id,
                workflow_id,
                serde_json::to_string(&draft)?,
                serde_json::to_string(&program)?,
                program_hash,
                now,
            ],
        )?;
        transaction.execute(
            r#"
            INSERT INTO workflow_authoring_revisions
                (revision_id, workflow_id, revision_number, draft_id, draft_version,
                 program_hash, published_at)
            VALUES (?1, ?2, 1, ?3, 1, ?4, ?5)
            "#,
            params![
                receipt.revision_id.0,
                workflow_id,
                draft_id,
                receipt.program_hash,
                now,
            ],
        )?;
        transaction.commit()?;
        drop(conn);
        if matches!(scope, WorkflowScope::Project { .. }) {
            self.write_project_package(&receipt.workflow_id)?;
            self.write_project_revision(&receipt)?;
        }
        Ok(receipt)
    }

    pub fn publish(&self, request: PublishWorkflowDraft) -> Result<PublishedWorkflowReceipt> {
        ensure_request_id(&request.request_id)?;
        if let Some(receipt) = self.load_idempotent_response(&request.request_id, "publish")? {
            return serde_json::from_str(&receipt).context("invalid stored publish receipt");
        }
        let draft = self.get_draft(&request.draft_id)?;
        if draft.receipt.lifecycle == WorkflowLifecycle::Transient {
            bail!("workflow must be saved to the Library before it can be published");
        }
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
            UPDATE workflow_authoring_definitions
            SET lifecycle = 'published', updated_at = ?2
            WHERE id = ?1
            "#,
            params![receipt.workflow_id, published_at],
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
        drop(conn);
        if matches!(draft.receipt.scope, WorkflowScope::Project { .. }) {
            self.write_project_package(&receipt.workflow_id)?;
            self.write_project_revision(&receipt)?;
        }
        Ok(receipt)
    }

    fn write_project_package(&self, workflow_id: &str) -> Result<()> {
        let entry = self.get_library_entry(workflow_id)?;
        let WorkflowScope::Project { workspace, .. } = &entry.scope else {
            return Ok(());
        };
        let draft = entry
            .workflow
            .clone()
            .ok_or_else(|| anyhow::anyhow!("project workflow is missing its draft"))?;
        let compiled = self.get_draft(&entry.draft_id)?.compiled;
        let directory = project_workflow_directory(workspace, &entry.name, workflow_id)?;
        fs::create_dir_all(&directory)
            .with_context(|| format!("create workflow directory {}", directory.display()))?;
        write_json_atomically(
            &directory.join("workflow.json"),
            &ProjectWorkflowPackage {
                schema_version: 1,
                definition: entry,
                draft,
                program: Some(compiled),
            },
        )
    }

    fn write_project_revision(&self, receipt: &PublishedWorkflowReceipt) -> Result<()> {
        let entry = self.get_library_entry(&receipt.workflow_id)?;
        let WorkflowScope::Project { workspace, .. } = &entry.scope else {
            return Ok(());
        };
        let directory = project_workflow_directory(workspace, &entry.name, &entry.workflow_id)?
            .join("revisions");
        fs::create_dir_all(&directory)
            .with_context(|| format!("create revision directory {}", directory.display()))?;
        let spec = self.revision_store.load_revision(&receipt.revision_id)?;
        let filename = format!(
            "{:04}-{}.json",
            receipt.revision_number,
            &receipt.program_hash[..16]
        );
        write_json_atomically(
            &directory.join(filename),
            &serde_json::json!({
                "schema_version": 1,
                "receipt": receipt,
                "program": spec,
            }),
        )
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

fn cap_workflow_permission(
    mut program: WorkflowSpec,
    caller_permission: &PermissionConfig,
) -> Result<WorkflowSpec> {
    for node in &mut program.nodes {
        if let NodeKind::Activity { kind, config } = &mut node.kind {
            if kind == CUSTOM_AGENT_ACTIVITY_KIND {
                let mut frozen: FrozenCustomAgentConfig =
                    serde_json::from_value(config.clone())
                        .context("invalid frozen custom-agent config in project workflow")?;
                frozen.permission = intersect_permission_ceiling(
                    caller_permission,
                    frozen.permission,
                );
                *config = serde_json::to_value(frozen)?;
            }
        }
    }
    Ok(program)
}

fn hash_json(value: &impl Serialize) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

fn lifecycle_as_str(lifecycle: WorkflowLifecycle) -> &'static str {
    match lifecycle {
        WorkflowLifecycle::Transient => "transient",
        WorkflowLifecycle::Draft => "draft",
        WorkflowLifecycle::Published => "published",
    }
}

fn lifecycle_from_str(value: &str) -> WorkflowLifecycle {
    match value {
        "transient" => WorkflowLifecycle::Transient,
        "published" => WorkflowLifecycle::Published,
        _ => WorkflowLifecycle::Draft,
    }
}

fn ensure_column(
    connection: &rusqlite::Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|existing| existing == column) {
        connection.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn project_workflow_directory(
    workspace: &str,
    name: &str,
    workflow_id: &str,
) -> Result<PathBuf> {
    let workspace = Path::new(workspace);
    if workspace.as_os_str().is_empty() || !workspace.is_absolute() {
        bail!("project workflow workspace must be an absolute path");
    }
    let mut slug = name
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "workflow" } else { slug };
    Ok(workspace
        .join(".agverse")
        .join("workflows")
        .join(format!("{slug}-{}", &workflow_id[..8.min(workflow_id.len())])))
}

fn write_json_atomically(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("workflow path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".{}.{}.tmp", path.file_name().unwrap_or_default().to_string_lossy(), uuid::Uuid::new_v4()));
    let encoded = serde_json::to_vec_pretty(value)?;
    fs::write(&temporary, encoded)
        .with_context(|| format!("write temporary workflow file {}", temporary.display()))?;
    fs::rename(&temporary, path)
        .with_context(|| format!("publish workflow file {}", path.display()))?;
    Ok(())
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
        let mut edited = inline_draft();
        edited.description = "A new unpublished edit".into();
        let derived = service
            .apply_draft(
                ApplyWorkflowDraft {
                    request_id: "edit-published".into(),
                    draft_id: Some(published.draft_id.clone()),
                    expected_version: Some(published.draft_version),
                    workflow: edited,
                },
                &permission,
            )
            .expect("derived draft");
        assert_ne!(derived.draft_id, published.draft_id);
        assert_eq!(derived.version, 1);
        assert_eq!(
            hash_json(
                &store
                    .load_revision(&published.revision_id)
                    .expect("old revision")
            )
            .expect("old hash"),
            hash_json(&revision).expect("revision hash")
        );
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

    #[test]
    fn transient_is_hidden_until_saved_and_is_cleaned_with_its_session() {
        let (_directory, service, _store) = service();
        let transient = service
            .apply_draft_in_scope(
                ApplyWorkflowDraft {
                    request_id: "temporary".into(),
                    draft_id: None,
                    expected_version: None,
                    workflow: inline_draft(),
                },
                &PermissionConfig::default(),
                WorkflowScope::Session {
                    session_id: "session-a".into(),
                },
            )
            .expect("temporary draft");
        assert_eq!(transient.lifecycle, WorkflowLifecycle::Transient);
        assert!(service.catalog(None, false).expect("catalog").is_empty());
        assert!(
            service
                .publish(PublishWorkflowDraft {
                    request_id: "publish-transient".into(),
                    draft_id: transient.draft_id,
                    expected_version: transient.version,
                })
                .unwrap_err()
                .to_string()
                .contains("must be saved")
        );
        assert_eq!(
            service
                .delete_transient_for_session("session-a")
                .expect("cleanup"),
            1
        );
        assert!(service.list_drafts().expect("drafts").is_empty());
    }

    #[test]
    fn project_draft_is_atomic_and_published_revisions_are_immutable() {
        let (directory, service, _store) = service();
        let transient = service
            .apply_draft_in_scope(
                ApplyWorkflowDraft {
                    request_id: "project-temporary".into(),
                    draft_id: None,
                    expected_version: None,
                    workflow: inline_draft(),
                },
                &PermissionConfig::default(),
                WorkflowScope::Session {
                    session_id: "session-a".into(),
                },
            )
            .expect("temporary draft");
        let saved = service
            .save_draft(SaveWorkflowDraft {
                request_id: "save-project".into(),
                draft_id: transient.draft_id,
                expected_version: transient.version,
                scope: WorkflowScope::Project {
                    project_id: "project-a".into(),
                    workspace: directory.path().to_string_lossy().into_owned(),
                },
            })
            .expect("save");
        assert_eq!(saved.lifecycle, WorkflowLifecycle::Draft);
        let package = project_workflow_directory(
            directory.path().to_str().expect("workspace"),
            "Summarize",
            &saved.workflow_id,
        )
        .expect("package")
        .join("workflow.json");
        assert!(package.is_file());
        assert!(
            package
                .parent()
                .expect("parent")
                .read_dir()
                .expect("read package")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
        );
        let published = service
            .publish(PublishWorkflowDraft {
                request_id: "publish-project".into(),
                draft_id: saved.draft_id,
                expected_version: saved.version,
            })
            .expect("publish");
        assert_eq!(
            service
                .catalog(Some("project-a"), false)
                .expect("catalog")[0]
                .lifecycle,
            WorkflowLifecycle::Published
        );
        assert_eq!(
            service
                .revisions_for_workflow(&published.workflow_id)
                .expect("history")
                .len(),
            1
        );
        assert!(
            package
                .parent()
                .expect("parent")
                .join("revisions")
                .read_dir()
                .expect("revisions")
                .next()
                .is_some()
        );
    }
}
