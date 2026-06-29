# PLAN-0009: User-Defined Agents & Multi-Agent Workflow System

```yaml
---
id: PLAN-0009
type: PLAN
title: User-Defined Agents & Multi-Agent Workflow System
status: Draft
author: agent_core (AI-generated, requires human review)
created: 2026-06-30
updated: 2026-06-30
reviewers: [zniverse]
related: [ADR-0001]
supersedes: ~
superseded_by: ~
tags: [agent, subagent, workflow, react-flow, multi-agent, memory, persistence]
---
```

## Objective

让用户能够自定义 Agent（名字、system prompt、model、skills、tools、permissions），并使用 React Flow 可视化画布将多个 Agent 拖拽组合成一个 Multi-Agent Workflow（类似 Google ADK）。每个自定义 Agent 拥有独立的 Memory 和 History，可以复用、持续进化。后端提供完整的数据库持久化，支持 Agent 间的 Context 传递，且不影响现有主 Agent 的运行。

## Background

### 现状分析

当前系统存在以下关键事实：

1. **`create_agent` 命令不存在**：`NewAgentModal.tsx:55` 调用了 `invoke("create_agent", ...)`，但 `app/src-tauri/src/lib.rs` 中并未注册该命令。该按钮点击后必定报错。
2. **Agent 不是持久化实体**：现有 Agent 是 `RunManager` → `Brain` → `Run` 的临时执行单元。Subagent 是运行时由 `subagent` / `subagents` tool 临时 spawn 的，不持久化到数据库，Run 结束即消失。
3. **Memory 是全局共享的**：`MemoryManager` 持有一个全局 `session_id`（UUID），所有 conversation 都写入同一张 `recall_memory` 表，以 `session_id` 区分。没有 "agent-specific memory" 的概念 —— 不存在 `agent_id` 字段。
4. **Skills 是文件系统级**：`SkillManager` 扫描文件系统目录中的 `SKILL.md`，不存储在数据库中。Skills 与 Agent 没有显式关联。
5. **Teams 系统存在但未接入**：`core/src/teams/` 有 `AgentTeam` + `MessageBus`（基于 `Arc<Mutex<HashMap>>` 的 inbox 模式），但当前没有任何代码使用它。
6. **前端无图/流库**：`package.json` 中没有 `react-flow` / `@xyflow/react` 或任何图可视化库。
7. **Subagent 架构已成熟**：`Subagent` struct 拥有独立的 `Context`、`ToolRegistry`、`PermissionPolicy`、`HookRegistry`，通过 `EventSender`（mpsc channel）与父 Agent 通信。这是一个良好的基础。

### 为什么现在做

- 用户已经看到 `NewAgentModal` UI，但功能不完整（后端缺失）。
- 系统已有 subagent、teams、memory 三层架构的基础设施，需要串联。
- Multi-Agent Workflow 是产品差异化的核心能力。

## Scope

### In Scope

- **Agent 持久化层**：数据库 schema 设计，存储自定义 Agent 定义（name、prompt、model、skills、tools、permissions）。
- **Agent Memory 隔离**：每个自定义 Agent 拥有独立的 Memory 和 History，跨 session 持久化，可复用、可进化。
- **Workflow 持久化层**：存储 React Flow 画布的节点/边/拓扑结构。
- **后端执行引擎**：将 Workflow DAG 编译为执行计划，调度多个 Agent 协作，管理 Context 在节点间传递。
- **前端 Agent CRUD**：修复 `NewAgentModal`，实现 Agent 列表/编辑/删除。
- **前端 Workflow Editor**：基于 React Flow 的可视化画布，拖拽 Agent 节点，连接边，配置传递关系。
- **不影响主 Agent**：所有新增功能是增量式扩展，主 Agent（`RunManager` → `Brain` → `Run`）的执行路径不变。

### Out of Scope

- **分布式多机调度**：当前为单机本地执行，不涉及跨机器 Agent 调度。
- **Agent 市场/共享**：不支持 Agent 的发布、下载、分享。
- **实时协作编辑**：不支持多人同时编辑同一个 Workflow。
- **Workflow 版本管理**：V1 不做 Workflow 的版本历史和 diff。
- **动态拓扑**：Workflow 运行时不支持动态增删节点（拓扑在运行前固定）。
- **重写主 Agent**：主 Agent 的执行路径保持不变，不迁移到新架构。

---

## 架构设计

### 设计原则

1. **增量扩展，不破坏现有**：所有新功能通过新增模块和新增数据库表实现。`Brain`、`Run`、`RunManager` 的现有接口不变。自定义 Agent 和 Workflow 是在现有 `Subagent` 架构之上的封装层。
2. **复用现有基础设施**：自定义 Agent 复用 `MemoryManager`、`SkillManager`、`ToolRegistry`、`PermissionPolicy`、`ContextEngine`。不重新造轮子。
3. **Agent 是一等公民**：Agent 有唯一的 `agent_id`，持久化到数据库，有自己的 Memory、History、配置。可以被多个 Workflow 复用。
4. **Workflow 是 DAG**：Workflow 是有向无环图，节点是 Agent（或特殊节点如 Input/Output/Human Approval），边定义 Context 传递方向。

### 整体架构图

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Frontend (React + Tauri)                      │
│                                                                      │
│  ┌──────────────┐  ┌──────────────────┐  ┌────────────────────────┐ │
│  │ Agent CRUD   │  │ Workflow Editor   │  │  Main Chat (不变)      │ │
│  │ (Modal/List) │  │ (React Flow)      │  │  RunManager → Run      │ │
│  └──────┬───────┘  └────────┬──────────┘  └────────────────────────┘ │
│         │                   │                                         │
└─────────┼───────────────────┼─────────────────────────────────────────┘
          │ invoke()           │ invoke()
┌─────────▼───────────────────▼─────────────────────────────────────────┐
│                      Tauri Backend (lib.rs)                           │
│                                                                      │
│  ┌─────────────────┐  ┌──────────────────┐  ┌──────────────────────┐ │
│  │ Agent Commands  │  │ Workflow Commands│  │  Existing Commands   │ │
│  │ (CRUD + memory) │  │ (CRUD + execute) │  │  (send_message, ...) │ │
│  └────────┬────────┘  └────────┬─────────┘  └──────────────────────┘ │
│           │                    │                                       │
└───────────┼────────────────────┼───────────────────────────────────────┘
            │                    │
