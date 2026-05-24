pub mod agent;
pub mod background;
pub mod client;
pub mod comprehensive;
pub mod config;
pub mod context;
pub mod cron;
pub mod error_recovery;
pub mod hooks;
pub mod mcp;
pub mod memory;
pub mod permission;
pub mod prompt;
pub mod skills;
pub mod subagent;
pub mod tasks;
pub mod teams;
pub mod todo;
pub mod tools;
pub mod types;
pub mod worktree;

pub use agent::{Agent, AgentBuilder};
pub use config::{Config, MemoryConfig, ModelConfig, RuntimeOverrides};
pub use context::Context;
pub use memory::MemoryManager;
pub use tools::{Tool, ToolRegistry, ToolUpdateFn};
pub use types::{
    AgentEvent, AgentState, FunctionCall, FunctionSchema, Message, MessageDelta, Role, StreamEvent,
    ToolCall, ToolDefinition, ToolExecutionMode, ToolResultRecord,
};

// New harness modules
pub use background::{BackgroundPool, Notification};
pub use comprehensive::{ComprehensiveAgent, ComprehensiveAgentBuilder};
pub use cron::{CronJob, CronSchedule, CronScheduler};
pub use error_recovery::{RecoveryAction, RecoveryContext, RecoveryEngine};
pub use hooks::{Hook, HookAction, HookEvent, HookRegistry};
pub use mcp::{McpClient, McpServerConfig, McpTransport};
pub use permission::{ApprovalLevel, PermissionDecision, PermissionPolicy, PermissionRule};
pub use skills::{SkillLoader, SkillManifest};
pub use subagent::{Subagent, SubagentConfig, SubagentManager, SubagentResult};
pub use tasks::{TaskBoard, TaskRecord, TaskStatus};
pub use teams::{AgentTeam, MessageBus, TeamMessage, TeamMessageType};
pub use todo::{TodoItem, TodoList, TodoStatus};
pub use worktree::{WorktreeManager, WorktreeRecord, WorktreeStatus};
