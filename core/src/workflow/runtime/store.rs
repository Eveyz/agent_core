use std::{collections::HashMap, sync::Arc};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{params, OptionalExtension};
use sha2::Digest;

use super::model::{
    RunId, StoredRun, WorkflowEvent, WorkflowEventKind, WorkflowRevisionId, WorkflowSpec,
};
use crate::memory::storage::Storage;

pub struct CreateStoredRun {
    pub run: StoredRun,
}

pub struct CreateStoredRunResult {
    pub run_id: RunId,
    pub created: bool,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "workflow history conflict for {run_id}: expected {expected_sequence}, actual {actual_sequence}"
)]
pub struct HistoryConflict {
    pub run_id: String,
    pub expected_sequence: u64,
    pub actual_sequence: u64,
}

pub fn is_history_conflict(error: &anyhow::Error) -> bool {
    error.downcast_ref::<HistoryConflict>().is_some()
}

pub trait WorkflowStore: Send + Sync + 'static {
    fn create_or_get(&self, request: CreateStoredRun) -> Result<CreateStoredRunResult>;
    fn load(&self, run_id: &RunId) -> Result<StoredRun>;
    fn history(&self, run_id: &RunId, after_sequence: Option<u64>) -> Result<Vec<WorkflowEvent>>;
    fn append(
        &self,
        run_id: &RunId,
        expected_last_sequence: u64,
        events: Vec<WorkflowEventKind>,
    ) -> Result<Vec<WorkflowEvent>>;
    fn recoverable_runs(&self) -> Result<Vec<RunId>>;
    fn publish_revision(&self, revision_id: &WorkflowRevisionId, spec: &WorkflowSpec)
        -> Result<()>;
    fn load_revision(&self, revision_id: &WorkflowRevisionId) -> Result<WorkflowSpec>;
}

#[derive(Default)]
struct InMemoryState {
    runs: HashMap<RunId, StoredRun>,
    by_request: HashMap<String, RunId>,
    events: HashMap<RunId, Vec<WorkflowEvent>>,
    revisions: HashMap<WorkflowRevisionId, WorkflowSpec>,
}

#[derive(Clone, Default)]
pub struct InMemoryWorkflowStore {
    state: Arc<Mutex<InMemoryState>>,
}

impl WorkflowStore for InMemoryWorkflowStore {
    fn create_or_get(&self, request: CreateStoredRun) -> Result<CreateStoredRunResult> {
        let mut state = self.state.lock();
        if let Some(run_id) = state.by_request.get(&request.run.request_id) {
            return Ok(CreateStoredRunResult {
                run_id: run_id.clone(),
                created: false,
            });
        }

        let run_id = request.run.run_id.clone();
        let request_id = request.run.request_id.clone();
        state.runs.insert(run_id.clone(), request.run);
        state.by_request.insert(request_id, run_id.clone());
        state.events.insert(
            run_id.clone(),
            vec![WorkflowEvent {
                run_id: run_id.clone(),
                sequence: 0,
                created_at: Utc::now().to_rfc3339(),
                kind: WorkflowEventKind::RunCreated,
            }],
        );
        Ok(CreateStoredRunResult {
            run_id,
            created: true,
        })
    }

    fn load(&self, run_id: &RunId) -> Result<StoredRun> {
        self.state
            .lock()
            .runs
            .get(run_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("workflow run not found: {}", run_id.0))
    }

    fn history(&self, run_id: &RunId, after_sequence: Option<u64>) -> Result<Vec<WorkflowEvent>> {
        let state = self.state.lock();
        let events = state
            .events
            .get(run_id)
            .ok_or_else(|| anyhow::anyhow!("workflow run not found: {}", run_id.0))?;
        Ok(events
            .iter()
            .filter(|event| after_sequence.is_none_or(|after| event.sequence > after))
            .cloned()
            .collect())
    }

    fn append(
        &self,
        run_id: &RunId,
        expected_last_sequence: u64,
        events: Vec<WorkflowEventKind>,
    ) -> Result<Vec<WorkflowEvent>> {
        let mut state = self.state.lock();
        let history = state
            .events
            .get_mut(run_id)
            .ok_or_else(|| anyhow::anyhow!("workflow run not found: {}", run_id.0))?;
        let last = history
            .last()
            .map(|event| event.sequence)
            .ok_or_else(|| anyhow::anyhow!("workflow history is empty: {}", run_id.0))?;
        if last != expected_last_sequence {
            return Err(HistoryConflict {
                run_id: run_id.0.clone(),
                expected_sequence: expected_last_sequence,
                actual_sequence: last,
            }
            .into());
        }

        let created_at = Utc::now().to_rfc3339();
        let appended: Vec<_> = events
            .into_iter()
            .enumerate()
            .map(|(index, kind)| WorkflowEvent {
                run_id: run_id.clone(),
                sequence: expected_last_sequence + index as u64 + 1,
                created_at: created_at.clone(),
                kind,
            })
            .collect();
        history.extend(appended.iter().cloned());
        Ok(appended)
    }