┌───────────▼────────────────────▼───────────────────────────────────────┐
│                       Core Library (Rust)                              │
│                                                                        │
│  ┌──────────────────────────────────────────┐  ┌────────────────────┐  │
│  │        NEW: Agent Registry                │  │  Existing: Brain   │  │
│  │  ┌─────────────┐  ┌──────────────────┐   │  │  RunManager → Run  │  │
│  │  │ AgentDef    │  │ AgentMemoryStore │   │  │  Subagent          │  │
│  │  │ (SQLite)    │  │ (per-agent_id)   │   │  │  MemoryManager     │  │
│  │  └─────────────┘  └──────────────────┘   │  │  SkillManager      │  │
│  └──────────────────────────────────────────┘  │  ToolRegistry      │  │
│                                                │  PermissionPolicy  │  │
│  ┌──────────────────────────────────────────┐  │  ContextEngine     │  │
│  │     NEW: Workflow Engine                  │  │  SessionManager    │  │
│  │  ┌─────────────┐  ┌──────────────────┐   │  │  Teams/MessageBus  │  │
│  │  │ WorkflowDef │  │ ExecutionPlanner │   │  └────────────────────┘  │
│  │  │ (SQLite)    │  │ (DAG → Runs)     │   │                          │
│  │  └─────────────┘  └──────────────────┘   │                          │
│  └──────────────────────────────────────────┘                          │
│                                                                        │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │                    SQLite (~/.agverse/memory.db)                  │  │
│  │                                                                  │  │
│  │  EXISTING TABLES (不变):                                         │  │
│  │  memory_blocks, recall_memory, archival_memory,                  │  │
│  │  conversation_summaries, sessions, session_messages,             │  │
│  │  session_event_log, projects, cronjobs, cronjob_runs             │  │
│  │                                                                  │  │
│  │  NEW TABLES:                                                     │  │
│  │  agents, agent_memory, agent_history,                            │  │
│  │  workflows, workflow_nodes, workflow_edges,                      │  │
│  │  workflow_runs, workflow_run_node_results                        │  │
│  └──────────────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 数据库 Schema 设计

### 新增表（全部在现有 `~/.agverse/memory.db` 中，通过 idempotent CREATE TABLE IF NOT EXISTS）

### 1. `agents` — 自定义 Agent 定义

```sql
CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,                  -- UUID v4
    name TEXT NOT NULL,                   -- 显示名，如 "Code Reviewer"
    description TEXT NOT NULL DEFAULT '',  -- 简短描述
    system_prompt TEXT NOT NULL DEFAULT '',-- Agent 的 system prompt
    model TEXT NOT NULL DEFAULT '',        -- "provider/model" 格式，空则用全局 default_model
    skills TEXT NOT NULL DEFAULT '[]',     -- JSON array of skill names
    tools TEXT NOT NULL DEFAULT '[]',      -- JSON array of tool names (显式指定)
    permission_mode TEXT NOT NULL DEFAULT 'standard',  -- paranoid/standard/developer/permissive/yolo
    permission_rules TEXT NOT NULL DEFAULT '[]',       -- JSON: 自定义 permission rules
    max_iterations INTEGER NOT NULL DEFAULT 50,
    max_context_tokens INTEGER NOT NULL DEFAULT 32000,
    memory_enabled INTEGER NOT NULL DEFAULT 1,  -- 0=stateless, 1=standard, 2=deep
    icon TEXT NOT NULL DEFAULT '',               -- 前端图标标识
    color TEXT NOT NULL DEFAULT '',              -- 前端节点颜色
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agents_name ON agents(name);
```

**设计说明**：
- `skills` / `tools` / `permission_rules` 用 JSON 字符串存储，避免额外的关联表。Agent 数量不会很大（几十个级别），JSON 解析开销可忽略。
- `memory_enabled` 三档对应现有 `MemoryMode`（Stateless/Standard/Deep）。
- `model` 为空时 fallback 到 `config.default_model`，允许 Agent 不绑定特定模型。
- `permission_mode` + `permission_rules` 允许每个 Agent 有独立的权限策略，例如 "Code Reviewer" 可以是 readonly，"Builder" 可以是 full access。

### 2. `agent_memory` — 每个 Agent 的持久化记忆

```sql
CREATE TABLE IF NOT EXISTS agent_memory (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    role TEXT NOT NULL,                    -- "user" / "assistant" / "system"
    content TEXT NOT NULL,
    embedding BLOB,                        -- 384-dim f32 LE bytes (同 recall_memory)
    importance REAL DEFAULT 0.5,
    memory_strength REAL DEFAULT 1.0,
    access_count INTEGER DEFAULT 0,
    last_accessed_at TEXT,
    category TEXT DEFAULT 'Conversation',  -- Conversation/Decision/Code/Preference/Trivia
    source TEXT DEFAULT 'conversation',    -- conversation / reflection / workflow
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_memory_agent ON agent_memory(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_memory_created ON agent_memory(created_at);

CREATE VIRTUAL TABLE IF NOT EXISTS agent_memory_fts USING fts5(content, tokenize='unicode61');

-- FTS 触发器（同 recall_memory 的模式）
CREATE TRIGGER IF NOT EXISTS agent_memory_fts_ai AFTER INSERT ON agent_memory BEGIN
    INSERT INTO agent_memory_fts(rowid, content) VALUES (new.rowid, new.content);
END;
CREATE TRIGGER IF NOT EXISTS agent_memory_fts_ad AFTER DELETE ON agent_memory BEGIN
    DELETE FROM agent_memory_fts WHERE rowid = old.rowid;
END;
CREATE TRIGGER IF NOT EXISTS agent_memory_fts_au AFTER UPDATE ON agent_memory BEGIN
    DELETE FROM agent_memory_fts WHERE rowid = old.rowid;
    INSERT INTO agent_memory_fts(rowid, content) VALUES (new.rowid, new.content);
END;
```

**设计说明**：
- 与 `recall_memory` 结构几乎一致，但以 `agent_id` 替代 `session_id` 作为隔离维度。
- 复用 `SalienceScorer`、`EmbeddingModel`、`BM25Index`、`HNSWIndex` 的逻辑（代码层面泛化，不是复制粘贴）。
- `source` 字段区分记忆来源：普通对话、reflection 自动提取、workflow 执行产出。
- 每个 Agent 拥有独立的 Memory，意味着 "Code Reviewer" Agent 记住的所有代码审查经验，在每次被调用时都可以检索到。
- **不共享**：Agent A 的 Memory 对 Agent B 不可见（除非通过 Workflow 的 Context 传递机制显式传递）。

### 3. `agent_history` — Agent 执行历史

```sql
CREATE TABLE IF NOT EXISTS agent_history (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,              -- 关联到 sessions 表
    workflow_run_id TEXT DEFAULT '',       -- 如果是 workflow 执行产生的，关联到 workflow_runs
    trigger TEXT NOT NULL DEFAULT 'manual',-- manual / workflow / cronjob
    input TEXT NOT NULL,                   -- 用户/上游传入的输入
    output TEXT NOT NULL DEFAULT '',       -- Agent 最终输出
    iterations_used INTEGER DEFAULT 0,
    success INTEGER NOT NULL DEFAULT 1,
    model_used TEXT NOT NULL DEFAULT '',
    process_time_ms INTEGER DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_history_agent ON agent_history(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_history_session ON agent_history(session_id);
CREATE INDEX IF NOT EXISTS idx_agent_history_workflow ON agent_history(workflow_run_id);
```

**设计说明**：
- 每次 Agent 被调用（无论手动还是 workflow 内）都记录一条历史。
- `session_id` 复用现有 `sessions` 表，完整对话消息存在 `session_messages` 中，这里只存摘要。
- `trigger` 区分来源：手动调用、Workflow 触发、Cronjob 触发。
- 这使得 Agent "越用越强"：可以回顾历史，看到自己之前做了什么，成功还是失败。

### 4. `workflows` — Workflow 定义

```sql
CREATE TABLE IF NOT EXISTS workflows (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    input_schema TEXT NOT NULL DEFAULT '{}',   -- JSON: 输入参数定义
    output_schema TEXT NOT NULL DEFAULT '{}',  -- JSON: 输出格式定义
    config TEXT NOT NULL DEFAULT '{}',         -- JSON: 全局配置（max_concurrent, timeout, etc.）
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_workflows_name ON workflows(name);
```

