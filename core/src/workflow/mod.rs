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
pub mod runtime;
pub mod trust;
pub mod validate;

pub use context::{RouterConfig, RouterRule, WorkflowContext};
pub use definition::{
    EdgeDef, NodeDef, NodeType, OnNodeFailure, TrustMode, WorkflowDef, WorkflowRun,
    WorkflowRunNodeResult, create, create_run, delete, finish_run, get, get_run_node_results, list,
    list_runs, record_node_result, save, set_node_status,
};
pub use executor::{WorkflowExecutor, WorkflowRunResult};
pub use planner::{ExecutionPlan, Stage, plan};
pub use runtime::{
    ActivityAdapter, ActivityDescriptor, ActivityInvocation, ActivityOutcome,
    DurableWorkflowRuntime, InMemoryWorkflowStore, NodeKind as RuntimeNodeKind,
    NodeSpec as RuntimeNodeSpec, ObserveRun, RunId as DurableRunId, RunObservation, RunScope,
    RunStatus as DurableRunStatus, StartReceipt, StartRun, ValueExpr, WorkflowCommand,
    WorkflowPolicy, WorkflowRuntime, WorkflowSource, WorkflowSpec,
};
pub use validate::{Severity, ValidationIssue, ValidationResult, validate};
