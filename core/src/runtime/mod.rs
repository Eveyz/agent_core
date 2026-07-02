//! Agent Runtime Management System.
//!
//! Provides independent, isolated execution spaces ("Runs") for agent requests,
//! with full lifecycle management, process supervision, and clean teardown.
//!
//! ## Architecture
//!
//! ```text
//! RunManager (owns Brain, tracks Runs)
//!   ├── create_run() → RunId
//!   ├── command(run_id, cmd)
//!   └── subscribe(run_id) → event stream
//!
//! Brain (reusable, shared across Runs)
//!   ├── client factory
//!   ├── tool factory
//!   └── memory / skills (shared)
//!
//! Run (per-request, independent)
//!   ├── state machine (Created → Running → Completed/Cancelled/Failed)
//!   ├── ProcessSupervisor (kills all child processes on cancel/drop)
//!   ├── JoinSet (aborts all background tasks on cancel/drop)
//!   └── event log (append-only)
//! ```
//!
//! ## Cleanup guarantee
//!
//! Three layers ensure no leaks:
//! 1. Normal completion → explicit terminal transition
//! 2. Cancel → `cancel_and_cleanup()` kills processes + aborts tasks
//! 3. Drop (RAII) → supervisor + join_set + cancel all fire automatically

pub mod approval;
pub mod brain;
pub mod command;
pub mod event;
pub mod event_log;
pub mod guard;
pub mod manager;
pub mod run;
pub mod state;
pub mod supervisor;

pub use approval::ApprovalResolver;
pub use brain::Brain;
pub use command::{RunCommand, SteerEntry};
pub use event::{ChildId, Envelope, RunEvent, RunId};
pub use event_log::EventLog;
pub use guard::EventGuard;
pub use manager::{RunHandle, RunManager};
pub use run::Run;
pub use state::RunState;
pub use supervisor::{ProcessSupervisor, SupervisedChild};
