//! Durable workflow runtime.
//!
//! This module is intentionally separate from the legacy canvas executor. It
//! owns durable orchestration while the existing agent runtime continues to own
//! a single agent's model/tool loop.

mod activity;
mod authoring;
mod authoring_tool;
mod custom_agent;
mod engine;
mod legacy;
mod mention;
mod model;
mod reducer;
mod store;
mod tool;

pub use activity::{
    ActivityAdapter, ActivityDescriptor, ActivityInvocation, ActivityOutcome, ActivityRegistry,
    RecoveryDisposition,
};
pub use authoring::{
    AgentBinding, ApplyWorkflowDraft, DraftStep, InlineAgentBlueprint, PublishWorkflowDraft,
    PublishedWorkflowReceipt, WorkflowAuthoringService, WorkflowDraftReceipt, WorkflowDraftRecord,
    WorkflowDraftSpec,
};
pub use authoring_tool::{
    WorkflowApplyDraftTool, WorkflowCatalogTool, WorkflowPreviewTool, WorkflowPublishTool,
    workflow_authoring_tool_factory,
};
pub use custom_agent::{
    AgentHandoff, CUSTOM_AGENT_ACTIVITY_KIND, CustomAgentActivityAdapter, FrozenCustomAgentConfig,
};
pub use engine::{DurableWorkflowRuntime, WorkflowRuntime};
pub use legacy::LegacyWorkflowCompiler;
pub use mention::{
    AgentMention, MentionManifest, MentionPlan, MentionTask, MentionWorkflowCompiler,
    derive_mention_request_id,
};
pub use model::*;
pub use store::{InMemoryWorkflowStore, SqliteWorkflowStore, WorkflowStore};
pub use tool::MentionWorkflowTool;