### 5. `workflow_nodes` — Workflow 中的节点

```sql
CREATE TABLE IF NOT EXISTS workflow_nodes (
    id TEXT PRIMARY KEY,                   -- UUID
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    node_type TEXT NOT NULL,               -- agent / input / output / human_approval / transform
    agent_id TEXT DEFAULT '',              -- node_type='agent' 时关联到 agents 表
    label TEXT NOT NULL DEFAULT '',        -- 节点显示名
    position_x REAL NOT NULL DEFAULT 0,    -- React Flow 画布坐标
    position_y REAL NOT NULL DEFAULT 0,
    config TEXT NOT NULL DEFAULT '{}',     -- JSON: 节点特定配置
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_workflow_nodes_workflow ON workflow_nodes(workflow_id);
```

**节点类型（`node_type`）**：
| 类型 | 说明 | config 内容 |
|------|------|-------------|
| `input` | Workflow 输入入口 | 输入变量定义 |
| `output` | Workflow 输出口 | 输出格式定义 |
| `agent` | Agent 执行节点 | agent_id, 输入模板, 输出变量名 |
| `human_approval` | 人工审核节点 | 审核提示语, 超时行为 |
| `transform` | 数据转换节点 | Jinja2/Handlebars 模板, 或 JS 表达式 |

### 6. `workflow_edges` — Workflow 中的边

```sql
CREATE TABLE IF NOT EXISTS workflow_edges (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    source_node_id TEXT NOT NULL REFERENCES workflow_nodes(id) ON DELETE CASCADE,
    target_node_id TEXT NOT NULL REFERENCES workflow_nodes(id) ON DELETE CASCADE,
    source_handle TEXT DEFAULT '',          -- 输出端口名（多输出节点）
    target_handle TEXT DEFAULT '',          -- 输入端口名（多输入节点）
    label TEXT NOT NULL DEFAULT '',         -- 边的标签/条件
    condition TEXT NOT NULL DEFAULT '',     -- 条件表达式（空=无条件传递）
    data_mapping TEXT NOT NULL DEFAULT '{}',-- JSON: 上下文映射规则
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_workflow_edges_workflow ON workflow_edges(workflow_id);
CREATE INDEX IF NOT EXISTS idx_workflow_edges_source ON workflow_edges(source_node_id);
CREATE INDEX IF NOT EXISTS idx_workflow_edges_target ON workflow_edges(target_node_id);
```

**设计说明**：
- `data_mapping` 定义 Context 如何从上游传递到下游，例如 `{"output": "review_result", "pass_through": true}`。
- `condition` 支持简单的条件表达式，例如 `output.success == true`，用于条件分支。
- React Flow 的边数据直接映射到这张表。

### 7. `workflow_runs` — Workflow 执行记录

```sql
CREATE TABLE IF NOT EXISTS workflow_runs (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,               -- 关联到 sessions 表
    status TEXT NOT NULL DEFAULT 'pending', -- pending/running/completed/failed/cancelled
    input TEXT NOT NULL DEFAULT '{}',       -- JSON: 实际输入
    output TEXT NOT NULL DEFAULT '{}',      -- JSON: 最终输出
    error TEXT NOT NULL DEFAULT '',
    started_at TEXT NOT NULL,
    finished_at TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_workflow_runs_workflow ON workflow_runs(workflow_id);
CREATE INDEX IF NOT EXISTS idx_workflow_runs_status ON workflow_runs(status);
```

### 8. `workflow_run_node_results` — Workflow 执行中每个节点的结果

```sql
CREATE TABLE IF NOT EXISTS workflow_run_node_results (
    id TEXT PRIMARY KEY,
    workflow_run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    node_id TEXT NOT NULL,
    agent_history_id TEXT DEFAULT '',       -- 关联到 agent_history（如果节点是 agent 类型）
    status TEXT NOT NULL DEFAULT 'pending', -- pending/running/completed/failed/skipped
    input TEXT NOT NULL DEFAULT '{}',       -- JSON: 该节点收到的输入
    output TEXT NOT NULL DEFAULT '{}',      -- JSON: 该节点的输出
    error TEXT NOT NULL DEFAULT '',
    started_at TEXT,
    finished_at TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_workflow_run_nodes ON workflow_run_node_results(workflow_run_id);
CREATE INDEX IF NOT EXISTS idx_workflow_run_nodes_node ON workflow_run_node_results(node_id);
```

### 表关系 ER 图

```
agents (1) ──────────< agent_memory (N)
agents (1) ──────────< agent_history (N)
agents (1) ──────────< workflow_nodes (N) [node_type='agent']

workflows (1) ───────< workflow_nodes (N)
workflows (1) ───────< workflow_edges (N)
workflows (1) ───────< workflow_runs (N)

workflow_nodes (1) ──< workflow_edges (N) [as source]
workflow_nodes (1) ──< workflow_edges (N) [as target]

workflow_runs (1) ───< workflow_run_node_results (N)
workflow_run_node_results (N) >── agent_history (1) [optional]

sessions (1) ────────< agent_history (N)
sessions (1) ────────< workflow_runs (N)
```

---

## 后端架构设计

### 新增模块结构

```
core/src/
├── agent_registry/          ← NEW: Agent 定义 CRUD + Agent Memory 管理
│   ├── mod.rs               ← AgentRegistry 协调层
│   ├── definition.rs        ← AgentDef struct + DB 操作
│   ├── memory.rs            ← AgentMemoryStore (per-agent memory 隔离)
│   └── history.rs           ← AgentHistoryStore (执行历史)
├── workflow/                ← NEW: Workflow 引擎
│   ├── mod.rs               ← WorkflowEngine 协调层
│   ├── definition.rs        ← WorkflowDef + NodeDef + EdgeDef + DB 操作
│   ├── planner.rs           ← DAG → 执行计划 (拓扑排序 + 并行分组)
│   ├── executor.rs          ← 执行计划 → 逐节点运行 (复用 Subagent/Run)
│   └── context.rs           ← Context 传递机制 (节点间数据流转)
├── runtime/                 ← EXISTING: 不修改
├── memory/                  ← EXISTING: 不修改，但 AgentMemoryStore 复用其模式
├── subagent/                ← EXISTING: 不修改，Workflow executor 复用 Subagent
└── ...
```

### 1. Agent Registry (`core/src/agent_registry/`)

#### `AgentDef` — Agent 定义（对应 `agents` 表）

```rust
pub struct AgentDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub model: String,           // "provider/model" or "" for default
    pub skills: Vec<String>,
    pub tools: Vec<String>,      // empty = inherit all available tools
    pub permission_mode: String, // paranoid/standard/developer/permissive/yolo
    pub permission_rules: serde_json::Value,
    pub max_iterations: usize,
    pub max_context_tokens: usize,
    pub memory_enabled: u8,      // 0/1/2
    pub icon: String,
    pub color: String,
    pub created_at: String,
    pub updated_at: String,
}
```

#### `AgentRegistry` — CRUD + 实例化

```rust
pub struct AgentRegistry {
    storage: Storage,  // 复用现有 SQLite Storage
}

impl AgentRegistry {
    // CRUD
    pub fn create(&self, def: AgentDef) -> Result<AgentDef>;
    pub fn get(&self, id: &str) -> Result<AgentDef>;
    pub fn list(&self) -> Result<Vec<AgentDef>>;
    pub fn update(&self, id: &str, updates: &AgentDefUpdate) -> Result<AgentDef>;
    pub fn delete(&self, id: &str) -> Result<()>;

    // 实例化：从 AgentDef 构建 SubagentConfig + 运行时组件
    pub fn build_subagent_config(&self, def: &AgentDef, model_config: &ModelConfig) -> SubagentConfig;
    pub fn build_permission_config(&self, def: &AgentDef, base: &PermissionConfig) -> PermissionConfig;
}
```

