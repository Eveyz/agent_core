//! RunManager — creates and tracks Runs, routes commands.
//!
//! The RunManager owns the shared [`Brain`] and a map of active [`Run`]s.
//! It is the primary interface for the frontend/CLI bridge layer.
//!
//! ## Lifecycle
//!
//! ```text
//! create_run() → RunId
//!   ├── command(run_id, Start)        → Run begins executing
//!   ├── command(run_id, Steer{...})   → Inject mid-run message
//!   ├── command(run_id, Cancel)       → Kill the run + all children
//!   └── subscribe(run_id)             → Get event stream
//! ```

use anyhow::{Context, Result};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::reflector::Reflector;
use crate::runtime::brain::Brain;
use crate::runtime::command::RunCommand;
use crate::runtime::event::{Envelope, RunEvent, RunId};
use crate::runtime::event_log::EventLog;
use crate::runtime::run::{Run, default_runs_dir};
use crate::runtime::state::RunState;
use crate::worktree::WorktreeManager;

/// Capacity for the event broadcast channel per Run.
const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Capacity for the command channel per Run.
const COMMAND_CHANNEL_CAPACITY: usize = 64;

/// A handle to an active (or recently completed) Run.
pub struct RunHandle {
    pub id: RunId,
    pub session_id: Option<String>,
    /// Sender for commands (Start, Pause, Cancel, Steer, Approve, etc.)
    cmd_tx: mpsc::Sender<RunCommand>,
    /// Broadcast sender for events. Subscribers call `.subscribe()` on this.
    pub event_tx: broadcast::Sender<Envelope>,
    /// The tokio task running the Run's loop.
    join_handle: Option<JoinHandle<()>>,
    /// Shared state for querying (read-only, updated by the Run task).
    state: Arc<RwLock<RunState>>,
}

impl RunHandle {
    /// Send a command to the Run.
    pub fn command(&self, cmd: RunCommand) -> Result<()> {
        self.cmd_tx
            .try_send(cmd)
            .context("failed to send command to run (channel full or closed)")
    }

    /// Subscribe to the Run's event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.event_tx.subscribe()
    }

    /// Current state of the Run (best-effort, may be slightly stale).
    pub fn state(&self) -> RunState {
        *self.state.read()
    }

    /// Whether the Run has finished (terminal state).
    pub fn is_done(&self) -> bool {
        self.state().is_terminal()
    }

    /// Wait for the Run's task to complete.
    pub async fn join(mut self) -> Result<()> {
        if let Some(handle) = self.join_handle.take() {
            handle.await.context("run task panicked")?;
        }
        Ok(())
    }
}

/// The RunManager — owns the Brain and tracks all active Runs.
pub struct RunManager {
    brain: Arc<Brain>,
    runs: Mutex<HashMap<RunId, RunHandle>>,
}

impl RunManager {
    /// Create a RunManager from a loaded Config.
    pub fn new(brain: Brain) -> Self {
        Self {
            brain: Arc::new(brain),
            runs: Mutex::new(HashMap::new()),
        }
    }

    /// Load config from a file and create a RunManager.
    pub fn load_config(path: &str) -> Result<Self> {
        let brain = Brain::load_config(path)?;
        Ok(Self::new(brain))
    }

    /// Access the shared Brain (for model switching, config queries, etc.)
    pub fn brain(&self) -> &Arc<Brain> {
        &self.brain
    }

    /// Create a new Run for a user request.
    ///
    /// Returns the RunId. The Run starts in `Created` state — call
    /// `command(run_id, RunCommand::Start)` to begin execution.
    pub async fn create_run(
        &self,
        user_input: &str,
        session_id: Option<String>,
        history: Vec<crate::types::Message>,
    ) -> Result<RunId> {
        self.create_run_with_workdir(user_input, session_id, None, history)
            .await
    }

