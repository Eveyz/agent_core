//! Multi-agent workflow engine (PLAN-0009).
//!
//! A workflow is a persisted DAG of typed nodes (`agent`, `input`, `output`,
//! `transform`, `human_approval`) connected by edges. The [`planner`] turns the
//! graph into parallel stages; the [`executor`] runs them stage-by-stage,
//! routing between nodes via per-node routers and recording per-node results.
//!
//! This is a *static DAG workflow orchestrator* (closer to Dify/n8n than to
//! dynamic multi-agent orchestration): the topology is fixed before execution,
//! routing is explicit via router configs, and communication is structured JSON
//! state rather than free-form messages.

pub mod context;
pub mod definition;
pub mod executor;
pub mod planner;
pub mod trust;
pub mod validate;

pub use context::{RouterConfig, RouterRule, WorkflowContext};
pub use definition::{
    create, create_run, delete, finish_run, get, get_run_node_results, list, list_runs,
    record_node_result, save, set_node_status, EdgeDef, NodeDef, NodeType, OnNodeFailure,
    TrustMode, WorkflowDef, WorkflowRun, WorkflowRunNodeResult,
};
pub use executor::{WorkflowExecutor, WorkflowRunResult};
pub use planner::{plan, ExecutionPlan, Stage};
pub use validate::{validate, ValidationIssue, ValidationResult, Severity};