**关键设计**：`AgentRegistry` 不直接执行 Agent，它只管理定义。执行由 `WorkflowExecutor` 或手动调用时，通过 `build_subagent_config()` 将 `AgentDef` 转换为现有 `SubagentConfig`，然后复用现有的 `Subagent::new()` + `Subagent::run_with_sender()` 执行。

#### `AgentMemoryStore` — Per-Agent Memory

```rust
pub struct AgentMemoryStore {
    storage: Storage,
    embedding_model: Option<EmbeddingModel>,
    salience_scorer: SalienceScorer,
    // Per-agent in-memory indexes (lazy-initialized)
    bm25_indexes: Mutex<HashMap<String, BM25Index>>,
    hnsw_indexes: Mutex<HashMap<String, HNSWIndex>>,
}

impl AgentMemoryStore {
    pub fn store(&self, agent_id: &str, role: &str, content: &str) -> Result<String>;
    pub fn search(&self, agent_id: &str, query: &str, top_k: usize) -> Result<Vec<ScoredRecord>>;
    pub fn search_hybrid(&self, agent_id: &str, query: &str, top_k: usize) -> Result<Vec<ScoredRecord>>;
    pub fn consolidate(&self, agent_id: &str) -> Result<ConsolidationReport>;
    pub fn prune_cold(&self, agent_id: &str) -> Result<usize>;
    pub fn stats(&self, agent_id: &str) -> Result<MemoryStats>;
    pub fn ensure_indexes(&self, agent_id: &str);  // lazy init BM25/HNSW
}
```

**关键设计**：
- 与全局 `MemoryManager` 的接口几乎一致，但所有操作都以 `agent_id` 为前缀。
- BM25/HNSW 索引按 `agent_id` 分别构建，lazy-initialize（首次访问某 Agent 的 memory 时构建）。
- 复用 `SalienceScorer`、`EmbeddingModel`、`BM25Index`、`HNSWIndex` 的实现，不复制代码。
- **与全局 Memory 隔离**：全局 `recall_memory` 表和 `agent_memory` 表完全独立。主 Agent 的 Memory 不受影响。

#### `AgentHistoryStore` — 执行历史

```rust
pub struct AgentHistoryStore {
    storage: Storage,
}

impl AgentHistoryStore {
    pub fn record(&self, entry: AgentHistoryEntry) -> Result<String>;
    pub fn list(&self, agent_id: &str, limit: usize) -> Result<Vec<AgentHistoryEntry>>;
    pub fn get_recent(&self, agent_id: &str, n: usize) -> Result<Vec<AgentHistoryEntry>>;
}
```

### 2. Workflow Engine (`core/src/workflow/`)

#### `WorkflowDef` — Workflow 定义（对应 `workflows` + `workflow_nodes` + `workflow_edges`）

```rust
pub struct WorkflowDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub config: WorkflowConfig,
    pub nodes: Vec<NodeDef>,
    pub edges: Vec<EdgeDef>,
}

pub struct NodeDef {
    pub id: String,
    pub node_type: NodeType,  // Agent / Input / Output / HumanApproval / Transform
    pub agent_id: Option<String>,
    pub label: String,
    pub position: (f64, f64),
    pub config: serde_json::Value,
}

pub enum NodeType {
    Input,
    Output,
    Agent,
    HumanApproval,
    Transform,
}

pub struct EdgeDef {
    pub id: String,
    pub source: String,
    pub target: String,
    pub source_handle: String,
    pub target_handle: String,
    pub label: String,
    pub condition: String,
    pub data_mapping: serde_json::Value,
}
```

#### `WorkflowPlanner` — DAG → 执行计划

```rust
pub struct ExecutionPlan {
    pub stages: Vec<ExecutionStage>,  // 拓扑排序后的分阶段执行
}

pub struct ExecutionStage {
    pub nodes: Vec<String>,  // 同一 stage 内的节点可以并行执行
}

impl WorkflowPlanner {
    pub fn plan(workflow: &WorkflowDef) -> Result<ExecutionPlan> {
        // 1. 构建邻接表
        // 2. Kahn 算法拓扑排序
        // 3. 同层节点分组（无依赖关系的分到同一 stage）
        // 4. 检测环（有环则报错）
    }
}
```

**关键设计**：
- 使用 Kahn 算法进行拓扑排序，将 DAG 分为多个 stage。
- 同一 stage 内的节点无依赖关系，可以并行执行。
- 如果检测到环，返回错误（Workflow 必须是 DAG）。
- 条件边（`condition` 非空）在执行时动态评估，决定是否传递 Context。

#### `WorkflowExecutor` — 执行计划 → 逐节点运行

```rust
pub struct WorkflowExecutor {
    registry: Arc<AgentRegistry>,
    memory_store: Arc<AgentMemoryStore>,
    history_store: Arc<AgentHistoryStore>,
    brain: Arc<Brain>,  // 复用 Brain 的 config/client/skill_manager
    session_manager: Arc<Mutex<SessionManager>>,
}

impl WorkflowExecutor {
    pub async fn execute(
        &self,
        workflow: &WorkflowDef,
        input: serde_json::Value,
        session_id: &str,
        event_tx: broadcast::Sender<Envelope>,
    ) -> Result<serde_json::Value>;

    async fn execute_stage(
        &self,
        stage: &ExecutionStage,
        ctx: &WorkflowContext,
        event_tx: &broadcast::Sender<Envelope>,
    ) -> Result<()>;

    async fn execute_node(
        &self,
        node: &NodeDef,
        input: serde_json::Value,
        ctx: &WorkflowContext,
        event_tx: &broadcast::Sender<Envelope>,
    ) -> Result<serde_json::Value>;
}
```

**执行流程**：

```
1. WorkflowExecutor::execute(workflow, input)
   │
   ├── 创建 workflow_run 记录 (status=running)
   ├── 创建 WorkflowContext (存储所有节点的输入/输出)
   │
   ├── for stage in plan.stages:
   │   │
   │   ├── 并行执行 stage 内所有节点 (tokio::JoinSet)
   │   │   │
   │   │   ├── execute_node(node, input_from_upstream):
   │   │   │   │
   │   │   │   ├── NodeDef::Agent:
   │   │   │   │   ├── 从 AgentRegistry 获取 AgentDef
   │   │   │   │   ├── 构建 SubagentConfig (system_prompt, tools, max_iterations)
   │   │   │   │   ├── 构建 ToolRegistry (从 AgentDef.tools 过滤)
   │   │   │   │   ├── 构建 PermissionPolicy (从 AgentDef.permission_mode)
   │   │   │   │   ├── 检索 Agent Memory (agent_memory search) → 注入 context
   │   │   │   │   ├── 创建 Subagent::new() + run_with_sender(input)
   │   │   │   │   ├── 执行完成后：
   │   │   │   │   │   ├── 存储 conversation 到 agent_memory
   │   │   │   │   │   ├── 记录到 agent_history
   │   │   │   │   │   └── 返回 output
   │   │   │   │   │
   │   │   │   ├── NodeDef::Transform:
   │   │   │   │   ├── 执行模板渲染 (Jinja2/Handlebars)
   │   │   │   │   └── 返回渲染结果
   │   │   │   │
   │   │   │   ├── NodeDef::HumanApproval:
   │   │   │   │   ├── 发送 ApprovalRequired 事件
   │   │   │   │   ├── 等待用户审批 (oneshot channel)
   │   │   │   │   └── 返回 {approved: bool, comment: string}
   │   │   │   │
   │   │   │   ├── NodeDef::Input:
   │   │   │   │   └── 返回 workflow input
   │   │   │   │
   │   │   │   └── NodeDef::Output:
   │   │   │       └── 设置 workflow output
   │   │   │
   │   │   └── 收集所有节点输出 → 更新 WorkflowContext
   │   │
   │   └── 评估条件边 → 决定下游节点是否执行
   │
   ├── 更新 workflow_run (status=completed, output=...)
   └── 返回最终输出
```

