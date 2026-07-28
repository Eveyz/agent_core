//! Shared, wakeable steering mailbox for a live Run.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::runtime::command::SteerEntry;

#[derive(Debug)]
pub enum SteerAcceptError<E> {
    Closed,
    Publish(E),
}

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

    /// Atomically queue a steer, publish its acceptance, then interrupt the
    /// active turn. Holding the queue lock through `publish` prevents a turn
    /// boundary from injecting the steer before `SteerQueued` is observable.
    pub fn accept<F, E>(&self, entry: SteerEntry, publish: F) -> Result<usize, SteerAcceptError<E>>
    where
        F: FnOnce(usize) -> Result<(), E>,
    {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(SteerAcceptError::Closed);
        }
        let mut state = self.inner.state.lock();
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(SteerAcceptError::Closed);
        }
        state.queue.push_back(entry);
        let depth = state.queue.len();
        if let Err(error) = publish(depth) {
            state.queue.pop_back();
            return Err(SteerAcceptError::Publish(error));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::command::RunCommand;
    use std::time::Duration;

    fn entry() -> SteerEntry {
        SteerEntry {
            id: "s1".into(),
            message: RunCommand::steer_message("change course"),
            raw_text: "change course".into(),
            timestamp: 0,
        }
    }

    #[test]
    fn drain_cannot_overtake_acceptance_publication() {
        let controller = SteeringController::new(CancellationToken::new());
        let accepting = controller.clone();
        let draining = controller.clone();
        let (publish_started_tx, publish_started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (drained_tx, drained_rx) = std::sync::mpsc::channel();

        let accept_thread = std::thread::spawn(move || {
            accepting
                .accept(entry(), |_| {
                    publish_started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok::<(), ()>(())
                })
                .unwrap();
        });
        publish_started_rx.recv().unwrap();
        let drain_thread = std::thread::spawn(move || {
            drained_tx.send(draining.drain().len()).unwrap();
        });

        assert!(
            drained_rx.recv_timeout(Duration::from_millis(20)).is_err(),
            "drain must wait until SteerQueued publication commits"
        );
        release_tx.send(()).unwrap();
        assert_eq!(drained_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        accept_thread.join().unwrap();
        drain_thread.join().unwrap();
    }
}
