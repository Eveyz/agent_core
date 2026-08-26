//! Swarm execution workspace binding and turn leases.
//!
//! Conversation history stays on the session. This module owns the durable
//! execution directory for a swarm run. A live [`TurnWorkspaceLease`] is the
//! only way to obtain an [`ExecutionScope`]. The lease waits on a process-local
//! tokio RwLock keyed by the binding's `lock_key`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

const ADHOC_PROJECT_ID: &str = "__adhoc_chat__";
const OCCUPIED_STATUSES: &str = "('running', 'completing', 'needs_attention', 'cancelling')";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    Scratch,
    ProjectRoot,
}

impl WorkspaceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Scratch => "scratch",
            Self::ProjectRoot => "project_root",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "scratch" => Ok(Self::Scratch),
            "project_root" => Ok(Self::ProjectRoot),
            value => bail!("unknown workspace kind '{value}'"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceOwnership {
    Managed,
    Unmanaged,
}

impl WorkspaceOwnership {
    fn as_str(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::Unmanaged => "unmanaged",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "managed" => Ok(Self::Managed),
            "unmanaged" => Ok(Self::Unmanaged),
            value => bail!("unknown workspace ownership '{value}'"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupPolicy {
    Preserve,
}

impl CleanupPolicy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "preserve" => Ok(Self::Preserve),
            value => bail!("unknown cleanup policy '{value}'"),
        }
    }
}

/// Capability for one turn. Callers must not construct this for authorization;
/// [`classify_turn_access`] is the only classification entry point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnAccess {
    ReadOnly,
    ReadWrite,
}

const READONLY_WORKSPACE_TOOLS: &[&str] = &["read_file", "grep", "glob"];

