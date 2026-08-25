pub mod agent_messaging;
pub mod agent_swarm;
pub mod agent_registry;
pub mod attachments;
pub mod background;
pub mod client;
pub mod compressor;
pub mod config;
pub mod context;
pub mod context_processor;
pub mod cron;
pub mod error_recovery;
pub mod eval;
pub mod hooks;
pub mod hygiene;
pub mod mcp;
pub mod memory;
pub mod mode;
pub mod model_capabilities;
pub mod paths;
pub mod permission;
pub mod project;
pub mod prompt;
pub mod reflector;
pub mod runtime;
pub mod session;
pub mod skills;
pub mod subagent;
pub mod tasks;
pub mod teams;
pub mod todo;
pub mod tools;
pub mod trace;
pub mod types;
pub mod util;
pub mod workflow;
pub mod worktree;

pub use agent_messaging::dispatcher::{AgentInboxDispatcher, AgentMessageExecutor};
pub use agent_messaging::{
    AGENT_MESSAGE_SCHEMA_V1, AgentConversation, AgentMessage, AgentMessageEvent, AgentMessageTask,
    AgentMessaging, AgentTaskCommand, AgentTaskStatus, ClaimedAgentMessage, DeliveryReceipt,
    ActiveAgentRunLease, ActiveAgentRuns, AgentRunLane, MAX_AGENT_MESSAGE_HOPS, MessageKind,
    MessageObservation, MessagePart, PeerMessageRoute, SendAgentMessage,
};
pub use agent_swarm::{
    CompleteSwarmTool, SendAgentMessageTool, StartSwarm, SwarmCommand, SwarmCoordinator,
    SwarmEvent, SwarmObservation, SwarmRun, SwarmSnapshot, SwarmStatus, SwarmToolContext,
    register_swarm_tools,
};
pub use config::{
    ApiMode, Config, MemoryConfig, MemoryMode, ModelConfig, ReflectionConfig, RuntimeOverrides,
    default_config_path, default_scaffold_config, load_or_init_default, resolve_env_value,
};
pub use context::{
    CacheHint, Context, ContextEngine, ContextSegment, ContextSegmentUsage, ContextUsageSnapshot,
    RefreshPolicy, Stability,
};
pub use context_processor::{ContextProcessor, TransformContextFn};
pub use memory::{
    LifecycleReport, MemoryCategory, MemoryManager, MemoryStats, RecallIntent, SalienceConfig,
    SalienceScorer, ScoredRecord,
};
pub use mode::AgentMode;
pub use model_capabilities::{
    DEFAULT_CONTEXT_TOKENS, ModelCapabilities, format_context_label, lookup_capabilities,
    resolve_context_tokens, resolve_max_output_tokens, resolve_supports_images,
};
pub use tokio_util::sync::CancellationToken;
pub use tools::{Tool, ToolRegistry, ToolUpdateFn, build_tool_by_name};
pub use types::{
    AgentEvent, AgentState, EventReceiver, EventSender, FunctionCall, FunctionSchema,
    ImageAttachment, Message, MessageDelta, ReasoningState, Role, StreamEvent, ToolCall,
    ToolDefinition, ToolExecutionMode, ToolResultRecord,
};

// New harness modules
pub use attachments::{
    MAX_ATTACHMENT_BYTES, attachment_url, resolve_attachment_ref, reuse_session_image,
    save_session_image,
};
pub use compressor::{
    CompressionResult, Compressor, RollingSummary, SummarizeRequest, SummaryFiles, TurnSummary,
    merge_summary,
};
pub use cron::{CronJob, CronJobRun, CronjobStore};
pub use error_recovery::{RecoveryAction, RecoveryContext, RecoveryEngine};
pub use eval::{
    CollectOpts, EvalMode, EvalRunOptions, HARNESS_FAIL_TAGS, HarnessConfig, ModelInfo, RunLedger,
    SuiteSummary, collect_ledger, load_trace_jsonl, matrix_from_summaries, render_report_md,
    resolve_suite_dir, run_suite, summarize_suite, write_matrix, write_report,
};
pub use hooks::{Hook, HookAction, HookEvent, HookRegistry};
pub use mcp::{
    McpChannel, McpClientManager, McpConfig, McpServerConfig, McpTool, McpToolDef, McpTransport,
};
pub use permission::{
    ApprovalChoice, ApprovalLevel, ApprovalPrompt, ApprovalScope, AuditEntry, AuditLog, AuditStats,
    ConfigRule, DangerLevel, PermissionConfig, PermissionDecision, PermissionMode,
    PermissionPolicy, PermissionRule, RuleSource, ToolPermissionPattern, WhitelistEntry,
    WhitelistManager, is_destructive_command,
};
pub use project::{
    Project, ProjectManager, default_project_path, documents_dir, sanitize_project_folder_name,
};
pub use reflector::{Digester, Reflector, Suggestion, SuggestionAction, SuggestionKind};
pub use runtime::{
    ApprovalResolver, Brain, ChildId, ClarificationAnswers, ClarificationOption,
    ClarificationQuestion, ClarificationRequest, CreateRunResult, Envelope, EventGuard, EventLog,
    ExecutionPhase, ExecutionState, InputResolver, ProcessSupervisor, Run, RunCommand, RunEvent,
    RunHandle, RunId, RunManager, RunState, STEER_MID_RUN_PREFIX, SteerEntry, SupervisedChild,
};
pub use rusqlite;
pub use session::{
    Prompt, Session, SessionCounts, SessionManager, SessionMeta, SubagentResultLike,
};
pub use skills::{SkillDiagnostic, SkillLoader, SkillManager, SkillManifest, parse_skill_mentions};
pub use subagent::{Subagent, SubagentConfig, SubagentManager, SubagentResult};
pub use tasks::{TaskBoard, TaskRecord, TaskStatus};
pub use teams::{AgentTeam, MessageBus, TeamMessage, TeamMessageType};
pub use todo::{
    ContinueResolution, ParkedPlanSummary, Plan, PlanDetail, PlanStatus, PlansSnapshot,
    ResumeTarget, SessionPlanStore, SessionTodoStore, TodoItem, TodoList, TodoStatus,
    is_bare_continue, is_continue_cue, is_explicit_plan_resume, is_object_bearing_continue,
    parse_resume_target,
};
pub use trace::TraceCollector;
pub use worktree::{WorktreeManager, WorktreeRecord, WorktreeStatus};