#### `WorkflowContext` — 节点间 Context 传递

```rust
pub struct WorkflowContext {
    // 每个节点的输出
    node_outputs: RwLock<HashMap<String, serde_json::Value>>,
    // 全局共享上下文（所有节点可读）
    shared: RwLock<serde_json::Value>,
    // 输入参数
    input: serde_json::Value,
}

impl WorkflowContext {
    // 上游节点输出 → 下游节点输入
    pub fn resolve_input(
        &self,
        node_id: &str,
        edges: &[EdgeDef],
    ) -> serde_json::Value;

    // 节点执行完成后存储输出
    pub fn set_output(
        &self,
        node_id: &str,
        output: serde_json::Value,
    );
}
```

**Context 传递机制**：

```
┌───────────┐     edge(data_mapping)      ┌───────────┐
│  Node A   │ ──────────────────────────► │  Node B   │
│ (Agent)   │   output: {result: "..."}   │ (Agent)   │
└───────────┘                              └───────────┘

data_mapping = {
    "source_field": "output.result",     // 从 A 的 output.result 取值
    "target_field": "input.context",      // 放入 B 的 input.context
    "pass_through": true                  // 同时传递 A 的完整 output
}
```

- `data_mapping` 定义字段级别的映射规则。
- `pass_through: true` 时，上游的完整 output 作为 `context` 字段附加到下游 input。
- 多个上游节点连到同一下游时，inputs 合并为数组。
- 条件边：`condition` 字段求值为 `true` 时传递，`false` 时跳过下游节点。

### 3. 与现有系统的关系

| 现有组件 | 是否修改 | 如何复用 |
|---------|---------|---------|
| `RunManager` / `Brain` / `Run` | **不修改** | Workflow Executor 从 Brain 获取 config/client/skill_manager |
| `Subagent` | **不修改** | Workflow 中的 Agent 节点通过 `Subagent::new()` + `run_with_sender()` 执行 |
| `MemoryManager` | **不修改** | 主 Agent 的全局 Memory 保持不变。Agent Memory 是独立的新表 + 新 Store |
| `SkillManager` | **不修改** | Agent 节点执行时，Subagent 的 SkillManager 从 Brain 继承 |
| `ToolRegistry` | **不修改** | Agent 节点通过 `ToolRegistry::from_names(agent_def.tools)` 构建 |
| `PermissionPolicy` | **不修改** | Agent 节点通过 `PermissionConfig` + `agent_def.permission_mode` 构建 |
| `ContextEngine` | **不修改** | Subagent 内部的 ContextEngine 照常工作 |
| `SessionManager` | **不修改** | Workflow Run 创建 session，Agent 执行创建 sub-session（parent_session_id 链接） |
| `teams/MessageBus` | **可选复用** | V2 可考虑用 MessageBus 替代 WorkflowContext 做更灵活的 Agent 间通信 |
| `Storage` (SQLite) | **扩展** | 新增 8 张表，通过 `CREATE TABLE IF NOT EXISTS` idempotent 添加 |

---

## 前端架构设计

### 新增依赖

```json
{
  "dependencies": {
    "@xyflow/react": "^12.x.x"
  }
}
```

> 使用 `@xyflow/react`（React Flow v12，已更名）而非旧版 `reactflow`。这是 React 19 兼容的最新版本。

### 新增前端结构

```
app/src/
├── features/
│   ├── agents/                    ← NEW: Agent 管理状态
│   │   ├── agentSlice.ts          ← Redux slice: agents[], loading, error
│   │   ├── types.ts               ← AgentDef, AgentHistory, etc.
│   │   └── thunks.ts              ← fetchAgents, createAgent, updateAgent, deleteAgent
│   └── workflow/                  ← NEW: Workflow 管理状态
│       ├── workflowSlice.ts       ← Redux slice: workflows[], activeWorkflow, runState
│       ├── types.ts               ← WorkflowDef, NodeDef, EdgeDef, WorkflowRun
│       └── thunks.ts              ← fetchWorkflows, createWorkflow, executeWorkflow
├── components/
│   ├── agents/                    ← NEW: Agent 管理组件
│   │   ├── AgentList.tsx          ← Agent 列表侧边面板
│   │   ├── AgentEditor.tsx        ← Agent 编辑器（扩展 NewAgentModal）
│   │   └── AgentMemoryViewer.tsx  ← 查看 Agent Memory
│   └── workflow/                  ← NEW: Workflow 编辑器组件
│       ├── WorkflowEditor.tsx     ← React Flow 画布主组件
│       ├── WorkflowSidebar.tsx    ← Agent 节点拖拽面板
│       ├── AgentNode.tsx          ← 自定义 React Flow 节点
│       ├── InputNode.tsx          ← 输入节点
│       ├── OutputNode.tsx         ← 输出节点
│       ├── TransformNode.tsx      ← 数据转换节点
│       ├── ApprovalNode.tsx       ← 人工审核节点
│       ├── EdgeConfigPanel.tsx    ← 边配置面板（data_mapping）
│       └── WorkflowRunView.tsx    ← 运行时状态可视化
```

### Agent CRUD UI（修复 NewAgentModal）

扩展现有 `NewAgentModal.tsx`，增加以下字段：
- **Tools**：多选下拉（从 `ToolRegistry` 的可用 tools 列表选择）
- **Permission Mode**：下拉选择（paranoid/standard/developer/permissive/yolo）
- **Memory**：开关 + 模式选择（stateless/standard/deep）
- **Max Iterations**：数字输入
- **Max Context Tokens**：数字输入
- **Icon / Color**：图标和颜色选择器（用于 React Flow 节点样式）

保存时调用新增的 `create_agent` / `update_agent` Tauri command（后端实现）。

### React Flow Workflow Editor

#### 画布布局

```
┌─────────────────────────────────────────────────────────────────┐
│  Workflow Toolbar: [Save] [Run] [Validate] [Export]             │
├──────────┬──────────────────────────────────────────────────────┤
│          │                                                      │
│  Node    │              React Flow Canvas                       │
│  Palette │                                                      │
│          │   ┌────────┐     ┌────────────┐     ┌────────┐      │
│  > Input │   │ Input  │────►│  Agent A   │────►│ Output │      │
│  > Agent │   └────────┘     │ Code       │     └────────┘      │
│  > Trans │                  │ Reviewer   │                      │
│  > Apprv │                  └────────────┘                      │
│  > Output│                       │                              │
│          │                       ▼                              │
│  Agents: │                  ┌────────────┐                      │
│  [Code   │                  │  Agent B   │                      │
│   Revwr] │                  │  Security  │                      │
│  [Bldr]  │                  │  Scanner   │                      │
│  [Test]  │                  └────────────┘                      │
│          │                                                      │
├──────────┴──────────────────────────────────────────────────────┤
│  Node Config Panel (选中节点时显示)                               │
│  - Agent: Code Reviewer                                         │
│  - Input Template: {{upstream.output}}                          │
│  - Output Variable: review_result                               │
└─────────────────────────────────────────────────────────────────┘
```