/// Classifies workspace access from prepared tool names. Empty (inherit all)
/// and any non-whitelist name fail closed to [`TurnAccess::ReadWrite`].
pub fn classify_turn_access(tool_names: &[String]) -> TurnAccess {
    if tool_names.is_empty() {
        return TurnAccess::ReadWrite;
    }
    if tool_names
        .iter()
        .all(|name| READONLY_WORKSPACE_TOOLS.contains(&name.as_str()))
    {
        TurnAccess::ReadOnly
    } else {
        TurnAccess::ReadWrite
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceBinding {
    pub id: String,
    pub run_id: String,
    pub kind: WorkspaceKind,
    pub canonical_root: String,
    pub lock_key: String,
    pub ownership: WorkspaceOwnership,
    pub cleanup_policy: CleanupPolicy,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionScope {
    pub cwd: String,
    pub lock_key: String,
    pub access: TurnAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeOutcome {
    Preserve,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizeReport {
    pub run_id: String,
    pub preserved: bool,
}

struct LiveTurn {
    #[allow(dead_code)]
    lock_key: String,
}

#[allow(dead_code)]
enum HeldLock {
    Shared(OwnedRwLockReadGuard<()>),
    Exclusive(OwnedRwLockWriteGuard<()>),
}

/// Process-local lease. Not serializable. Drop only releases the in-memory
/// lock slot; it must not migrate durable swarm status.
pub struct TurnWorkspaceLease {
    run_id: String,
    turn_id: String,
    scope: ExecutionScope,
    live: Arc<Mutex<HashMap<(String, String), LiveTurn>>>,
    held: Option<HeldLock>,
    released: bool,
}

impl std::fmt::Debug for TurnWorkspaceLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnWorkspaceLease")
            .field("run_id", &self.run_id)
            .field("turn_id", &self.turn_id)
            .field("cwd", &self.scope.cwd)
            .field("access", &self.scope.access)
            .field("released", &self.released)
            .finish()
    }
}

impl TurnWorkspaceLease {
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    pub fn execution_scope(&self) -> &ExecutionScope {
        &self.scope
    }

    fn release_lock(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        self.held.take();
        self.live
            .lock()
            .remove(&(self.run_id.clone(), self.turn_id.clone()));
    }
}

impl Drop for TurnWorkspaceLease {
    fn drop(&mut self) {
        self.release_lock();
    }
}

#[derive(Clone)]
pub struct SwarmWorkspaceManager {
    scratch_root: Arc<PathBuf>,
    live: Arc<Mutex<HashMap<(String, String), LiveTurn>>>,
    locks: Arc<Mutex<HashMap<String, Arc<RwLock<()>>>>>,
}

impl SwarmWorkspaceManager {
    pub fn new(scratch_root: PathBuf) -> Self {
        Self {
            scratch_root: Arc::new(scratch_root),
            live: Arc::new(Mutex::new(HashMap::new())),
            locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_scratch_root(self, scratch_root: PathBuf) -> Self {
        Self {
            scratch_root: Arc::new(scratch_root),
            live: self.live,
            locks: self.locks,
        }
    }

    /// Creates the on-disk workspace (if managed) and records an immutable
    /// binding. Occupancy for project roots is checked in the same transaction.
    pub fn provision(
        &self,
        tx: &Transaction<'_>,
        run_id: &str,
        project_id: &str,
    ) -> Result<WorkspaceBinding> {
        let now = Utc::now().to_rfc3339();
        let binding = if is_adhoc_project(project_id) {
            prepare_scratch(&self.scratch_root, run_id, &now)?
        } else {
            prepare_project_root(tx, run_id, project_id, &now)?
        };
        persist_binding(tx, &binding)?;
        Ok(binding)
    }

    pub fn binding_for_run(&self, tx: &Transaction<'_>, run_id: &str) -> Result<WorkspaceBinding> {
        load_binding(tx, run_id)
    }

    pub async fn begin_turn(
        &self,
        binding: &WorkspaceBinding,
        run_id: &str,
        turn_id: &str,
        access: TurnAccess,
    ) -> Result<TurnWorkspaceLease> {
        if binding.run_id != run_id {
            bail!("workspace binding does not belong to swarm run '{run_id}'");
        }
        if binding.canonical_root.is_empty() || binding.lock_key.is_empty() {
            bail!("swarm workspace binding is missing a canonical root");
        }
        let key = (run_id.to_string(), turn_id.to_string());
        {
            let live = self.live.lock();
            if live.contains_key(&key) {
                bail!("turn '{turn_id}' already holds a workspace lease");
            }
        }
        let slot = {
            let mut locks = self.locks.lock();
            locks
                .entry(binding.lock_key.clone())
                .or_insert_with(|| Arc::new(RwLock::new(())))
                .clone()
        };
        let held = match access {
            TurnAccess::ReadOnly => HeldLock::Shared(slot.read_owned().await),
            TurnAccess::ReadWrite => HeldLock::Exclusive(slot.write_owned().await),
        };
        let mut live = self.live.lock();
        if live.contains_key(&key) {
            drop(held);
            bail!("turn '{turn_id}' already holds a workspace lease");
        }
        live.insert(
            key,
            LiveTurn {
                lock_key: binding.lock_key.clone(),
            },
        );
        drop(live);
        Ok(TurnWorkspaceLease {
            run_id: run_id.to_string(),
            turn_id: turn_id.to_string(),
            scope: ExecutionScope {
                cwd: binding.canonical_root.clone(),
                lock_key: binding.lock_key.clone(),
                access,
            },
            live: self.live.clone(),
            held: Some(held),
            released: false,
        })
    }

    pub fn live_lease_count(&self, run_id: &str) -> usize {
        self.live
            .lock()
            .keys()
            .filter(|(owned_run, _)| owned_run == run_id)
            .count()
    }

    pub fn finalize(
        &self,
        run_status: &str,
        binding: &WorkspaceBinding,
        outcome: FinalizeOutcome,
    ) -> Result<FinalizeReport> {
        if run_status == "cancelling" {
            bail!("cannot finalize a swarm workspace while the run is cancelling");
        }
        if self.live_lease_count(&binding.run_id) > 0 {
            bail!("cannot finalize a swarm workspace while turn leases are still held");
        }
        match outcome {
            FinalizeOutcome::Preserve => Ok(FinalizeReport {
                run_id: binding.run_id.clone(),
                preserved: true,
            }),
        }
    }
}

fn is_adhoc_project(project_id: &str) -> bool {
    project_id.is_empty() || project_id == ADHOC_PROJECT_ID
}

fn prepare_scratch(scratch_root: &Path, run_id: &str, now: &str) -> Result<WorkspaceBinding> {
    let workspace_path = scratch_root.join(run_id).join("workspace");
    std::fs::create_dir_all(&workspace_path).with_context(|| {
        format!(
            "failed to create swarm scratch workspace {}",
            workspace_path.display()
        )
    })?;
    let canonical = canonicalize_existing(&workspace_path)?;
    Ok(WorkspaceBinding {
        id: uuid::Uuid::new_v4().to_string(),
        run_id: run_id.to_string(),
        kind: WorkspaceKind::Scratch,
        lock_key: canonical.clone(),
        canonical_root: canonical,
        ownership: WorkspaceOwnership::Managed,
        cleanup_policy: CleanupPolicy::Preserve,
        created_at: now.to_string(),
    })
}

fn prepare_project_root(
    tx: &Transaction<'_>,
    run_id: &str,
    project_id: &str,
    now: &str,
) -> Result<WorkspaceBinding> {
    let path: String = tx
        .query_row(
            "SELECT path FROM projects WHERE id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .optional()?
        .with_context(|| format!("project '{project_id}' not found"))?;
    let canonical = canonicalize_existing(Path::new(&path))?;
    occupy_or_fail(tx, &canonical)?;
    Ok(WorkspaceBinding {
        id: uuid::Uuid::new_v4().to_string(),
        run_id: run_id.to_string(),
        kind: WorkspaceKind::ProjectRoot,
        lock_key: canonical.clone(),
        canonical_root: canonical,
        ownership: WorkspaceOwnership::Unmanaged,
        cleanup_policy: CleanupPolicy::Preserve,
        created_at: now.to_string(),
    })
}

fn occupy_or_fail(tx: &Transaction<'_>, lock_key: &str) -> Result<()> {
    let occupied: Option<String> = tx
        .query_row(
            &format!(
                "SELECT run.id FROM agent_swarm_runs AS run
                 JOIN agent_swarm_workspaces AS workspace ON workspace.id = run.workspace_id
                 WHERE workspace.lock_key = ?1 AND run.status IN {OCCUPIED_STATUSES}
                 LIMIT 1"
            ),
            params![lock_key],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(run_id) = occupied {
        bail!("project workspace is occupied by swarm run '{run_id}'");
    }
    Ok(())
}

fn persist_binding(tx: &Transaction<'_>, binding: &WorkspaceBinding) -> Result<()> {
    tx.execute(
        "INSERT INTO agent_swarm_workspaces
         (id, run_id, kind, canonical_root, lock_key, ownership, cleanup_policy, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            binding.id,
            binding.run_id,
            binding.kind.as_str(),
            binding.canonical_root,
            binding.lock_key,
            binding.ownership.as_str(),
            binding.cleanup_policy.as_str(),
            binding.created_at,
        ],
    )?;
    tx.execute(
        "UPDATE agent_swarm_runs SET workspace_id = ?1 WHERE id = ?2",
        params![binding.id, binding.run_id],
    )?;
    Ok(())
}

fn load_binding(tx: &Transaction<'_>, run_id: &str) -> Result<WorkspaceBinding> {
    let row = tx
        .query_row(
            "SELECT id, run_id, kind, canonical_root, lock_key, ownership, cleanup_policy, created_at
             FROM agent_swarm_workspaces WHERE run_id = ?1",
            params![run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?
        .with_context(|| format!("swarm run '{run_id}' has no workspace binding"))?;
    let (id, bound_run_id, kind, canonical_root, lock_key, ownership, cleanup_policy, created_at) =
        row;
    Ok(WorkspaceBinding {
        id,
        run_id: bound_run_id,
        kind: WorkspaceKind::parse(&kind)?,
        canonical_root,
        lock_key,
        ownership: WorkspaceOwnership::parse(&ownership)?,
        cleanup_policy: CleanupPolicy::parse(&cleanup_policy)?,
        created_at,
    })
}

fn canonicalize_existing(path: &Path) -> Result<String> {
    if path.as_os_str().is_empty() {
        bail!("workspace path must not be empty");
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize workspace path {}", path.display()))?;
    let text = canonical
        .to_str()
        .with_context(|| format!("workspace path {} is not valid UTF-8", canonical.display()))?;
    if !canonical.is_absolute() {
        bail!("workspace path must be absolute");
    }
    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::storage::Storage;

    fn storage_with_project(project_path: &Path) -> (tempfile::TempDir, Storage, String) {
        let directory = tempfile::tempdir().expect("tempdir");
        let storage = Storage::new(directory.path().join("test.db").to_str().expect("path"))
            .expect("storage");
        let project_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        storage
            .conn()
            .execute(
                "INSERT INTO projects (id, name, path, pinned, pinned_at, created_at, updated_at)
                 VALUES (?1, 'demo', ?2, 0, '', ?3, ?3)",
                params![project_id, project_path.to_str().expect("utf8"), now],
            )
            .expect("insert project");
        (directory, storage, project_id)
    }

    fn insert_run(storage: &Storage, run_id: &str) {
        let now = Utc::now().to_rfc3339();
        storage
            .conn()
            .execute(
                "INSERT INTO agent_swarm_runs
                 (id, project_id, root_agent_id, goal, status, max_messages, messages_used,
                  max_turns, turns_used, max_hops, hops_used, summary, error,
                  created_at, updated_at, completed_at, completion_task_id, completion_turn_id,
                  workspace_id)
                 VALUES (?1, 'p', 'coder', 'g', 'running', 1, 0, 1, 0, 1, 0, '', '', ?2, ?2,
                         NULL, NULL, NULL, '')",
                params![run_id, now],
            )
            .expect("insert run");
    }

    #[test]
    fn adhoc_provision_creates_canonical_scratch() {
        let directory = tempfile::tempdir().expect("tempdir");
        let storage = Storage::new(directory.path().join("test.db").to_str().expect("path"))
            .expect("storage");
        let manager = SwarmWorkspaceManager::new(directory.path().join("swarms"));
        let run_id = uuid::Uuid::new_v4().to_string();
        insert_run(&storage, &run_id);
        let mut db = storage.conn();
        let tx = db.transaction().expect("tx");
        let binding = manager
            .provision(&tx, &run_id, "__adhoc_chat__")
            .expect("provision");
        tx.commit().expect("commit");
        assert_eq!(binding.kind, WorkspaceKind::Scratch);
        assert_eq!(binding.ownership, WorkspaceOwnership::Managed);
        assert!(Path::new(&binding.canonical_root).is_absolute());
        assert_eq!(binding.lock_key, binding.canonical_root);
        assert!(Path::new(&binding.canonical_root).is_dir());
    }

    #[test]
    fn project_occupancy_is_atomic_in_the_provision_transaction() {
        let project_dir = tempfile::tempdir().expect("project");
        let (_db_dir, storage, project_id) = storage_with_project(project_dir.path());
        let manager =
            SwarmWorkspaceManager::new(tempfile::tempdir().unwrap().path().join("swarms"));
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        insert_run(&storage, &first);
        insert_run(&storage, &second);
        {
            let mut db = storage.conn();
            let tx = db.transaction().expect("tx");
            manager
                .provision(&tx, &first, &project_id)
                .expect("first occupant");
            tx.commit().expect("commit");
        }
        let mut db = storage.conn();
        let tx = db.transaction().expect("tx");
        let error = manager
            .provision(&tx, &second, &project_id)
            .expect_err("second occupant");
        assert!(error.to_string().contains("occupied"));
    }

    #[test]
    fn scratch_runs_do_not_occupy_each_other() {
        let directory = tempfile::tempdir().expect("tempdir");
        let storage = Storage::new(directory.path().join("test.db").to_str().expect("path"))
            .expect("storage");
        let manager = SwarmWorkspaceManager::new(directory.path().join("swarms"));
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        insert_run(&storage, &first);
        insert_run(&storage, &second);
        let mut db = storage.conn();
        let tx = db.transaction().expect("tx");
        manager
            .provision(&tx, &first, "__adhoc_chat__")
            .expect("first scratch");
        manager
            .provision(&tx, &second, "__adhoc_chat__")
            .expect("second scratch");
        tx.commit().expect("commit");
    }

    fn provision_scratch(
        manager: &SwarmWorkspaceManager,
        storage: &Storage,
    ) -> (String, WorkspaceBinding) {
        let run_id = uuid::Uuid::new_v4().to_string();
        insert_run(storage, &run_id);
        let mut db = storage.conn();
        let tx = db.transaction().expect("tx");
        let binding = manager
            .provision(&tx, &run_id, "__adhoc_chat__")
            .expect("provision");
        tx.commit().expect("commit");
        (run_id, binding)
    }

    #[test]
    fn classify_turn_access_fails_closed_outside_the_readonly_whitelist() {
        assert_eq!(classify_turn_access(&[]), TurnAccess::ReadWrite);
        assert_eq!(
            classify_turn_access(&["read_file".into(), "grep".into(), "glob".into()]),
            TurnAccess::ReadOnly
        );
        assert_eq!(
            classify_turn_access(&["read_file".into(), "shell".into()]),
            TurnAccess::ReadWrite
        );
        assert_eq!(
            classify_turn_access(&["mcp_unknown".into()]),
            TurnAccess::ReadWrite
        );
    }

    #[tokio::test]
    async fn execution_scope_is_only_available_through_a_live_lease() {
        let directory = tempfile::tempdir().expect("tempdir");
        let storage = Storage::new(directory.path().join("test.db").to_str().expect("path"))
            .expect("storage");
        let manager = SwarmWorkspaceManager::new(directory.path().join("swarms"));
        let (run_id, binding) = provision_scratch(&manager, &storage);
        let lease = manager
            .begin_turn(&binding, &run_id, "turn-1", TurnAccess::ReadWrite)
            .await
            .expect("lease");
        assert_eq!(lease.execution_scope().cwd, binding.canonical_root);
        assert_eq!(lease.execution_scope().access, TurnAccess::ReadWrite);
        assert_eq!(manager.live_lease_count(&run_id), 1);
        drop(lease);
        assert_eq!(manager.live_lease_count(&run_id), 0);
        manager
            .finalize("cancelling", &binding, FinalizeOutcome::Preserve)
            .expect_err("cancelling refuses finalize");
        let report = manager
            .finalize("completed", &binding, FinalizeOutcome::Preserve)
            .expect("preserve");
        assert!(report.preserved);
    }

    #[tokio::test]
    async fn readonly_leases_overlap_on_the_same_lock_key() {
        let directory = tempfile::tempdir().expect("tempdir");
        let storage = Storage::new(directory.path().join("test.db").to_str().expect("path"))
            .expect("storage");
        let manager = SwarmWorkspaceManager::new(directory.path().join("swarms"));
        let (run_id, binding) = provision_scratch(&manager, &storage);
        let first = manager
            .begin_turn(&binding, &run_id, "reader-1", TurnAccess::ReadOnly)
            .await
            .expect("first reader");
        let second = manager
            .begin_turn(&binding, &run_id, "reader-2", TurnAccess::ReadOnly)
            .await
            .expect("second reader");
        assert_eq!(manager.live_lease_count(&run_id), 2);
        drop(first);
        drop(second);
        assert_eq!(manager.live_lease_count(&run_id), 0);
    }

    #[tokio::test]
    async fn write_lock_excludes_another_writer() {
        let directory = tempfile::tempdir().expect("tempdir");
        let storage = Storage::new(directory.path().join("test.db").to_str().expect("path"))
            .expect("storage");
        let manager = SwarmWorkspaceManager::new(directory.path().join("swarms"));
        let (run_id, binding) = provision_scratch(&manager, &storage);
        let first = manager
            .begin_turn(&binding, &run_id, "writer-1", TurnAccess::ReadWrite)
            .await
            .expect("first writer");
        let waiter = {
            let manager = manager.clone();
            let binding = binding.clone();
            let run_id = run_id.clone();
            tokio::spawn(async move {
                manager
                    .begin_turn(&binding, &run_id, "writer-2", TurnAccess::ReadWrite)
                    .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        assert!(!waiter.is_finished());
        drop(first);
        let second = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("writer wait")
            .expect("join")
            .expect("second writer");
        assert_eq!(second.execution_scope().access, TurnAccess::ReadWrite);
    }

    #[tokio::test]
    async fn write_lock_excludes_readers() {
        let directory = tempfile::tempdir().expect("tempdir");
        let storage = Storage::new(directory.path().join("test.db").to_str().expect("path"))
            .expect("storage");
        let manager = SwarmWorkspaceManager::new(directory.path().join("swarms"));
        let (run_id, binding) = provision_scratch(&manager, &storage);
        let writer = manager
            .begin_turn(&binding, &run_id, "writer", TurnAccess::ReadWrite)
            .await
            .expect("writer");
        let waiter = {
            let manager = manager.clone();
            let binding = binding.clone();
            let run_id = run_id.clone();
            tokio::spawn(async move {
                manager
                    .begin_turn(&binding, &run_id, "reader", TurnAccess::ReadOnly)
                    .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        assert!(!waiter.is_finished());
        drop(writer);
        let reader = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("reader wait")
            .expect("join")
            .expect("reader");
        assert_eq!(reader.execution_scope().access, TurnAccess::ReadOnly);
    }
}
