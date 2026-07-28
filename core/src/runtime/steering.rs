//! Shared, wakeable steering mailbox for a live Run.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::runtime::command::SteerEntry;

#[derive(Clone)]
pub struct SteeringController {
    inner: Arc<SteeringInner>,
}

struct SteeringInner {
    lifetime_cancel: CancellationToken,
    state: Mutex<SteeringState>,
    closed: AtomicBool,
}

struct SteeringState {
    queue: VecDeque<SteerEntry>,
    turn_cancel: CancellationToken,
}

impl SteeringController {
    pub fn new(lifetime_cancel: CancellationToken) -> Self {
        let turn_cancel = lifetime_cancel.child_token();
        Self {
            inner: Arc::new(SteeringInner {
                lifetime_cancel,
                state: Mutex::new(SteeringState {
                    queue: VecDeque::new(),
                    turn_cancel,
                }),
                closed: AtomicBool::new(false),
            }),
        }
    }

    pub fn enqueue(&self, entry: SteerEntry) -> Result<usize, SteerEntry> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(entry);
        }
        let mut state = self.inner.state.lock();
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(entry);
        }
        state.queue.push_back(entry);
        let depth = state.queue.len();
        state.turn_cancel.cancel();
        Ok(depth)
    }

    pub fn begin_turn(&self) -> CancellationToken {
        let mut state = self.inner.state.lock();
        let token = self.inner.lifetime_cancel.child_token();
        if !state.queue.is_empty() {
            token.cancel();
        }
        state.turn_cancel = token.clone();
        token
    }

    pub fn turn_token(&self) -> CancellationToken {
        self.inner.state.lock().turn_cancel.clone()
    }

    pub fn drain(&self) -> Vec<SteerEntry> {
        self.inner.state.lock().queue.drain(..).collect()
    }

    pub fn cancel_pending(&self, steer_id: &str) -> bool {
        let mut state = self.inner.state.lock();
        let before = state.queue.len();
        state.queue.retain(|entry| entry.id != steer_id);
        state.queue.len() != before
    }

    pub fn clear(&self) {
        self.inner.state.lock().queue.clear();
    }

    pub fn close(&self) -> Vec<SteerEntry> {
        self.inner.closed.store(true, Ordering::Release);
        let mut state = self.inner.state.lock();
        state.turn_cancel.cancel();
        state.queue.drain(..).collect()
    }
}
