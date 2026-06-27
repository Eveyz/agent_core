pub mod agent;
pub mod background;
pub mod client;
pub mod comprehensive;
pub mod compressor;
pub mod config;
pub mod context;
pub mod cron;
pub mod error_recovery;
pub mod hooks;
pub mod hygiene;
pub mod mcp;
pub mod memory;
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
pub mod worktree;

// Legacy agent types — still used by the CLI. New code should use the
// `runtime` module (Brain + Run + RunManager) instead.
pub use agent::{Agent, AgentBuilder, ContextProcessor, TransformContextFn};
pub use config::{Config, MemoryConfig, MemoryMode, ModelConfig, ReflectionConfig, RuntimeOverrides, resolve_env_value};
pub use context::{CacheHint, Context, ContextEngine, ContextSegment, RefreshPolicy, Stability};
pub use memory::{
    MemoryCategory, MemoryManager, MemoryStats, SalienceConfig, SalienceScorer, ScoredRecord,
};
pub use tokio_util::sync::CancellationToken;
pub use tools::{Tool, ToolRegistry, ToolUpdateFn, build_tool_by_name};
pub use types::{
    AgentEvent, AgentState, EventReceiver, EventSender, FunctionCall, FunctionSchema, Message,
    MessageDelta, Role, StreamEvent, ToolCall, ToolDefinition, ToolExecutionMode, ToolResultRecord,
};

// New harness modules
pub use background::{BackgroundPool, Notification};
pub use comprehensive::{ComprehensiveAgent, ComprehensiveAgentBuilder, ReflectionReport};
pub use compressor::{CompressionResult, Compressor, SummarizeRequest, TurnSummary};
pub use cron::{CronJob, CronSchedule, CronScheduler};
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
    ApprovalResolver, Brain, ChildId, Envelope, EventGuard, EventLog, ProcessSupervisor, Run,
    RunCommand, RunEvent, RunHandle, RunId, RunManager, RunState, SupervisedChild,
};
pub use session::{
    EventLogEntry, Session, SessionCounts, SessionManager, SessionMeta, SubagentResultLike,
};
pub use skills::{SkillLoader, SkillManager, SkillManifest};
pub use subagent::{Subagent, SubagentConfig, SubagentManager, SubagentResult};
pub use tasks::{TaskBoard, TaskRecord, TaskStatus};
pub use teams::{AgentTeam, MessageBus, TeamMessage, TeamMessageType};
pub use todo::{TodoItem, TodoList, TodoStatus};
pub use trace::TraceCollector;
pub use worktree::{WorktreeManager, WorktreeRecord, WorktreeStatus};