    /// Create a Run with an isolated working directory (for worktree isolation).
    /// When `working_dir` is set, the Run's tools execute in that directory
    /// instead of the process CWD, allowing multiple concurrent Runs to work
    /// in separate git worktrees without file conflicts.
    pub async fn create_run_with_workdir(
        &self,
        user_input: &str,
        session_id: Option<String>,
        working_dir: Option<String>,
        history: Vec<crate::types::Message>,
    ) -> Result<RunId> {
        let run_id = uuid::Uuid::new_v4().to_string();

        // Get the current model config
        let model_config = self.brain.current_model_config()?;

        // Create channels
        let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        let (event_tx, _event_rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);

        // Shared state for external querying
        let shared_state = Arc::new(RwLock::new(RunState::Created));

        // Monotonic per-Run sequence counter. Shared with the Run so that
        // RunCreated (seq 0) and the Run's own events form one sequence.
        let seq = Arc::new(AtomicU64::new(0));

        // Create the Run
        let run = Run::new(
            run_id.clone(),
            session_id.clone(),
            self.brain.clone(),
            model_config,
            cmd_rx,
            event_tx.clone(),
            seq.clone(),
            working_dir,
            history,
        )?;

        // Emit RunCreated event (seq 0 — the bootstrap event).
        let _ = event_tx.send(Envelope {
            seq: seq.fetch_add(1, Ordering::Relaxed),
            event_id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.clone(),
            turn_id: None,
            parent_call_id: None,
            event: RunEvent::RunCreated {
                id: run_id.clone(),
                session_id: session_id.clone(),
            },
        });

        // Spawn the Run's task
        let user_input_owned = user_input.to_string();
        let state_clone = shared_state.clone();
        let event_tx_clone = event_tx.clone();
        let log_run_id = run_id.clone();
        let event_tx_for_log = event_tx.clone();
        let brain_for_reflect = self.brain.clone();
        let reflect_run_id = run_id.clone();
        let join_handle = tokio::spawn(async move {
            // Logging subscriber: persists every Envelope to JSONL so that
            // replay/resync works. Runs independently of the Run task, ensuring
            // streaming events (which bypass Run::emit) are also logged (B3).
            let mut log_rx = event_tx_for_log.subscribe();
            let mut event_log = EventLog::new(&log_run_id, &default_runs_dir());
            let log_task = tokio::spawn(async move {
                loop {
                    match log_rx.recv().await {
                        Ok(env) => {
                            let is_terminal = matches!(
                                env.event,
                                RunEvent::RunCompleted { .. }
                                    | RunEvent::RunCancelled { .. }
                                    | RunEvent::RunFailed { .. }
                            );
                            event_log.append(env);
                            if is_terminal {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(run_id = %log_run_id, lagged = n, "event log subscriber lagged");
                            continue;
                        }
                    }
                }
            });
            // We need to update shared_state as the Run progresses.
            // The Run owns its state internally, so we use a wrapper that
            // mirrors state changes via events.
            let mut rx = event_tx_clone.subscribe();
            let state_task = tokio::spawn(async move {
                while let Ok(ev) = rx.recv().await {
                    if let RunEvent::StateChanged { to, .. } = &ev.event {
                        let mut g = state_clone.write();
                        *g = *to;
                    }
                    if matches!(
                        ev.event,
                        RunEvent::RunCompleted { .. }
                            | RunEvent::RunCancelled { .. }
                            | RunEvent::RunFailed { .. }
                    ) {
                        break;
                    }
                }
            });

            run.run(&user_input_owned).await;
            // The state mirror task will exit when it sees the terminal event.
            // Give it a moment to process the last event.
            let _ = state_task.await;
            // Wait for the logging subscriber to flush remaining events.
            let _ = log_task.await;

            // Offline reflection: analyze the Run's event log for improvement
            // suggestions. Only runs if the Brain has a Reflector configured.
            if let Some(ref reflector) = brain_for_reflect.reflector {
                let log_path = std::path::PathBuf::from(default_runs_dir())
                    .join(format!("{reflect_run_id}.jsonl"));
                match Reflector::load_event_log(&log_path) {
                    Ok(events) => {
                        let suggestions = reflector.analyze(&events);
                        for sug in &suggestions {
                            match reflector.apply(sug) {
                                Ok(crate::reflector::SuggestionAction::Applied) => {
                                    tracing::info!(
                                        suggestion = %sug.id,
                                        target = %sug.target,
                                        "reflector auto-applied skill"
                                    );
                                }
                                Ok(crate::reflector::SuggestionAction::NeedsApproval(diff)) => {
                                    tracing::info!(
                                        suggestion = %sug.id,
                                        "reflector suggestion needs approval: {diff}"
                                    );
                                }
                                Ok(crate::reflector::SuggestionAction::Forbidden) => {
                                    tracing::debug!(
                                        suggestion = %sug.id,
                                        "reflector suggestion forbidden"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        suggestion = %sug.id,
                                        "reflector apply error: {e}"
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("reflector failed to load event log: {e}");
                    }
                }
            }
        });

        // Store the handle
        let handle = RunHandle {
            id: run_id.clone(),
            session_id,
            cmd_tx,
            event_tx,
            join_handle: Some(join_handle),
            state: shared_state,
        };

        self.runs.lock().await.insert(run_id.clone(), handle);

        Ok(run_id)
    }

    /// Send a command to a specific Run.
    pub async fn command(&self, run_id: &str, cmd: RunCommand) -> Result<()> {
        let runs = self.runs.lock().await;
        let handle = runs
            .get(run_id)
            .with_context(|| format!("run '{run_id}' not found"))?;
        handle.command(cmd)
    }

    /// Subscribe to a Run's event stream.
    pub async fn subscribe(&self, run_id: &str) -> Result<broadcast::Receiver<Envelope>> {
        let runs = self.runs.lock().await;
        let handle = runs
            .get(run_id)
            .with_context(|| format!("run '{run_id}' not found"))?;
        Ok(handle.subscribe())
    }

    /// Get the current state of a Run.
    pub async fn run_state(&self, run_id: &str) -> Result<RunState> {
        let runs = self.runs.lock().await;
        let handle = runs
            .get(run_id)
            .with_context(|| format!("run '{run_id}' not found"))?;
        Ok(handle.state())
    }

    /// List all active Run IDs.
    pub async fn list_runs(&self) -> Vec<RunId> {
        self.runs.lock().await.keys().cloned().collect()
    }

    /// Cancel a specific Run.
    pub async fn cancel_run(&self, run_id: &str) -> Result<()> {
        self.command(run_id, RunCommand::Cancel).await
    }

    /// Cancel all active Runs (used on app shutdown).
    pub async fn cancel_all(&self) {
        let runs = self.runs.lock().await;
        for handle in runs.values() {
            let _ = handle.command(RunCommand::Cancel);
        }
    }

    /// Remove completed Runs from the tracking map (garbage collection).
    pub async fn reap_completed(&self) -> usize {
        let mut runs = self.runs.lock().await;
        let before = runs.len();
        runs.retain(|_, h| !h.is_done());
        before - runs.len()
    }

    /// List all Run IDs that have persisted event logs (for replay/fork).
    pub fn list_logged_runs(&self) -> Result<Vec<RunId>> {
        let dir = crate::paths::get_runs_dir();
        EventLog::list_runs(&dir.to_string_lossy())
    }

    /// Load a persisted Run's event log for replay.
    pub fn load_run_log(&self, run_id: &str) -> Result<Vec<Envelope>> {
        let path = crate::paths::get_runs_dir().join(format!("{run_id}.jsonl"));
        EventLog::load(&path)
    }

    /// Replay envelopes with `seq > from_seq` from a Run's log (resync).
    ///
    /// Used by the frontend to recover events lost to broadcast lag (B2).
    pub fn replay_since(&self, run_id: &str, from_seq: u64) -> Result<Vec<Envelope>> {
        let path = crate::paths::get_runs_dir().join(format!("{run_id}.jsonl"));
        EventLog::replay_since(&path, from_seq)
    }

    /// Create a Run in an isolated git worktree.
    ///
    /// This creates a new git worktree with a fresh branch, then creates a
    /// Run with `working_dir` set to the worktree path. The Run's tools
    /// (bash, file operations) execute in the worktree, not the main repo.
    ///
    /// When the Run completes (or is cancelled), the worktree is NOT
    /// automatically removed — the caller should inspect the result first
    /// and call `cleanup_worktree()` when done.
    pub async fn create_run_in_worktree(
        &self,
        user_input: &str,
        session_id: Option<String>,
        repo_root: &str,
        branch_name: &str,
        history: Vec<crate::types::Message>,
    ) -> Result<(RunId, String)> {
        let mut wt = WorktreeManager::new(std::path::PathBuf::from(repo_root));
        let record = wt.create(&uuid::Uuid::new_v4().to_string(), branch_name)?;
        let worktree_path = record.path.to_string_lossy().to_string();

        let run_id = self
            .create_run_with_workdir(user_input, session_id, Some(worktree_path.clone()), history)
            .await?;

        Ok((run_id, worktree_path))
    }

    /// Remove a git worktree by path. Called after a worktree-isolated
    /// Run has been inspected and the caller is done with it.
    pub fn cleanup_worktree(&self, repo_root: &str, worktree_path: &str) -> Result<()> {
        let mut wt = WorktreeManager::new(std::path::PathBuf::from(repo_root));
        // Find the worktree record by path
        let target_id = wt
            .list_all()
            .iter()
            .find(|r| r.path.to_string_lossy() == worktree_path)
            .map(|r| r.id.clone());
        if let Some(id) = target_id {
            wt.remove(&id)?;
            return Ok(());
        }
        anyhow::bail!("worktree not found: {worktree_path}")
    }

    /// Switch the active model for future Runs.
    pub fn switch_model(&mut self, name: &str) -> Result<()> {
        let brain_mut = Arc::make_mut(&mut self.brain);
        brain_mut.switch_model(name)
    }

    /// Update the active configuration.
    pub fn update_config(&mut self, config: crate::config::Config) -> Result<()> {
        let brain_mut = Arc::make_mut(&mut self.brain);
        brain_mut.update_config(config);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn test_config() -> Config {
        let toml = r#"
default_model = "test/default"

[providers.test]
name = "test"
base_url = "http://127.0.0.1:1"
api_key = "sk-test"

[providers.test.models]
default = { model_id = "mock" }
"#;
        let mut config: Config = toml::from_str(toml).unwrap();
        config.rebuild_models();
        config
    }

    #[tokio::test]
    async fn create_run_returns_id() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);
        let run_id = manager.create_run("hello", None, vec![]).await.unwrap();
        assert!(!run_id.is_empty());
    }

    #[tokio::test]
    async fn list_runs_shows_created_run() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);
        let _id = manager.create_run("hello", None, vec![]).await.unwrap();
        let runs = manager.list_runs().await;
        assert_eq!(runs.len(), 1);
    }

    #[tokio::test]
    async fn cancel_run_sends_command() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);
        let run_id = manager.create_run("hello", None, vec![]).await.unwrap();
        // Cancel before start should work
        manager.cancel_run(&run_id).await.unwrap();
    }

    #[tokio::test]
    async fn command_to_nonexistent_fails() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);
        let result = manager.command("nonexistent", RunCommand::Cancel).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_run_with_workdir() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);
        let run_id = manager
            .create_run_with_workdir("hello", None, Some("/tmp".to_string()), vec![])
            .await
            .unwrap();
        assert!(!run_id.is_empty());
    }

    #[tokio::test]
    async fn multiple_concurrent_runs() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);

        // Create two runs — they should coexist
        let id1 = manager.create_run("task 1", None, vec![]).await.unwrap();
        let id2 = manager.create_run("task 2", None, vec![]).await.unwrap();

        let runs = manager.list_runs().await;
        assert_eq!(runs.len(), 2);
        assert!(runs.contains(&id1));
        assert!(runs.contains(&id2));
    }

    #[tokio::test]
    async fn cancel_all_cancels_every_run() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);

        let _id1 = manager.create_run("task 1", None, vec![]).await.unwrap();
        let _id2 = manager.create_run("task 2", None, vec![]).await.unwrap();
        assert_eq!(manager.list_runs().await.len(), 2);

        manager.cancel_all().await;
        // After cancel_all, runs are still tracked (they transition to Cancelled)
        // but they should eventually be done.
    }
}