    fn recoverable_runs(&self) -> Result<Vec<RunId>> {
        Ok(self.state.lock().runs.keys().cloned().collect())
    }

    fn publish_revision(
        &self,
        revision_id: &WorkflowRevisionId,
        spec: &WorkflowSpec,
    ) -> Result<()> {
        let mut state = self.state.lock();
        if let Some(existing) = state.revisions.get(revision_id) {
            if serde_json::to_value(existing)? != serde_json::to_value(spec)? {
                bail!("workflow revision '{}' is immutable", revision_id.0);
            }
            return Ok(());
        }
        state.revisions.insert(revision_id.clone(), spec.clone());
        Ok(())
    }

    fn load_revision(&self, revision_id: &WorkflowRevisionId) -> Result<WorkflowSpec> {
        self.state
            .lock()
            .revisions
            .get(revision_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("workflow revision not found: {}", revision_id.0))
    }
}

#[derive(Clone)]
pub struct SqliteWorkflowStore {
    storage: Storage,
}

impl SqliteWorkflowStore {
    pub fn new(storage: Storage) -> Result<Self> {
        {
            let conn = storage.conn();
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS durable_workflow_runs (
                    id TEXT PRIMARY KEY,
                    request_id TEXT NOT NULL UNIQUE,
                    program_hash TEXT NOT NULL,
                    manifest TEXT NOT NULL,
                    input TEXT NOT NULL,
                    scope TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'pending',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_durable_workflow_runs_status
                    ON durable_workflow_runs(status);

                CREATE TABLE IF NOT EXISTS durable_workflow_revisions (
                    id TEXT PRIMARY KEY,
                    program_hash TEXT NOT NULL,
                    spec TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS durable_workflow_events (
                    run_id TEXT NOT NULL,
                    sequence INTEGER NOT NULL,
                    event TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (run_id, sequence),
                    FOREIGN KEY (run_id) REFERENCES durable_workflow_runs(id)
                        ON DELETE RESTRICT
                );
                "#,
            )
            .context("failed to initialize durable workflow tables")?;
        }
        Ok(Self { storage })
    }

    fn decode_run(
        run_id: String,
        request_id: String,
        manifest: String,
        input: String,
        scope: String,
    ) -> Result<StoredRun> {
        Ok(StoredRun {
            run_id: RunId(run_id),
            request_id,
            manifest: serde_json::from_str(&manifest)
                .context("invalid persisted workflow manifest")?,
            input: serde_json::from_str(&input).context("invalid persisted workflow input")?,
            scope: serde_json::from_str(&scope).context("invalid persisted workflow scope")?,
        })
    }
}

impl WorkflowStore for SqliteWorkflowStore {
    fn create_or_get(&self, request: CreateStoredRun) -> Result<CreateStoredRunResult> {
        let mut conn = self.storage.conn();
        let transaction = conn.transaction()?;
        let existing = transaction
            .query_row(
                "SELECT id FROM durable_workflow_runs WHERE request_id = ?1",
                params![request.run.request_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(run_id) = existing {
            transaction.commit()?;
            return Ok(CreateStoredRunResult {
                run_id: RunId(run_id),
                created: false,
            });
        }

        let now = Utc::now().to_rfc3339();
        let manifest = serde_json::to_string(&request.run.manifest)?;
        let input = serde_json::to_string(&request.run.input)?;
        let scope = serde_json::to_string(&request.run.scope)?;
        let created_event = serde_json::to_string(&WorkflowEventKind::RunCreated)?;
        transaction.execute(
            r#"
            INSERT INTO durable_workflow_runs
                (id, request_id, program_hash, manifest, input, scope, status, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7)
            "#,
            params![
                request.run.run_id.0,
                request.run.request_id,
                request.run.manifest.program_hash,
                manifest,
                input,
                scope,
                now,
            ],
        )?;
        transaction.execute(
            r#"
            INSERT INTO durable_workflow_events (run_id, sequence, event, created_at)
            VALUES (?1, 0, ?2, ?3)
            "#,
            params![request.run.run_id.0, created_event, now],
        )?;
        transaction.commit()?;

        Ok(CreateStoredRunResult {
            run_id: request.run.run_id,
            created: true,
        })
    }

    fn load(&self, run_id: &RunId) -> Result<StoredRun> {
        let conn = self.storage.conn();
        let row = conn
            .query_row(
                r#"
                SELECT id, request_id, manifest, input, scope
                FROM durable_workflow_runs
                WHERE id = ?1
                "#,
                params![run_id.0],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("workflow run not found: {}", run_id.0))?;
        Self::decode_run(row.0, row.1, row.2, row.3, row.4)
    }

    fn history(&self, run_id: &RunId, after_sequence: Option<u64>) -> Result<Vec<WorkflowEvent>> {
        let conn = self.storage.conn();
        let mut statement = conn.prepare(
            r#"
            SELECT sequence, event, created_at
            FROM durable_workflow_events
            WHERE run_id = ?1 AND sequence > ?2
            ORDER BY sequence ASC
            "#,
        )?;
        let rows = statement.query_map(
            params![
                run_id.0,
                after_sequence.map_or(-1_i64, |sequence| sequence as i64)
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;

        let mut events = Vec::new();
        for row in rows {
            let (sequence, event, created_at) = row?;
            events.push(WorkflowEvent {
                run_id: run_id.clone(),
                sequence: sequence as u64,
                created_at,
                kind: serde_json::from_str(&event).context("invalid persisted workflow event")?,
            });
        }
        if events.is_empty() {
            let exists = conn
                .query_row(
                    "SELECT 1 FROM durable_workflow_runs WHERE id = ?1",
                    params![run_id.0],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !exists {
                bail!("workflow run not found: {}", run_id.0);
            }
        }
        Ok(events)
    }

    fn append(
        &self,
        run_id: &RunId,
        expected_last_sequence: u64,
        events: Vec<WorkflowEventKind>,
    ) -> Result<Vec<WorkflowEvent>> {
        let mut conn = self.storage.conn();
        let transaction = conn.transaction()?;
        let last: Option<i64> = transaction.query_row(
            "SELECT MAX(sequence) FROM durable_workflow_events WHERE run_id = ?1",
            params![run_id.0],
            |row| row.get(0),
        )?;
        let last = last.ok_or_else(|| anyhow::anyhow!("workflow run not found: {}", run_id.0))?;
        if last as u64 != expected_last_sequence {
            return Err(HistoryConflict {
                run_id: run_id.0.clone(),
                expected_sequence: expected_last_sequence,
                actual_sequence: last as u64,
            }
            .into());
        }

        let created_at = Utc::now().to_rfc3339();
        let mut appended = Vec::with_capacity(events.len());
        let mut projected_status = None;
        for (index, kind) in events.into_iter().enumerate() {
            let sequence = expected_last_sequence + index as u64 + 1;
            transaction.execute(
                r#"
                INSERT INTO durable_workflow_events (run_id, sequence, event, created_at)
                VALUES (?1, ?2, ?3, ?4)
                "#,
                params![
                    run_id.0,
                    sequence as i64,
                    serde_json::to_string(&kind)?,
                    created_at,
                ],
            )?;
            projected_status = status_projection(&kind).or(projected_status);
            appended.push(WorkflowEvent {
                run_id: run_id.clone(),
                sequence,
                created_at: created_at.clone(),
                kind,
            });
        }
        if let Some(status) = projected_status {
            transaction.execute(
                r#"
                UPDATE durable_workflow_runs
                SET status = ?2, updated_at = ?3
                WHERE id = ?1
                "#,
                params![run_id.0, status, created_at],
            )?;
        }
        transaction.commit()?;
        Ok(appended)
    }

    fn recoverable_runs(&self) -> Result<Vec<RunId>> {
        let conn = self.storage.conn();
        let mut statement = conn.prepare(
            r#"
            SELECT id
            FROM durable_workflow_runs
            WHERE status IN ('pending', 'running', 'waiting', 'paused')
            ORDER BY created_at ASC
            "#,
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(RunId)
            .collect())
    }

    fn publish_revision(
        &self,
        revision_id: &WorkflowRevisionId,
        spec: &WorkflowSpec,
    ) -> Result<()> {
        let encoded = serde_json::to_vec(spec)?;
        let hash = hex::encode(sha2::Sha256::digest(&encoded));
        let conn = self.storage.conn();
        let existing = conn
            .query_row(
                "SELECT program_hash FROM durable_workflow_revisions WHERE id = ?1",
                params![revision_id.0],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing != hash {
                bail!("workflow revision '{}' is immutable", revision_id.0);
            }
            return Ok(());
        }
        conn.execute(
            r#"
            INSERT INTO durable_workflow_revisions (id, program_hash, spec, created_at)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                revision_id.0,
                hash,
                String::from_utf8(encoded).context("workflow spec is not utf8 JSON")?,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn load_revision(&self, revision_id: &WorkflowRevisionId) -> Result<WorkflowSpec> {
        let conn = self.storage.conn();
        let encoded = conn
            .query_row(
                "SELECT spec FROM durable_workflow_revisions WHERE id = ?1",
                params![revision_id.0],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("workflow revision not found: {}", revision_id.0))?;
        serde_json::from_str(&encoded).context("invalid persisted workflow revision")
    }
}

fn status_projection(event: &WorkflowEventKind) -> Option<&'static str> {
    match event {
        WorkflowEventKind::RunStarted | WorkflowEventKind::RunResumed { .. } => Some("running"),
        WorkflowEventKind::RunPaused { .. } => Some("paused"),
        WorkflowEventKind::NodeWaiting { .. } => Some("waiting"),
        WorkflowEventKind::NodeNeedsAttention { .. } => Some("needs_attention"),
        WorkflowEventKind::RunCompleted { .. } => Some("succeeded"),
        WorkflowEventKind::RunFailed { .. } => Some("failed"),
        WorkflowEventKind::RunCancelled { .. } => Some("cancelled"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{json, Value};

    use super::*;
    use crate::{
        memory::storage::Storage,
        workflow::runtime::{
            EffectPolicy, NodeKey, NodeKind, NodeSpec, RetryPolicy, RunManifest, RunScope,
            ValueExpr, WorkflowPolicy, WorkflowSpec,
        },
    };

    fn stored_run(run_id: &str, request_id: &str) -> StoredRun {
        StoredRun {
            run_id: RunId(run_id.to_string()),
            request_id: request_id.to_string(),
            manifest: RunManifest {
                program_hash: "hash".to_string(),
                program: WorkflowSpec {
                    schema_version: 1,
                    nodes: vec![NodeSpec {
                        key: NodeKey::from("output"),
                        kind: NodeKind::Output,
                        inputs: BTreeMap::new(),
                        after: Vec::new(),
                        retry: RetryPolicy::default(),
                        timeout_ms: None,
                        effect: EffectPolicy::Pure,
                        resources: Vec::new(),
                    }],
                    result: ValueExpr::Literal { value: Value::Null },
                    policy: WorkflowPolicy::default(),
                },
                adapter_versions: BTreeMap::new(),
            },
            input: json!({ "hello": "world" }),
            scope: RunScope::default(),
        }
    }

    #[test]
    fn sqlite_store_persists_history_and_request_idempotency() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("workflow.sqlite");
        let first_id = RunId("run-1".to_string());
        {
            let storage = Storage::new(path.to_str().expect("utf8 path")).expect("storage");
            let store = SqliteWorkflowStore::new(storage).expect("sqlite workflow store");
            let created = store
                .create_or_get(CreateStoredRun {
                    run: stored_run(&first_id.0, "request-1"),
                })
                .expect("create run");
            assert!(created.created);
            store
                .append(&first_id, 0, vec![WorkflowEventKind::RunStarted])
                .expect("append run started");
        }

        let storage = Storage::new(path.to_str().expect("utf8 path")).expect("reopen storage");
        let store = SqliteWorkflowStore::new(storage).expect("reopen workflow store");
        let loaded = store.load(&first_id).expect("load persisted run");
        assert_eq!(loaded.input, json!({ "hello": "world" }));
        let history = store.history(&first_id, None).expect("persisted history");
        assert_eq!(history.len(), 2);
        assert!(matches!(history[1].kind, WorkflowEventKind::RunStarted));

        let duplicate = store
            .create_or_get(CreateStoredRun {
                run: stored_run("run-2", "request-1"),
            })
            .expect("idempotent create");
        assert!(!duplicate.created);
        assert_eq!(duplicate.run_id, first_id);
        assert!(
            store
                .append(&first_id, 0, vec![WorkflowEventKind::RunStarted])
                .is_err(),
            "stale sequence must be rejected"
        );

        let revision_id = WorkflowRevisionId("revision-1".to_string());
        let spec = loaded.manifest.program;
        store
            .publish_revision(&revision_id, &spec)
            .expect("publish revision");
        assert_eq!(
            serde_json::to_value(store.load_revision(&revision_id).expect("load revision"))
                .expect("revision json"),
            serde_json::to_value(&spec).expect("spec json")
        );
        let mut changed = spec;
        changed.policy.max_concurrency += 1;
        assert!(
            store.publish_revision(&revision_id, &changed).is_err(),
            "published revisions must be immutable"
        );
    }
}