#### 自定义节点组件 (`AgentNode.tsx`)

```tsx
function AgentNode({ data, id }: NodeProps<AgentNodeData>) {
  const status = data.runStatus; // idle / running / completed / failed
  return (
    <div className={`workflow-agent-node status-${status}`}>
      <div className="node-header" style={{ background: data.color }}>
        {data.icon && <data.icon size={14} />}
        <span>{data.label}</span>
      </div>
      <div className="node-body">
        <p className="node-desc">{data.description}</p>
        {data.runStatus === 'running' && <Spinner />}
        {data.runStatus === 'completed' && <CheckIcon size={12} />}
        {data.runStatus === 'failed' && <XIcon size={12} />}
      </div>
      {/* React Flow Handles (输入/输出端口) */}
      <Handle type="target" position={Position.Left} />
      <Handle type="source" position={Position.Right} />
    </div>
  );
}
```

#### 交互流程

1. **拖拽创建**：从左侧 Node Palette 拖拽 Agent 到画布 → 创建 `workflow_nodes` 记录。
2. **连线**：从节点输出端口拖到另一节点输入端口 → 创建 `workflow_edges` 记录。
3. **配置**：点击节点 → 右侧/底部面板显示配置项（input template、output variable、tools override 等）。
4. **保存**：点击 Save → 将 React Flow 的 nodes/edges 序列化 → 调用 `save_workflow` Tauri command → 写入数据库。
5. **验证**：点击 Validate → 后端执行 `WorkflowPlanner::plan()` → 检测环、孤立节点、缺失配置 → 返回错误列表。
6. **运行**：点击 Run → 调用 `execute_workflow` Tauri command → 后端创建 `workflow_runs` 记录 → 执行 → 前端通过 `agent-event` 流接收节点状态更新。

### 状态管理

#### `agentSlice.ts`

```typescript
interface AgentState {
  agents: AgentDef[];
  loading: boolean;
  error: string | null;
  selectedAgentId: string | null;
}

// Thunks: fetchAgents, createAgent, updateAgent, deleteAgent, getAgentMemory
```

#### `workflowSlice.ts`

```typescript
interface WorkflowState {
  workflows: WorkflowDef[];
  activeWorkflow: WorkflowDef | null;
  // React Flow nodes/edges (编辑时状态)
  nodes: Node[];
  edges: Edge[];
  // 运行时状态
  runState: WorkflowRunState | null;
  // 每个节点的运行状态
  nodeRunStates: Record<string, NodeRunStatus>;
}
```

---

## Tauri Commands 设计

### Agent CRUD Commands

```rust
#[tauri::command]
async fn create_agent(
    name: String,
    description: String,
    system_prompt: String,
    model: String,
    skills: Vec<String>,
    tools: Vec<String>,
    permission_mode: String,
    permission_rules: serde_json::Value,
    max_iterations: usize,
    max_context_tokens: usize,
    memory_enabled: u8,
    icon: String,
    color: String,
    state: State<'_, AppState>,
) -> Result<AgentDef, String>;

#[tauri::command]
async fn list_agents(state: State<'_, AppState>) -> Result<Vec<AgentDef>, String>;

#[tauri::command]
async fn get_agent(id: String, state: State<'_, AppState>) -> Result<AgentDef, String>;

#[tauri::command]
async fn update_agent(id: String, updates: serde_json::Value, state: State<'_, AppState>) -> Result<AgentDef, String>;

#[tauri::command]
async fn delete_agent(id: String, state: State<'_, AppState>) -> Result<(), String>;

#[tauri::command]
async fn search_agent_memory(agent_id: String, query: String, top_k: usize, state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, String>;

#[tauri::command]
async fn get_agent_history(agent_id: String, limit: usize, state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, String>;

#[tauri::command]
async fn run_agent_standalone(
    agent_id: String,
    input: String,
    session_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String>;  // returns run_id (复用 Run 事件流)
```

### Workflow CRUD Commands

```rust
#[tauri::command]
async fn create_workflow(name: String, description: String, state: State<'_, AppState>) -> Result<WorkflowDef, String>;

#[tauri::command]
async fn list_workflows(state: State<'_, AppState>) -> Result<Vec<WorkflowDef>, String>;

#[tauri::command]
async fn get_workflow(id: String, state: State<'_, AppState>) -> Result<WorkflowDef, String>;

#[tauri::command]
async fn save_workflow(
    id: String,
    name: String,
    description: String,
    nodes: serde_json::Value,   // React Flow nodes
    edges: serde_json::Value,   // React Flow edges
    config: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<WorkflowDef, String>;

#[tauri::command]
async fn delete_workflow(id: String, state: State<'_, AppState>) -> Result<(), String>;

#[tauri::command]
async fn validate_workflow(id: String, state: State<'_, AppState>) -> Result<ValidationResult, String>;

#[tauri::command]
async fn execute_workflow(
    id: String,
    input: serde_json::Value,
    session_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String>;  // returns workflow_run_id

#[tauri::command]
async fn get_workflow_run(run_id: String, state: State<'_, AppState>) -> Result<WorkflowRun, String>;

#[tauri::command]
async fn list_workflow_runs(workflow_id: String, limit: usize, state: State<'_, AppState>) -> Result<Vec<WorkflowRun>, String>;
```

---

## Context 传递机制详细设计

### 问题

Agent 间 Context 传递是 Multi-Agent System 的核心难题。需要决定：

1. **传什么**：完整的对话历史？仅最终输出？结构化数据？
2. **怎么传**：直接注入 system prompt？作为 user message？还是结构化 JSON？
3. **粒度**：字段级映射 vs 全量传递。

### 方案：结构化 JSON + 模板渲染

```
上游 Agent A 输出:
{
  "result": "代码审查完成，发现3个问题...",
  "issues": [...],
  "metadata": {"files_reviewed": 5, "time_ms": 12000}
}

       │
       │  Edge data_mapping:
       │  {
       │    "source_field": "result",
       │    "target_field": "context",
       │    "pass_through": false
       │  }
       │
       ▼

下游 Agent B 输入:
{
  "task": "根据审查结果修复代码",
  "context": "代码审查完成，发现3个问题..."  // ← 来自 Agent A
}
```

### Context 注入到 Agent 的方式

Agent 节点执行时，输入被组装为一条 user message：

```
## Task
{node.config.task_template 渲染后的内容}

## Context from Upstream
{上游传递的结构化数据，格式化为可读文本}

## Additional Instructions
{node.config.instructions}
```

这保持了与现有 `Subagent` 接口的兼容性 —— Subagent 的 `run(task: &str)` 只需要一个字符串输入。

### 多上游汇聚

当一个节点有多个上游时：

```
Agent A ──┐
          ├──► Agent C
Agent B ──┘
```

Agent C 的输入：

