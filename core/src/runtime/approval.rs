//! Per-Run approval channel.
//!
//! Replaces the global `global_pending_approvals()` map. Each Run owns its
//! own `PendingApprovalMap`. When a tool needs approval, the executor
//! inserts a `oneshot::Sender` into this map and emits an
//! `ApprovalRequired` event. The Run's command loop receives `Approve`
//! commands from the frontend and resolves them via
//! [`ApprovalResolver::resolve`].
//!
//! When the Run is cancelled or dropped, the map is cleared, and all
//! waiting `oneshot::Receiver`s get a `RecvError` — the executor treats
//! this as a denial, allowing clean shutdown.

use crate::permission::ApprovalChoice;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Type alias for the pending approval map: prompt_id → oneshot sender.
pub type PendingApprovalMap = HashMap<String, tokio::sync::oneshot::Sender<ApprovalChoice>>;

/// A shared, mutable map of pending approvals, scoped to a single Run.
///
/// Wrapped in `Arc<Mutex>` so the executor (which borrows `&self`) and the
/// Run's command loop (which borrows `&mut self`) can both access it without
/// borrow conflicts.
#[derive(Clone)]
pub struct ApprovalResolver {
    inner: Arc<Mutex<PendingApprovalMap>>,
    automatic_choice: Option<ApprovalChoice>,
}

impl ApprovalResolver {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            automatic_choice: None,
        }
    }

    /// Build a resolver for unattended execution. Any tool call that would
    /// normally block for approval is denied immediately.
    pub fn auto_deny() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            automatic_choice: Some(ApprovalChoice::Deny),
        }
    }

    /// Insert a pending approval (called by the executor).
    pub fn insert(&self, prompt_id: String, tx: tokio::sync::oneshot::Sender<ApprovalChoice>) {
        if let Some(choice) = &self.automatic_choice {
            let _ = tx.send(choice.clone());
            return;
        }
        self.inner.lock().insert(prompt_id, tx);
    }

    /// Remove a pending approval without resolving it (cleanup).
    pub fn remove(&self, prompt_id: &str) {
        self.inner.lock().remove(prompt_id);
    }

    /// Resolve a pending approval with the user's choice (called by the Run
    /// command loop). Returns `true` if the approval was found and resolved.
    pub fn resolve(&self, prompt_id: &str, choice: ApprovalChoice) -> bool {
        let mut map = self.inner.lock();
        if let Some(tx) = map.remove(prompt_id) {
            let _ = tx.send(choice);
            return true;
        }
        false
    }

    /// Drop all pending approvals (called on cancel / drop).
    /// The waiting receivers will get `RecvError`, which the executor
    /// interprets as a denial.
    pub fn clear(&self) {
        self.inner.lock().clear();
    }

    /// Number of pending approvals.
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    /// Whether there are no pending approvals.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for ApprovalResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn insert_and_resolve() {
        let resolver = ApprovalResolver::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        resolver.insert("p1".into(), tx);
        assert_eq!(resolver.len(), 1);

        let resolved = resolver.resolve("p1", ApprovalChoice::AllowOnce);
        assert!(resolved);
        assert_eq!(resolver.len(), 0);

        let choice = rx.await.unwrap();
        assert!(matches!(choice, ApprovalChoice::AllowOnce));
    }

    #[tokio::test]
    async fn clear_drops_senders() {
        let resolver = ApprovalResolver::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        resolver.insert("p1".into(), tx);
        resolver.clear();
        assert!(resolver.is_empty());
        // Receiver should get an error (sender dropped)
        assert!(rx.await.is_err());
    }

    #[test]
    fn resolve_nonexistent_returns_false() {
        let resolver = ApprovalResolver::new();
        assert!(!resolver.resolve("nope", ApprovalChoice::Deny));
    }

    #[tokio::test]
    async fn auto_deny_resolver_never_leaves_a_waiter() {
        let resolver = ApprovalResolver::auto_deny();
        let (tx, rx) = tokio::sync::oneshot::channel();
        resolver.insert("p1".into(), tx);
        assert!(resolver.is_empty());
        assert!(matches!(rx.await, Ok(ApprovalChoice::Deny)));
    }

    #[test]
    fn clone_shares_state() {
        let resolver = ApprovalResolver::new();
        let cloned = resolver.clone();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        resolver.insert("p1".into(), tx);
        // Clone sees the same map
        assert_eq!(cloned.len(), 1);
    }
}
