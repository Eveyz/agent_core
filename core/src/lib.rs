pub mod agent_registry;
pub mod workflow;
pub mod background;
pub mod client;
pub mod compressor;
pub mod config;
pub mod context;
pub mod context_processor;
pub mod cron;
pub mod eval;
pub mod error_recovery;
pub mod hooks;
pub mod hygiene;
pub mod mcp;
pub mod memory;
pub mod mode;
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
pub mod worktree;

pub use context_processor::{ContextProcessor, TransformContextFn};
pub use config::{ApiMode, Config, MemoryConfig, MemoryMode, ModelConfig, ReflectionConfig, RuntimeOverrides, resolve_env_value};
pub use context::{CacheHint, Context, ContextEngine, ContextSegment, RefreshPolicy, Stability};
pub use memory::{
    LifecycleReport, MemoryCategory, MemoryManager, MemoryStats, RecallIntent, SalienceConfig,
    SalienceScorer, ScoredRecord,
};
pub use mode::AgentMode;
pub use tokio_util::sync::CancellationToken;
pub use tools::{Tool, ToolRegistry, ToolUpdateFn, build_tool_by_name};
pub use types::{
    AgentEvent, AgentState, EventReceiver, EventSender, FunctionCall, FunctionSchema, Message,
    MessageDelta, ReasoningState, Role, StreamEvent, ToolCall, ToolDefinition, ToolExecutionMode,
    ToolResultRecord,
};

// New harness modules
pub use background::{BackgroundPool, Notification};
pub use compressor::{CompressionResult, Compressor, SummarizeRequest, TurnSummary};
pub use cron::{CronJob, CronJobRun, CronjobStore};
pub use eval::{
    collect_ledger, load_trace_jsonl, matrix_from_summaries, render_report_md, resolve_suite_dir,
    run_suite, summarize_suite, write_matrix, write_report, CollectOpts, EvalMode, EvalRunOptions,
    HarnessConfig, ModelInfo, RunLedger, SuiteSummary, HARNESS_FAIL_TAGS,
};
pub use error_recovery::{RecoveryAction, RecoveryContext, RecoveryEngine};
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
pub use project::{Project, ProjectManager};
pub use reflector::{Digester, Reflector, Suggestion, SuggestionAction, SuggestionKind};
pub use runtime::{
    ApprovalResolver, Brain, ChildId, ClarificationAnswers, ClarificationOption,
    ClarificationQuestion, ClarificationRequest, Envelope, EventGuard, EventLog, ExecutionPhase,
    ExecutionState, InputResolver, ProcessSupervisor, Run, RunCommand, RunEvent, RunHandle, RunId,
    RunManager, RunState, STEER_MID_RUN_PREFIX, SteerEntry, SupervisedChild,
};
pub use session::{
    Prompt, Session, SessionCounts, SessionManager, SessionMeta, SubagentResultLike,
};
pub use skills::{SkillLoader, SkillManager, SkillManifest};
pub use subagent::{Subagent, SubagentConfig, SubagentManager, SubagentResult};
pub use tasks::{TaskBoard, TaskRecord, TaskStatus};
pub use teams::{AgentTeam, MessageBus, TeamMessage, TeamMessageType};
pub use todo::{TodoItem, TodoList, TodoStatus};
pub use trace::TraceCollector;
pub use worktree::{WorktreeManager, WorktreeRecord, WorktreeStatus};
pub use rusqlite;
