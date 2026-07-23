use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkflowRevisionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeKey(pub String);

impl From<&str> for NodeKey {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSpec {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub nodes: Vec<NodeSpec>,
    pub result: ValueExpr,
    #[serde(default)]
    pub policy: WorkflowPolicy,
}

fn default_schema_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSpec {
    pub key: NodeKey,
    pub kind: NodeKind,
    #[serde(default)]
    pub inputs: BTreeMap<String, ValueExpr>,
    #[serde(default)]
    pub after: Vec<NodeKey>,
    #[serde(default)]
    pub retry: RetryPolicy,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub effect: EffectPolicy,
    #[serde(default)]
    pub resources: Vec<ResourceClaim>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    Activity {
        kind: String,
        #[serde(default)]
        config: Value,
    },
    Output,
    Choice {
        #[serde(default)]
        config: Value,
    },
    WaitSignal {
        name: String,
    },
    Timer {
        delay_ms: u64,
    },
    ChildWorkflow {
        revision_id: WorkflowRevisionId,
    },
    ForEach {
        #[serde(default)]
        config: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ValueExpr {
    Literal {
        value: Value,
    },
    RunInput {
        #[serde(default)]
        pointer: String,
    },
    NodeOutput {
        node: NodeKey,
        #[serde(default)]
        pointer: String,
    },
    Object {
        fields: BTreeMap<String, ValueExpr>,
    },
    Array {
        items: Vec<ValueExpr>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowPolicy {
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
    #[serde(default)]
    pub on_failure: FailurePolicy,
}

impl Default for WorkflowPolicy {
    fn default() -> Self {
        Self {
            max_concurrency: default_max_concurrency(),
            on_failure: FailurePolicy::Abort,
        }
    }
}

fn default_max_concurrency() -> usize {
    3
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    #[default]
    Abort,
    Continue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default)]
    pub backoff_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            backoff_ms: 0,
        }
    }
}

fn default_max_attempts() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectPolicy {
    Pure,
    #[default]
    ReadOnly,
    WorkspaceWrite,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceClaim {
    pub resource: String,
    #[serde(default)]
    pub exclusive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowSource {
    Inline(WorkflowSpec),
    Published(WorkflowRevisionId),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunScope {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub parent_run_id: String,
    #[serde(default)]
    pub parent_prompt_id: String,
    #[serde(default)]
    pub parent_tool_call_id: String,
    #[serde(default)]
    pub invocation_id: String,
    #[serde(default)]
    pub continuation_key: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub trigger: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRun {
    pub request_id: String,
    pub source: WorkflowSource,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub scope: RunScope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartReceipt {
    pub run_id: RunId,
    pub created: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowCommand {
    Signal {
        command_id: String,
        name: String,
        payload: Value,
    },
    Pause {
        command_id: String,
    },
    Resume {
        command_id: String,
    },
    Cancel {
        command_id: String,
        reason: String,
    },
}

impl WorkflowCommand {
    pub fn command_id(&self) -> &str {
        match self {
            Self::Signal { command_id, .. }
            | Self::Pause { command_id }
            | Self::Resume { command_id }
            | Self::Cancel { command_id, .. } => command_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandReceipt {
    pub accepted: bool,
    pub sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObserveRun {
    pub run_id: RunId,
    #[serde(default)]
    pub after_sequence: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunObservation {
    pub snapshot: RunSnapshot,
    pub events: Vec<WorkflowEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Waiting,
    Paused,
    Succeeded,
    Failed,
    Cancelled,
    NeedsAttention,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::NeedsAttention
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Pending,
    Scheduled,
    Running,
    Succeeded,
    Failed,
    Waiting,
    NeedsAttention,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub status: NodeStatus,
    pub attempt: u32,
    #[serde(default)]
    pub output: Value,
    #[serde(default)]
    pub artifacts: Vec<ArtifactRef>,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub run_id: RunId,
    pub request_id: String,
    pub status: RunStatus,
    pub last_sequence: u64,
    pub nodes: BTreeMap<NodeKey, NodeSnapshot>,
    #[serde(default)]
    pub output: Value,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEvent {
    pub run_id: RunId,
    pub sequence: u64,
    pub created_at: String,
    pub kind: WorkflowEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowEventKind {
    RunCreated,
    RunStarted,
    NodeScheduled {
        node: NodeKey,
        node_instance_id: String,
        attempt_id: String,
        attempt: u32,
        effect_key: String,
    },
    AttemptStarted {
        node: NodeKey,
        attempt_id: String,
    },
    NodeCompleted {
        node: NodeKey,
        output: Value,
        #[serde(default)]
        artifacts: Vec<ArtifactRef>,
    },
    NodeFailed {
        node: NodeKey,
        error: String,
        retryable: bool,
    },
    RetryScheduled {
        node: NodeKey,
        next_attempt: u32,
        /// Absolute UTC deadline. Empty values from older histories mean
        /// "retry immediately".
        #[serde(default)]
        retry_at: String,
    },
    NodeWaiting {
        node: NodeKey,
        signal: String,
    },
    TimerScheduled {
        node: NodeKey,
        fire_at: String,
    },
    TimerFired {
        node: NodeKey,
        fired_at: String,
    },
    NodeNeedsAttention {
        node: NodeKey,
        reason: String,
    },
    SignalReceived {
        command_id: String,
        name: String,
        payload: Value,
    },
    SignalConsumed {
        node: NodeKey,
        command_id: String,
        name: String,
    },
    RunPaused {
        command_id: String,
    },
    RunResumed {
        command_id: String,
    },
    RunCompleted {
        output: Value,
    },
    RunFailed {
        error: String,
    },
    RunCancelled {
        command_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub id: String,
    pub media_type: String,
    pub sha256: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunManifest {
    pub program_hash: String,
    pub program: WorkflowSpec,
    #[serde(default)]
    pub adapter_versions: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRun {
    pub run_id: RunId,
    pub request_id: String,
    pub manifest: RunManifest,
    pub input: Value,
    pub scope: RunScope,
}
