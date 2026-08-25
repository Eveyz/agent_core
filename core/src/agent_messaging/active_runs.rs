use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunLane {
    User,
    Peer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerMessageRoute {
    Queued,
    DeferredForUser { run_id: String },
    PreemptedPeer { run_id: String },
}

#[derive(Clone, Default)]
pub struct ActiveAgentRuns {
    inner: Arc<ActiveAgentRunsInner>,
}

#[derive(Default)]
struct ActiveAgentRunsInner {
    runs: Mutex<HashMap<String, ActiveAgentRun>>,
    changed: Notify,
}

struct ActiveAgentRun {
    run_id: String,
    lane: AgentRunLane,
    cancel: CancellationToken,
    preempted: Arc<AtomicBool>,
}

pub struct ActiveAgentRunLease {
    registry: ActiveAgentRuns,
    agent_id: String,
    run_id: String,
    preempted: Arc<AtomicBool>,
    finished: bool,
}

impl ActiveAgentRuns {
    pub fn new() -> Self {
        Self::default()
    }

    /// Waits until the agent has no active turn, then owns its single run slot.
    pub async fn enter(
        &self,
        agent_id: impl Into<String>,
        run_id: impl Into<String>,
        lane: AgentRunLane,
        cancel: CancellationToken,
    ) -> ActiveAgentRunLease {
        let agent_id = agent_id.into();
        let run_id = run_id.into();
        loop {
            let changed = self.inner.changed.notified();
            let acquired = {
                let mut runs = self.inner.runs.lock();
                if runs.contains_key(&agent_id) {
                    None
                } else {
                    let preempted = Arc::new(AtomicBool::new(false));
                    runs.insert(
                        agent_id.clone(),
                        ActiveAgentRun {
                            run_id: run_id.clone(),
                            lane,
                            cancel: cancel.clone(),
                            preempted: preempted.clone(),
                        },
                    );
                    Some(preempted)
                }
            };
            if let Some(preempted) = acquired {
                return ActiveAgentRunLease {
                    registry: self.clone(),
                    agent_id,
                    run_id,
                    preempted,
                    finished: false,
                };
            }
            changed.await;
        }
    }

    pub fn route_peer_message(&self, agent_id: &str, priority: bool) -> PeerMessageRoute {
        if !priority {
            return PeerMessageRoute::Queued;
        }
        let runs = self.inner.runs.lock();
        let Some(active) = runs.get(agent_id) else {
            return PeerMessageRoute::Queued;
        };
        match active.lane {
            AgentRunLane::User => PeerMessageRoute::DeferredForUser {
                run_id: active.run_id.clone(),
            },
            AgentRunLane::Peer => {
                active.preempted.store(true, Ordering::Release);
                active.cancel.cancel();
                PeerMessageRoute::PreemptedPeer {
                    run_id: active.run_id.clone(),
                }
            }
        }
    }

    fn finish(&self, agent_id: &str, run_id: &str) {
        let mut runs = self.inner.runs.lock();
        if runs
            .get(agent_id)
            .is_some_and(|active| active.run_id == run_id)
        {
            runs.remove(agent_id);
            drop(runs);
            self.inner.changed.notify_waiters();
        }
    }
}

impl ActiveAgentRunLease {
    pub fn was_preempted(&self) -> bool {
        self.preempted.load(Ordering::Acquire)
    }

    pub fn finish(mut self) {
        self.registry.finish(&self.agent_id, &self.run_id);
        self.finished = true;
    }
}

impl Drop for ActiveAgentRunLease {
    fn drop(&mut self) {
        if !self.finished {
            self.registry.finish(&self.agent_id, &self.run_id);
        }
    }
}