```
## Task
{C 的 task}

## Context from Upstream Agents

### From: Code Reviewer (Agent A)
{A 的输出}

### From: Security Scanner (Agent B)
{B 的输出}
```

### 条件分支

边上的 `condition` 字段支持简单表达式：

```
condition: "output.success == true"
```

执行时，对上游节点的 output JSON 求值。支持：
- `output.field == value`
- `output.field != value`
- `output.field contains "keyword"`
- `output.field > 100`

不满足条件的边不传递 Context，下游节点如果没有其他满足条件的上游边，则被标记为 `skipped`。

---

## Agent 进化机制

### "越用越强" 的实现

每个自定义 Agent 通过以下机制持续进化：

1. **Memory 积累**：每次执行后，对话存储到 `agent_memory` 表。下次执行前，检索相关记忆注入 context。
   - 例如 "Code Reviewer" 第 10 次审查代码时，可以回忆起前 9 次审查中发现的常见问题模式。

2. **History 回顾**：`agent_history` 记录每次执行的输入/输出/成功与否。可以在 system prompt 中注入最近 N 次执行摘要。
   - 例如 "你最近 5 次执行中，有 2 次因为遗漏边界条件而失败，请特别注意。"

3. **Memory 检索注入**：Agent 执行前，自动用 input 作为 query 检索 `agent_memory`，将 top-K 结果注入 context 的 Active Memory segment。
   - 复用现有 `SalienceScorer` + Ebbinghaus 衰减 + 重要性评分机制。

4. **Consolidation**：定期对 `agent_memory` 执行去重和升级（`consolidate`），将高频访问的记忆强化，冷记忆淘汰。
   - 复用现有 `MemoryConsolidator` 逻辑。

5. **Reflection**（Deep 模式）：如果 Agent 的 `memory_enabled = 2`（Deep），后台 `ReflectionDaemon` 自动从对话中提取事实，写入 `agent_memory`。
   - 复用现有 `ReflectionDaemon` 逻辑，但 target 表从 `recall_memory` 改为 `agent_memory`。

### 进化数据流

```
Agent 执行
  │
  ├── 执行前:
  │   ├── search agent_memory(input, top_k=5) → 相关记忆
  │   ├── get_recent_history(agent_id, n=3) → 最近执行摘要
  │   └── 注入到 Subagent 的 ContextEngine (Active Memory segment)
  │
  ├── 执行中:
  │   └── Subagent 正常运行 (LLM + tools + skills)
  │
  └── 执行后:
      ├── store agent_memory(role="user", content=input)
      ├── store agent_memory(role="assistant", content=output)
      ├── record agent_history(agent_id, input, output, success)
      └── if memory_enabled == 2: trigger ReflectionDaemon
```

---

## 与主 Agent 的隔离保证

### 隔离原则

| 维度 | 主 Agent | 自定义 Agent |
|------|---------|-------------|
| Memory 表 | `recall_memory` | `agent_memory` |
| Memory 索引 | 全局 BM25/HNSW | per-agent_id BM25/HNSW |
| Session | `sessions` (session_type='main') | `sessions` (session_type='subagent' 或新增 'custom_agent') |
| Config | `config.toml` 全局配置 | `agents` 表中的 per-agent 配置 |
| 执行路径 | `RunManager` → `Brain` → `Run` | `WorkflowExecutor` → `Subagent::new()` → `run_with_sender()` |
| 事件流 | `broadcast::Sender<Envelope>` | 同一个 broadcast（通过 `subagent_id` 区分） |
| 工具 | `ToolRegistry::with_defaults()` + 全部注册 | `ToolRegistry::from_names(agent_def.tools)` 过滤 |
| 权限 | 全局 `PermissionConfig` | per-agent `PermissionConfig` (mode + rules) |

### 不影响主 Agent 的具体措施

1. **数据库表隔离**：新增 8 张表，不修改现有 10 张表的 schema。所有新表的读写操作通过独立的 `AgentRegistry` / `AgentMemoryStore` / `WorkflowEngine`，不触碰 `MemoryManager` / `SessionManager` 的现有方法。
2. **执行路径隔离**：主 Agent 通过 `RunManager::create_run()` 执行。自定义 Agent 通过 `WorkflowExecutor::execute_node()` 执行，使用 `Subagent::new()` 构造，不创建新的 `Run`。
3. **Memory 隔离**：`agent_memory` 表以 `agent_id` 为隔离维度，与 `recall_memory` 的 `session_id` 完全独立。`AgentMemoryStore` 是独立 struct，不共享 `MemoryManager` 的状态。
4. **配置隔离**：自定义 Agent 的配置存在 `agents` 表中，不修改 `config.toml`。`AgentDef.model` 为空时 fallback 到全局 `default_model`，但不覆盖全局配置。
5. **事件流复用**：自定义 Agent 执行时复用现有 `broadcast::Sender<Envelope>` 事件流，事件通过 `subagent_id` 字段区分。前端已有 `SubagentStarted` / `SubagentEnded` 等事件处理逻辑，可直接复用。

---

## Tasks

### Phase 1: 后端基础设施（Agent 持久化 + Memory 隔离）

| ID | Task | Owner | Status | ETA |
|----|------|-------|--------|-----|
| T1 | 设计并实现 8 张新数据库表的 schema + migrations | agent_core | Todo | TBD |
| T2 | 实现 `AgentRegistry` (CRUD + `build_subagent_config` / `build_permission_config`) | agent_core | Todo | TBD |
| T3 | 实现 `AgentMemoryStore` (store / search / search_hybrid / consolidate) | agent_core | Todo | TBD |
| T4 | 实现 `AgentHistoryStore` (record / list / get_recent) | agent_core | Todo | TBD |
| T5 | 实现 Tauri Commands: `create_agent` / `list_agents` / `get_agent` / `update_agent` / `delete_agent` | agent_core | Todo | TBD |
| T6 | 实现 Tauri Commands: `search_agent_memory` / `get_agent_history` / `run_agent_standalone` | agent_core | Todo | TBD |
| T7 | 实现 Agent 执行时的 Memory 注入逻辑 (执行前检索 + 执行后存储) | agent_core | Todo | TBD |

### Phase 2: 后端 Workflow 引擎

| ID | Task | Owner | Status | ETA |
|----|------|-------|--------|-----|
| T8 | 实现 `WorkflowDef` + `NodeDef` + `EdgeDef` + DB CRUD | agent_core | Todo | TBD |
| T9 | 实现 `WorkflowPlanner` (Kahn 拓扑排序 + 环检测 + 并行分组) | agent_core | Todo | TBD |
| T10 | 实现 `WorkflowContext` (node_outputs + data_mapping 解析 + 条件边评估) | agent_core | Todo | TBD |
| T11 | 实现 `WorkflowExecutor` (stage 并行执行 + agent/transform/approval 节点) | agent_core | Todo | TBD |
| T12 | 实现 Tauri Commands: workflow CRUD + `execute_workflow` + `validate_workflow` | agent_core | Todo | TBD |
| T13 | Workflow 执行事件流 (复用 broadcast Envelope, 新增 `WorkflowNodeStarted` / `WorkflowNodeEnded` 事件) | agent_core | Todo | TBD |

### Phase 3: 前端 Agent CRUD

