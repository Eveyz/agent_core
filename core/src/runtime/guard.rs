//! RAII guards that guarantee every `Start` event is paired with an `End`
//! event, even when the code between them returns `Err` early or panics.
//!
//! This structurally eliminates orphaned spinners / dangling states on the
//! frontend (the "Thinking..." bug). Instead of remembering to emit the End
//! event on every error path, the guard emits it automatically in `Drop`.

use std::sync::Arc;

/// Callback invoked when the guard drops without being explicitly completed.
type OnDrop<E> = Arc<dyn Fn() -> E + Send + Sync>;

/// RAII guard that emits a terminal event on drop if not completed.
///
/// Construct it right after emitting a `Start` event. Call `.complete()`
/// (and emit the normal `End` event yourself) on the success path. If the
/// function returns `?` early or panics, `Drop` fires the `on_incomplete`
/// closure, which should emit the `End(Error)` event.
pub struct EventGuard<E> {
    completed: bool,
    on_incomplete: OnDrop<E>,
}

impl<E> EventGuard<E> {
    /// Create a guard. `on_incomplete` is called in `Drop` **only if**
    /// `complete()` was never called.
    pub fn new<F>(on_incomplete: F) -> Self
    where
        F: Fn() -> E + Send + Sync + 'static,
    {
        Self {
            completed: false,
            on_incomplete: Arc::new(on_incomplete),
        }
    }

    /// Mark the guard as successfully completed so `Drop` becomes a no-op.
    pub fn complete(&mut self) {
        self.completed = true;
    }
}

impl<E> Drop for EventGuard<E> {
    fn drop(&mut self) {
        if !self.completed {
            (self.on_incomplete)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn fires_on_drop_when_not_completed() {
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        {
            let _guard = EventGuard::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
            });
            // dropped here without complete()
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn does_not_fire_when_completed() {
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        {
            let mut guard = EventGuard::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
            });
            guard.complete();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn fires_on_early_return() {
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        fn might_fail(c: Arc<AtomicUsize>) -> Result<(), ()> {
            let _guard = EventGuard::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
            });
            Err(())?; // early return — guard drops without complete()
            Ok(())
        }
        let _ = might_fail(counter.clone());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
