use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
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
    slots: Arc<Mutex<HashMap<String, Arc<AgentSlot>>>>,
}

struct AgentSlot {
    permit: Arc<Semaphore>,
    active: Mutex<Option<ActiveAgentRun>>,
}

struct ActiveAgentRun {
    run_id: String,
    lane: AgentRunLane,
    cancel: CancellationToken,
    preempted: Arc<AtomicBool>,
}

pub struct ActiveAgentRunLease {
    slot: Arc<AgentSlot>,
    run_id: Option<String>,
    preempted: Arc<AtomicBool>,
    permit: Option<OwnedSemaphorePermit>,
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
        let run_id = run_id.into();
        let mut lease = self.reserve(agent_id).await;
        lease.activate(run_id, lane, cancel);
        lease
    }

    /// Reserves an agent's single turn without exposing a cancellable run yet.
    /// The dispatcher uses this to choose the highest-priority durable task
    /// only after the recipient lane is available.
    pub(crate) async fn reserve(&self, agent_id: impl Into<String>) -> ActiveAgentRunLease {
        let agent_id = agent_id.into();
        let slot = self
            .slots
            .lock()
            .entry(agent_id)
            .or_insert_with(|| {
                Arc::new(AgentSlot {
                    permit: Arc::new(Semaphore::new(1)),
                    active: Mutex::new(None),
                })
            })
            .clone();
        let permit = slot
            .permit
            .clone()
            .acquire_owned()
            .await
            .expect("agent run semaphore is never closed");
        let preempted = Arc::new(AtomicBool::new(false));
        ActiveAgentRunLease {
            slot,
            run_id: None,
            preempted,
            permit: Some(permit),
        }
    }

    pub fn route_peer_message(&self, agent_id: &str, priority: bool) -> PeerMessageRoute {
        if !priority {
            return PeerMessageRoute::Queued;
        }
        let Some(slot) = self.slots.lock().get(agent_id).cloned() else {
            return PeerMessageRoute::Queued;
        };
        let active = slot.active.lock();
        let Some(active) = active.as_ref() else {
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
}

impl ActiveAgentRunLease {
    pub(crate) fn activate(
        &mut self,
        run_id: impl Into<String>,
        lane: AgentRunLane,
        cancel: CancellationToken,
    ) {
        let run_id = run_id.into();
        *self.slot.active.lock() = Some(ActiveAgentRun {
            run_id: run_id.clone(),
            lane,
            cancel,
            preempted: self.preempted.clone(),
        });
        self.run_id = Some(run_id);
    }

    pub fn was_preempted(&self) -> bool {
        self.preempted.load(Ordering::Acquire)
    }

    pub fn finish(mut self) {
        self.release();
    }

    fn release(&mut self) {
        let mut active = self.slot.active.lock();
        if self.run_id.as_ref().is_some_and(|run_id| {
            active
                .as_ref()
                .is_some_and(|active| active.run_id == *run_id)
        }) {
            *active = None;
        }
        drop(active);
        self.permit.take();
    }
}

impl Drop for ActiveAgentRunLease {
    fn drop(&mut self) {
        self.release();
    }
}