| ID | Task | Owner | Status | ETA |
|----|------|-------|--------|-----|
| T14 | 安装 `@xyflow/react` 依赖 | agent_core | Todo | TBD |
| T15 | 实现 `agentSlice` (Redux state + thunks) | agent_core | Todo | TBD |
| T16 | 扩展 `NewAgentModal` → `AgentEditor` (增加 tools / permissions / memory / max_iterations 字段) | agent_core | Todo | TBD |
| T17 | 实现 `AgentList` 侧边面板 (列出/搜索/删除 Agent) | agent_core | Todo | TBD |
| T18 | 实现 `AgentMemoryViewer` (查看 Agent 的 memory 和 history) | agent_core | Todo | TBD |

### Phase 4: 前端 Workflow Editor

| ID | Task | Owner | Status | ETA |
|----|------|-------|--------|----- |
| T19 | 实现 `workflowSlice` (Redux state + thunks) | agent_core | Todo | TBD |
| T20 | 实现 `WorkflowEditor` 主画布 (React Flow + 自定义节点/边) | agent_core | Todo | TBD |
| T21 | 实现 `WorkflowSidebar` (Node Palette + Agent 列表拖拽) | agent_core | Todo | TBD |
| T22 | 实现自定义节点组件 (`AgentNode` / `InputNode` / `OutputNode` / `TransformNode` / `ApprovalNode`) | agent_core | Todo | TBD |
| T23 | 实现 `EdgeConfigPanel` (data_mapping / condition 配置) | agent_core | Todo | TBD |
| T24 | 实现 `WorkflowRunView` (运行时节点状态可视化 + 事件流监听) | agent_core | Todo | TBD |

### Phase 5: 集成测试与打磨

| ID | Task | Owner | Status | ETA |
|----|------|-------|--------|-----|
| T25 | 端到端测试: 创建 Agent → 在 Workflow 中使用 → 执行 → 验证 Memory 积累 | agent_core | Todo | TBD |
| T26 | 验证主 Agent 不受影响 (现有功能回归测试) | agent_core | Todo | TBD |
| T27 | Workflow 验证器 UI (显示错误/警告) | agent_core | Todo | TBD |
| T28 | Workflow Run 历史 + 结果查看 | agent_core | Todo | TBD |

---

## Milestones

| Milestone | Description | Target Date |
|-----------|-------------|-------------|
| M1 | 后端 Agent 持久化 + Memory 隔离就绪 (T1-T7) | TBD |
| M2 | 后端 Workflow 引擎就绪 (T8-T13) | TBD |
| M3 | 前端 Agent CRUD 就绪，`create_agent` 可用 (T14-T18) | TBD |
| M4 | 前端 Workflow Editor 可拖拽/保存/运行 (T19-T24) | TBD |
| M5 | 端到端测试通过，主 Agent 无回归 (T25-T28) | TBD |

---

## Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| Agent Memory 的 BM25/HNSW per-agent 索引内存膨胀 | Med | Med | 索引 lazy-init，仅活跃 Agent 的索引常驻内存。不活跃 Agent 的索引在超时后卸载，下次访问重建。 |
| Workflow 执行中某个 Agent 节点超时/失败导致整个 Workflow 卡住 | High | Med | 每个 Agent 节点设置 timeout（可配置），超时后标记为 failed，条件边可配置失败行为（abort / skip / use_default）。 |
| 大量 Agent 并行执行导致 LLM API rate limit | High | High | `WorkflowConfig.max_concurrent` 限制并行度。Executor 使用 semaphore 控制并发。 |
| React Flow 画布数据与后端 DAG 不一致（前端画了环但后端未检测） | Med | Low | 保存时后端强制 `validate_workflow`，前端在保存前预检。运行前再次验证。 |
| Agent Memory 检索注入导致 context 过长 | Med | Med | 限制注入的 memory 条数（top_k=5）和总 token 数（如 2000 tokens）。超出时按 salience score 截断。 |
| 主 Agent 性能受影响 | High | Low | 所有新增表的读写走独立 Storage 连接或独立 Mutex，不与 `MemoryManager` 的 Mutex 竞争。Workflow 执行在独立 tokio task 中运行。 |
| Subagent 的 tool 权限泄漏（Agent 配置了不该有的 tool） | High | Low | `build_subagent_config` 时严格按 `AgentDef.tools` 过滤，empty = 继承全部。Permission 层面额外校验。 |

---

## Success Criteria

1. 用户可以在 `NewAgentModal`（扩展版）中创建自定义 Agent，保存后数据持久化到 SQLite。
2. 用户可以查看 Agent 列表，编辑/删除已有 Agent。
3. 用户可以在 React Flow 画布上拖拽 Agent 节点，连线组成 Workflow，保存到数据库。
4. 用户可以点击 Run 执行 Workflow，前端实时显示每个节点的运行状态。
5. 每次自定义 Agent 执行后，对话存储到该 Agent 的独立 Memory（`agent_memory` 表）。
6. 同一个 Agent 再次执行时，能够检索到之前的 Memory 并注入 context（"越用越强"）。
7. 主 Agent（现有 chat 功能）的行为和性能不受任何影响。
8. Workflow 支持条件分支、并行执行、人工审核节点。
9. Workflow 执行结果和每个节点的输入/输出可追溯（`workflow_run_node_results` 表）。

---

## Open Questions

1. **Agent Memory 是否需要跨 Agent 共享？** 当前设计是每个 Agent 独立 Memory。如果需要共享（例如 "Code Reviewer" 和 "Security Scanner" 共享代码库知识），是否需要引入 "Memory Group" 概念？还是通过 Workflow 的 Context 传递来间接共享？
   - **建议**：V1 不做跨 Agent Memory 共享。通过 Workflow Context 传递实现间接共享。V2 再考虑 Memory Group。

2. **Workflow 是否支持循环（cycle）？** 当前设计是 DAG（无环）。某些场景可能需要循环（例如 "审查 → 修复 → 再审查" 直到通过）。
   - **建议**：V1 仅支持 DAG。循环通过 `HumanApproval` 节点 + 手动重新运行实现。V2 考虑引入 `Loop` 节点类型。

3. **Agent 的 system prompt 是否支持模板变量？** 例如 `{{project_name}}`、`{{current_date}}`。
   - **建议**：V1 不支持模板变量。system prompt 是静态文本。V2 考虑支持 Jinja2 模板。

4. **Workflow 是否嵌套？** 一个 Workflow 能否作为另一个 Workflow 的节点？
   - **建议**：V1 不支持嵌套。每个 Workflow 是独立的。V2 考虑 `WorkflowNode` 类型。

5. **Agent 执行时是否复用现有的 `Run` 事件流（`broadcast::Sender<Envelope>`）？** 还是创建独立的事件流？
   - **建议**：复用现有事件流。Workflow 内的 Agent 执行产生 `SubagentStarted` / `SubagentEnded` 等事件，前端已有处理逻辑。新增 `WorkflowNodeStarted` / `WorkflowNodeEnded` 事件用于 Workflow 级别状态追踪。

6. **前端是否需要独立的 Workflow 页面/路由？** 还是在现有 Sidebar 中新增入口？
   - **建议**：在 Sidebar 中新增 "Workflows" 入口，打开新的全屏画布视图（类似独立的 Workflow Editor 页面）。不干扰现有 Chat 视图。

---

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-30 | agent_core | Created as Draft (AI-generated) |

---
*Generated by AI Agent (agent_core)*
*Model: glm-latest | Timestamp: 2026-06-30T00:00:00+08:00*
