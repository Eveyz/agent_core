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

让用户能够自定义 Agent（名字、system prompt、model、skills、tools、permissions），并使用 React Flow 可视化画布将多个 Agent 拖拽组合成一个 Multi-Agent Workflow（类似 Dify/n8n 的可视化工作流 + agent 节点）。每个自定义 Agent 拥有独立的 Memory 和 History，可以复用、持续进化。后端提供完整的数据库持久化，支持 Agent 间的 Context 传递，且以最小侵入方式集成到现有系统。

## Background

### 现状分析

当前系统存在以下关键事实（已逐条对照代码验证）：

1. **`create_agent` 命令不存在**：`NewAgentModal.tsx:55` 调用了 `invoke("create_agent", ...)`，但 `app/src-tauri/src/lib.rs:910` 的 `invoke_handler!` 中并未注册该命令。该按钮点击后必定报错。
2. **Agent 不是持久化实体**：现有 Agent 是 `RunManager` → `Brain` → `Run` 的临时执行单元。Subagent 是运行时由 `subagent` / `subagents` tool 临时 spawn 的，不持久化到数据库，Run 结束即消失。
3. **Memory 是全局共享的**：`MemoryManager`（`core/src/memory/mod.rs:27`）持有一个全局 `session_id: String`（UUID，line 32），所有 conversation 都写入同一张 `recall_memory` 表，以 `session_id` 区分。没有 "agent_id" 概念 —— `MemoryManager::new()`（line 38-44）的签名中不存在 `agent_id` 参数。
4. **Skills 是文件系统级**：`SkillManager` 扫描文件系统目录中的 `SKILL.md`，不存储在数据库中。Skills 与 Agent 没有显式关联。
5. **Teams 系统存在但未接入**：`core/src/teams/` 有 `AgentTeam` + `MessageBus`（基于 `Arc<Mutex<HashMap>>` 的 inbox 模式），但当前没有任何代码使用它。
6. **前端无图/流库**：`app/package.json` 中没有 `react-flow` / `@xyflow/react` 或任何图可视化库。
7. **`Subagent` 已与 Brain 解耦**：`core/src/subagent/mod.rs:40-79` 中 `Subagent` 拥有独立的 `client`、`context`、`registry`、`permission_policy`、`hook_registry`，`Subagent::new()` 不接受 `Arc<Brain>` 参数。这是自定义 Agent 执行体的理想基础。
8. **`Run::new` 是 `pub(crate)`**（`core/src/runtime/run.rs:129`），且 `Run` 持有 `brain: Arc<Brain>`（私有字段，line 76）。外部代码无法直接构造 `Run`。
9. **`Brain` 的 `memory` 和 `skill_manager` 字段是 `pub`**（`core/src/runtime/brain.rs:41,45`），`Brain::from_config`（line 61）内部通过 `Self::build_memory(&config)` 构造 `MemoryManager`。
10. **`SubagentConfig` 只有 4 个字段**（`core/src/subagent/mod.rs:13-18`）：`system_prompt`、`tools`、`max_iterations`、`max_context_tokens`。缺少 `model`、`skills`、`permission_mode`、`memory_enabled`、`temperature` 等字段的映射通路。`AgentDef` 有 12+ 字段，映射到 `SubagentConfig` 存在 gap。
11. **`Subagent` 不持有 `SkillManager`**（`core/src/subagent/mod.rs:40-49`）：Subagent 的 `ToolRegistry` 由外部传入，但没有 `SkillManager`。Skills 的加载/激活需要通过 `SkillManager`（`core/src/skills/mod.rs:69-74`）完成，目前没有方法将指定 skills 注入到一个独立的 Subagent 中。`register_skill_tools()`（`core/src/tools/skill.rs:8-16`）注册的是 `skill_list`/`skill_load`/`skill_deactivate`/`skill_reload` 工具，需要一个共享的 `SkillManager` 实例。
12. **`teams/` 模块未被使用**：`core/src/teams/` 的 `AgentTeam` + `MessageBus` 编译通过但没有任何代码调用。与 `WorkflowContext` 是两种不同的通信模型（异步 inbox vs 同步 state），并存会导致困惑。

### 为什么现在做

- 用户已经看到 `NewAgentModal` UI，但功能不完整（后端缺失）。
- 系统已有 Subagent、Teams、Memory 三层架构的基础设施，需要串联。
- Multi-Agent Workflow 是产品差异化的核心能力。

### 定位澄清

本方案是**可视化 DAG 工作流编排器**（ closer to Dify/n8n workflow + agent nodes），不是 Google ADK / AutoGen 式的动态多 agent 编排系统。区别：

| 维度 | 本方案 (DAG Workflow) | ADK / AutoGen (Dynamic Orchestration) |
|------|----------------------|--------------------------------------|
| 拓扑 | 静态 DAG，运行前固定 | 动态，运行时决定下一个 agent |
| 路由 | 条件边（V1 支持） | Agent 自主决策路由 |
| 通信 | 结构化 state 传递 | 自由消息传递 |
| 可预测性 | 高（拓扑确定） | 低（emergent behavior） |
| 调试 | 容易（按节点追踪） | 困难 |
| 适用场景 | 流水线式协作（研究→审查→修复） | 开放式协作（讨论→辩论→共识） |

V1 做静态 DAG + 条件路由，足够覆盖绝大多数实际场景。动态编排是 V2+ 方向。

---

## Scope

### In Scope (V1)

- **Agent 持久化层**：数据库 schema，存储自定义 Agent 定义。
- **Agent Memory 隔离**：每个自定义 Agent 拥有独立的 Memory 和 History，跨 session 持久化。
- **Workflow 持久化层**：存储画布的节点/边/拓扑结构（库无关格式，非 React Flow 原始 JSON）。
- **后端执行引擎**：DAG → 拓扑排序 → 分 stage 并行执行，条件边路由，结构化 state 传递。
- **前端 Agent CRUD**：修复 `NewAgentModal`，实现 Agent 列表/编辑/删除。
- **前端 Workflow Editor**：基于 React Flow 的可视化画布。
- **条件路由**：基于节点输出的 `if/else` 条件分支（V1 核心原语，不是增强功能）。
- **Workflow Trust Mode**：workflow 级别的权限策略，避免 coding agent 在 workflow 中被自动拒绝。
- **并发控制**：semaphore 限制并行 agent 执行数。
- **取消传播**：CancellationToken 从 workflow 级联到所有节点。
- **节点级可观测性**：token/cost/latency 字段。

### Out of Scope

- **动态编排**：运行时动态增删节点、agent 自主路由。
- **循环（cycle）**：V1 仅 DAG。循环通过 `HumanApproval` 节点 + 手动重跑实现。
- **Workflow 嵌套**：一个 Workflow 作为另一个 Workflow 的节点。
- **Agent 市场/共享**：不支持发布、下载、分享。
- **自动 Skill 生成**：Reflector 自动生成 Skill 列为 experimental，需人工确认后生效。
- **分布式多机调度**。

---

## 架构设计

### 设计原则

1. **Subagent 是执行原语**：自定义 Agent 的执行体是 `Subagent`，不是 `Run`。`Subagent` 已与 `Brain` 解耦（拥有独立的 `client`/`context`/`registry`/`permission_policy`），是 "持久化 + 带 memory + 可复用" 的天然载体。
2. **最小侵入，不自我设限**：不声称 "零修改"。明确列出哪些现有文件需要小改、哪些只读访问、哪些完全不动。
3. **复用现有基础设施**：`MemoryManager` 的存储/检索/评分逻辑通过泛化复用，不复制粘贴。`ToolRegistry`、`PermissionPolicy`、`ContextEngine` 直接复用。
4. **Agent 是一等公民**：Agent 有 `agent_id`，持久化到数据库，有独立 Memory/History，可被多个 Workflow 复用。
5. **库无关持久化**：后端存储 DAG 结构（节点表 + 边表），不存储 React Flow 的原始 JSON。React Flow 只是视图层。

### 侵入性评估（逐文件）

| 文件 | 改动级别 | 说明 |
|------|---------|------|
| `core/src/memory/storage.rs` | **扩展** | 新增 8 张表的 `CREATE TABLE IF NOT EXISTS` + `ALTER TABLE ADD COLUMN`（带 `PRAGMA table_info` 预检）。不修改现有表的 schema。 |
| `core/src/subagent/mod.rs` | **小改** | `Subagent` 新增一个构造函数 `new_with_memory()`，接受 `Option<Arc<AgentMemoryStore>>` 参数。现有 `new()` 不变。 |
| `core/src/runtime/brain.rs` | **只读访问** | `WorkflowExecutor` 通过 `brain.config` / `brain.skill_manager`（pub 字段）读取配置。不修改 Brain 的任何代码。 |
| `core/src/runtime/run.rs` | **不动** | `Run::new` 是 `pub(crate)`，外部不调用。 |
| `core/src/runtime/manager.rs` | **不动** | `RunManager` 的现有接口不变。 |
| `core/src/memory/mod.rs` | **不动** | `MemoryManager` 的现有代码不变。 |
| `core/src/lib.rs` | **扩展** | 新增 `pub mod agent_registry;` 和 `pub mod workflow;` 声明。 |
| `app/src-tauri/src/lib.rs` | **扩展** | 新增 Tauri commands 注册。现有命令不变。 |
| `app/src/components/ui/NewAgentModal.tsx` | **重写** | 扩展为完整 Agent Editor。 |
| `app/package.json` | **扩展** | 新增 `@xyflow/react` 依赖。 |

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
│  │        NEW: agent_registry/               │  │  EXISTING (只读):  │  │
│  │  ┌─────────────┐  ┌──────────────────┐   │  │  Brain.config      │  │
│  │  │ AgentDef    │  │ AgentMemoryStore │   │  │  Brain.skill_mgr   │  │
│  │  │ (SQLite)    │  │ (per-agent_id)   │   │  │  MemoryManager     │  │
│  │  └─────────────┘  └──────────────────┘   │  │  SkillManager      │  │
│  │  ┌─────────────┐                         │  │  ToolRegistry      │  │
│  │  │ AgentHistory│                         │  │  PermissionPolicy  │  │
│  │  │ Store       │                         │  │  ContextEngine     │  │
│  │  └─────────────┘                         │  │  SessionManager    │  │
│  └──────────────────────────────────────────┘  └────────────────────┘  │
│                                                │                       │
│  ┌──────────────────────────────────────────┐  │                       │
│  │     NEW: workflow/                        │  │                       │
│  │  ┌─────────────┐  ┌──────────────────┐   │  │  EXISTING (复用):    │  │
│  │  │ WorkflowDef │  │ WorkflowPlanner  │   │  │  Subagent            │  │
│  │  │ (SQLite)    │  │ (Kahn DAG sort)  │   │  │  SubagentConfig     │  │
│  │  └─────────────┘  └──────────────────┘   │  │  SubagentResult     │  │
│  │  ┌─────────────┐  ┌──────────────────┐   │  │  ToolOrchestrator   │  │
│  │  │ WorkflowCtx │  │ WorkflowExecutor │───┼──┼─► Subagent::new()    │  │
│  │  │ (State)     │  │ (stage并行)      │   │  │  Subagent::run()    │  │
│  │  └─────────────┘  └──────────────────┘   │  └────────────────────┘  │
│  └──────────────────────────────────────────┘                          │
│                                                                        │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │                    SQLite (~/.agverse/memory.db)                  │  │
│  │                                                                  │  │
│  │  EXISTING TABLES (不动):                                         │  │
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

### 为什么用 Subagent 而不是 Run/Brain

这是本方案的核心架构决策。reviewer 正确指出 `Subagent` 是更自然的复用对象，原因如下：

| 维度 | `Run` (via `RunManager`) | `Subagent` |
|------|--------------------------|------------|
| 依赖 `Arc<Brain>` | **是**（`Run::new` 是 `pub(crate)`，接受 `brain: Arc<Brain>`） | **否**（`Subagent::new` 接受独立组件） |
| 独立 Context | 是（但通过 Brain 构建） | **是**（自带 `Context::new(system_prompt, max_tokens)`） |
| 独立 ToolRegistry | 是（但通过 `brain.build_tool_registry()`） | **是**（外部传入，可完全自定义） |
| 独立 PermissionPolicy | 是（但通过 `brain.build_permission_policy()`） | **是**（外部传入 `PermissionConfig`） |
| 独立 HookRegistry | 是 | **是**（自带 fresh `HookRegistry::new()`） |
| 事件流 | `broadcast::Sender<Envelope>` | `Option<EventSender>`（mpsc，更轻量） |
| 进程管理 | `ProcessSupervisor`（完整） | 无（通过父 Run 的 supervisor） |
| 构造可见性 | `pub(crate)`（外部不可调） | `pub`（任何模块可调） |

**结论**：`Subagent` 已经是一个自包含的 agent 执行体，只需要：
1. 扩展一个 `new_with_memory()` 构造函数（接受 `Option<Arc<AgentMemoryStore>>`）
2. 在 `run_with_sender()` 中增加 memory 注入（执行前检索）和存储（执行后写入）

这比新建一个 `AgentRuntime` 去硬复用 `Run`/`Brain` 干净得多。

### SubagentConfig 扩展设计

reviewer 正确指出：`SubagentConfig` 只有 4 个字段，而 `AgentDef` 有 12+ 字段。直接映射存在 gap。以下是完整的映射方案：

**当前 `SubagentConfig`（`core/src/subagent/mod.rs:13-18`）**：
```rust
pub struct SubagentConfig {
    pub system_prompt: String,
    pub tools: Vec<String>,
    pub max_iterations: usize,
    pub max_context_tokens: usize,
}
```

**扩展后的 `SubagentConfig`**：
```rust
pub struct SubagentConfig {
    // ── 现有字段（不变） ──
    pub system_prompt: String,
    pub tools: Vec<String>,           // 空 = 继承全部
    pub max_iterations: usize,
    pub max_context_tokens: usize,

    // ── 新增字段（全部 Option，向后兼容） ──
    pub model_override: Option<String>,        // "provider/model"，None = 用 parent 的
    pub skills: Vec<String>,                   // skill names，空 = 无 skill
    pub permission_mode: Option<String>,       // paranoid/standard/developer/permissive/yolo
    pub memory_enabled: Option<u8>,            // None = 不启用 per-agent memory
    pub temperature: Option<f64>,              // per-agent temperature
}
```

**映射表**：

| `AgentDef` 字段 | `SubagentConfig` 字段 | 说明 |
|---|---|---|
| `system_prompt` | `system_prompt` | 直接映射 |
| `tools` | `tools` | 空 = 继承全部可用 tools |
| `max_iterations` | `max_iterations` | 直接映射 |
| `max_context_tokens` | `max_context_tokens` | 直接映射 |
| `model` | `model_override` | 空字符串 → `None`（用默认 model） |
| `skills` | `skills` | 直接映射 |
| `permission_mode` | `permission_mode` | 直接映射 |
| `memory_enabled` | `memory_enabled` | 0 → `None`（不启用），1/2 → `Some(value)` |
| `temperature` | `temperature` | 从 `ProviderModelEntry` 获取（如果有） |

**`Subagent::new()` 不变**：现有签名保持向后兼容。新增 `Subagent::new_with_config()` 接受扩展后的 `SubagentConfig`：

```rust
impl Subagent {
    /// 现有构造函数 — 不变，用默认值填充新字段
    pub fn new(role_name, config: SubagentConfig, model_config, registry, permission_config) -> Self {
        // 新字段从 config 中取（Option 默认 None）
        ...
    }

    /// 新增：完整构造，接受扩展配置 + memory store
    pub fn new_full(
        role_name: &str,
        config: SubagentConfig,
        brain: &Brain,                    // 只读：获取 model_config / skill_manager
        memory_store: Option<Arc<AgentMemoryStore>>,
        agent_id: Option<String>,
    ) -> Self {
        // 1. 解析 model_override → ModelConfig
        let model_config = if let Some(ref model) = config.model_override {
            brain.config.models.get(model).cloned()
                .unwrap_or_else(|| brain.config.models.get(&brain.current_model_name()).unwrap().clone())
        } else {
            brain.config.models.get(&brain.current_model_name()).unwrap().clone()
        };

        // 2. 构建 ToolRegistry
        let registry = if config.tools.is_empty() {
            brain.build_tool_registry(AgentMode::Build)
        } else {
            ToolRegistry::from_names(&config.tools)
        };

        // 3. 注入 Skills（见下方 Skills 通路设计）
        if !config.skills.is_empty() {
            if let Some(ref sm) = brain.skill_manager {
                inject_skills_to_registry(&config.skills, &mut registry, sm.clone());
            }
        }

        // 4. 构建 PermissionConfig
        let permission_config = if let Some(ref mode) = config.permission_mode {
            build_permission_config_with_mode(mode, &brain.config.permissions)
        } else {
            brain.config.permissions.clone()
        };

        // 5. 调用现有 new() 逻辑 + 设置 memory
        let mut sa = Self::new(role_name, config, &model_config, registry, permission_config);
        sa.memory_store = memory_store;
        sa.agent_id = agent_id;
        sa
    }
}
```

### Skills 通路设计

reviewer 正确指出：Subagent 不持有 `SkillManager`，Skills 的加载/激活需要通路。当前 `register_skill_tools()`（`core/src/tools/skill.rs:8-16`）注册的是 `skill_list`/`skill_load` 等管理工具，需要共享 `SkillManager` 实例。

**方案：Tool 注入 + Content 注入（双通路）**

Skill 有两个作用：(1) 提供 tool 能力（`provides_tools`），(2) 提供知识/context。两者都需要注入到 Subagent。

#### 通路 1：Skill 内容注入到 Context（知识）

```rust
/// 在 SkillManager 上新增方法：将指定 skills 的内容注入到一个 ContextEngine
impl SkillManager {
    /// 将指定 skills 的内容加载并注入到 context 的 Active Memory segment
    /// 复用 build_active_context() 的逻辑，但限定 skill 范围
    pub fn inject_to_context(&self, skill_names: &[String], context: &mut Context) {
        let mut parts = Vec::new();
        for name in skill_names {
            if let Some(manifest) = self.find_by_name(name) {
                if let Ok(content) = self.load_content(manifest) {
                    parts.push(format!("## Skill: {} (v{})\n{}\n",
                        manifest.name, manifest.version, content));
                }
            }
        }
        if !parts.is_empty() {
            let skill_text = format!(
                "The following skills are loaded. Use their knowledge:\n\n{}",
                parts.join("\n")
            );
            // 追加到现有的 Active Memory segment
            let existing = context.get_segment_content("active_memory");
            context.set_active_memory(&format!("{}\n\n{}", existing, skill_text));
        }
    }
}
```

#### 通路 2：Skill Tools 注册到 ToolRegistry（能力）

```rust
/// 在 SkillManager 上新增方法：将指定 skills 关联的 tools 注册到 registry
impl SkillManager {
    /// 将指定 skills 声明的 provides_tools 注册到 ToolRegistry
    /// 目前 provides_tools 是声明性元数据（不强制），此方法将其变为实际 tool 注册
    pub fn register_tools_to(&self, skill_names: &[String], registry: &mut ToolRegistry) {
        for name in skill_names {
            if let Some(manifest) = self.find_by_name(name) {
                for tool_name in &manifest.provides_tools {
                    if let Some(tool) = build_tool_by_name(tool_name) {
                        registry.register(tool);
                    }
                }
            }
        }
    }
}
```

#### 在 WorkflowExecutor 中的调用

```rust
// 在 execute_agent_node 中：
let mut subagent = Subagent::new_full(...);

// Skills 注入（在 Subagent 构造后、run 之前）
if !agent_def.skills.is_empty() {
    if let Some(ref sm) = self.brain.skill_manager {
        let mgr = sm.lock();
        // 通路 1: 内容注入到 context
        mgr.inject_to_context(&agent_def.skills, &mut subagent.context);
        // 通路 2: tools 注册到 registry
        mgr.register_tools_to(&agent_def.skills, &mut subagent.registry);
    }
}
```

**设计理由**：
- 方案 B（Tool 注入）比方案 A（纯 prompt 拼接）更完整，保留了 skill 的 tool 能力。
- 两个通路互不冲突：内容注入提供知识，tool 注册提供能力。
- `SkillManager` 的新方法是对现有代码的**扩展**，不修改现有方法签名。
- Subagent 的 `SkillManager` 通过 `brain.skill_manager`（`pub` 字段）获取，不需要 Brain 改动。

### SubagentConfig 扩展设计

reviewer 正确指出：`SubagentConfig`（`core/src/subagent/mod.rs:13-18`）只有 4 个字段，而 `AgentDef` 有 12+ 字段。映射 gap 必须在写 executor 代码之前想清楚。

#### 当前 SubagentConfig（4 字段）

```rust
pub struct SubagentConfig {
    pub system_prompt: String,
    pub tools: Vec<String>,
    pub max_iterations: usize,
    pub max_context_tokens: usize,
}
```

#### 扩展后的 SubagentConfig

```rust
pub struct SubagentConfig {
    // ── 现有字段（不变） ──
    pub system_prompt: String,
    pub tools: Vec<String>,           // empty = 继承全部
    pub max_iterations: usize,
    pub max_context_tokens: usize,

    // ── 新增可选字段（默认 None/empty，向后兼容） ──
    pub model: Option<String>,              // "provider/model"，None = 用传入的 model_config
    pub skills: Vec<String>,                // skill names，empty = 无 skill
    pub permission_mode: Option<String>,    // paranoid/standard/developer/permissive/yolo
    pub permission_rules: Vec<ConfigRule>,  // 自定义 permission rules
    pub memory_enabled: Option<u8>,         // None = 不启用 agent memory, 0/1/2 = stateless/standard/deep
    pub temperature: Option<f64>,           // per-agent temperature override
}
```

#### AgentDef → SubagentConfig 完整映射

| AgentDef 字段 | SubagentConfig 字段 | 说明 |
|---|---|---|
| `system_prompt` | `system_prompt` | 直接映射 |
| `tools` | `tools` | 直接映射，empty = 继承全部 |
| `max_iterations` | `max_iterations` | 直接映射 |
| `max_context_tokens` | `max_context_tokens` | 直接映射 |
| `model` | `model` | `Option<String>`，空 → `None`（用全局 default） |
| `skills` | `skills` | `Vec<String>`，skill names |
| `permission_mode` | `permission_mode` | `Option<String>` |
| `permission_rules` | `permission_rules` | `Vec<ConfigRule>`（反序列化自 JSON） |
| `memory_enabled` | `memory_enabled` | `Option<u8>`，0=stateless, 1=standard, 2=deep |
| `icon` / `color` | — | 仅前端使用，不传入 SubagentConfig |
| `description` | — | 仅 UI 展示，不传入 SubagentConfig |

#### Subagent 构造函数变更

`Subagent::new()` 的签名需要小幅扩展，或新增一个更完整的构造函数：

```rust
impl Subagent {
    /// 现有构造函数 — 保持不变，向后兼容
    /// 内部调用 new_extended，用 config 中的默认值填充
    pub fn new(
        role_name: &str,
        config: SubagentConfig,
        model_config: &ModelConfig,
        registry: ToolRegistry,
        permission_config: PermissionConfig,
    ) -> Self { ... }

    /// 新增：完整构造函数，支持 per-agent model / memory / skills
    pub fn new_extended(
        role_name: &str,
        config: SubagentConfig,
        brain: &Brain,                    // 只读：获取 model_config / skill_manager
        memory_store: Option<Arc<AgentMemoryStore>>,
        agent_id: Option<String>,
    ) -> Self {
        // 1. 解析 model：config.model 或 brain 的 current_model
        let model_config = Self::resolve_model(&config, brain);

        // 2. 构建 ToolRegistry（如果 config.tools 非空则过滤，否则继承全部）
        let registry = Self::resolve_registry(&config, brain);

        // 3. 构建 PermissionConfig
        let permission_config = Self::resolve_permissions(&config, brain);

        // 4. 如果 config.skills 非空，注入 skill content 到 system_prompt
        //    （详见 Skills 通路设计）
        let system_prompt = Self::inject_skills(&config, brain);

        // 5. 构造 Subagent
        let mut sa = Self::new(role_name, &config_with_skills, &model_config, registry, permission_config);
        sa.memory_store = memory_store;
        sa.agent_id = agent_id;
        sa
    }
}
```

### Skills 通路设计

这是 reviewer 指出的第二个关键设计点。当前 `Subagent` 不持有 `SkillManager`，无法加载/激活 skills。

#### 方案选择

| 方案 | 做法 | 优劣 |
|------|------|------|
| **A: Prompt 注入** | 读取 SKILL.md 内容，拼接到 system_prompt | 简单，但 skill 的 tool 绑定丢掉了 |
| **B: Tool 注入** | 通过 SkillManager 把 skill 注册为 tools + content 注入 | 完整保留 skill 的 tool 能力 |

**选择方案 B**（Tool 注入），原因：
- Subagent 已有独立的 `ToolRegistry`，可以把 skill 对应的 tool 直接注册进去
- 不污染 parent 的 ToolRegistry
- Skill content 仍然注入到 ContextEngine 的 Segment 6（LOADED SKILLS）

#### 实现：`SkillManager::register_to()` 新方法

```rust
impl SkillManager {
    /// 将指定 skills 的内容注入到 system_prompt，
    /// 并注册 skill tools 到给定的 ToolRegistry。
    /// 这是 Subagent 级别的 skill 注入 —— 不影响全局 SkillManager 状态。
    pub fn register_to(
        &self,
        skill_names: &[String],
        registry: &mut ToolRegistry,
    ) -> Result<String> {
        let mut skill_context = String::new();

        for name in skill_names {
            if let Some(manifest) = self.find_by_name(name) {
                // 1. 加载 skill content
                if let Ok(content) = self.load_content(manifest) {
                    skill_context.push_str(&format!(
                        "## Skill: {} (v{})\n{}\n\n",
                        manifest.name, manifest.version, content
                    ));
                }

                // 2. 如果 skill 声明了 provides_tools，注册对应 tools
                //    （当前 provides_tools 是声明性的，未来可扩展为动态注册）
            }
        }

        // 3. 注册 skill 管理工具（skill_list / skill_load / skill_deactivate）
        //    让 Subagent 在运行时也能动态加载其他 skills
        register_skill_tools(registry, /* 共享 SkillManager 的 Arc */);

        Ok(skill_context)
    }
}
```

#### 执行时的 Skills 注入流程

```
WorkflowExecutor::execute_agent_node()
  │
  ├── 1. 从 AgentDef 获取 skills 列表
  ├── 2. brain.skill_manager.lock().register_to(&skills, &mut registry)
  │      ├── 返回 skill content 字符串
  │      └── 注册 skill tools 到 Subagent 的 ToolRegistry
  ├── 3. 将 skill content 追加到 Subagent 的 system_prompt
  │      system_prompt = agent_def.system_prompt + "\n\n" + skill_context
  └── 4. Subagent 的 ContextEngine Segment 6 (LOADED SKILLS) 自动包含
         已激活 skills 的内容（通过 build_active_context）
```

**关键点**：`SkillManager` 是 `brain` 的 `pub` 字段（`Arc<Mutex<SkillManager>>`），`WorkflowExecutor` 可以通过 `brain.skill_manager` 只读访问。`register_to()` 是 `&self` 方法（不需要 `&mut self`），不会修改全局 SkillManager 的 `active_skills` 状态 —— 它只是读取 manifest 并返回 content + 注册 tools 到外部 registry。

### teams/ 模块处理

`core/src/teams/` 的 `MessageBus`（`Arc<Mutex<HashMap>>` inbox pattern）与 `WorkflowContext`（同步 state）是两种根本不同的通信模型：

| | WorkflowContext | MessageBus |
|---|---|---|
| 通信模式 | 同步 HashMap，stage 结束时收集 | 异步 inbox，轮询/等待/回复 |
| 适用 | DAG pipeline | Agent team negotiation（来回对话） |
| 死锁风险 | 无（DAG 无环） | 有（循环依赖） |

**决定**：V1 使用 `WorkflowContext`。在 `teams/mod.rs` 顶部添加 `#[deprecated]` 注释，标记为 "superseded by WorkflowContext in V1, may be revisited for V2 cyclic workflows"。避免两种模式并存导致困惑。

---

## 数据库 Schema 设计

### 迁移策略

现有代码（`core/src/memory/storage.rs`）只用 `CREATE TABLE IF NOT EXISTS`，从不 `ALTER`。本方案新增的表全部用 `CREATE TABLE IF NOT EXISTS`（幂等）。如果后续需要给已有新表加列，采用以下模式：

```rust
fn add_column_if_not_exists(conn: &Connection, table: &str, column: &str, def: &str) -> Result<()> {
    // PRAGMA table_info 预检，避免 "duplicate column" 错误
    let exists: bool = {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
        let rows: Vec<String> = stmt.query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok()).collect();
        rows.iter().any(|c| c == column)
    };
    if !exists {
        conn.execute(&format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, def), [])?;
    }
    Ok(())
}
```

### 新增表（全部在 `~/.agverse/memory.db` 中）

### 1. `agents` — 自定义 Agent 定义

```sql
CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    system_prompt TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',               -- "provider/model" or "" for default
    skills TEXT NOT NULL DEFAULT '[]',            -- JSON array of skill names
    tools TEXT NOT NULL DEFAULT '[]',             -- JSON array of tool names, empty = inherit all
    permission_mode TEXT NOT NULL DEFAULT 'standard',
    permission_rules TEXT NOT NULL DEFAULT '[]',  -- JSON: custom permission rules
    max_iterations INTEGER NOT NULL DEFAULT 50,
    max_context_tokens INTEGER NOT NULL DEFAULT 32000,
    memory_enabled INTEGER NOT NULL DEFAULT 1,    -- 0=stateless, 1=standard, 2=deep
    memory_group TEXT NOT NULL DEFAULT '',        -- 空=独立memory；非空=同组agent共享memory索引
    icon TEXT NOT NULL DEFAULT '',
    color TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agents_name ON agents(name);
```

### 2. `agent_memory` — Per-Agent / Memory Group 持久化记忆

```sql
CREATE TABLE IF NOT EXISTS agent_memory (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,                   -- 写入 agent 的 ID（追溯来源）
    memory_key TEXT NOT NULL,                 -- 隔离 key：默认 = agent_id；若 agent 在 memory_group 中则 = group name
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    embedding BLOB,
    importance REAL DEFAULT 0.5,
    memory_strength REAL DEFAULT 1.0,
    access_count INTEGER DEFAULT 0,
    last_accessed_at TEXT,
    category TEXT DEFAULT 'Conversation',
    source TEXT DEFAULT 'conversation',          -- conversation / reflection / workflow
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_memory_key ON agent_memory(memory_key);
CREATE INDEX IF NOT EXISTS idx_agent_memory_created ON agent_memory(created_at);
```

**`memory_key` 设计（Memory Sharing Group 支持）**：

reviewer 建议添加 opt-in 的 memory sharing group，让同组 agent 共享一个 memory 索引。这是产品创新点。

- `memory_key` 默认等于 `agent_id`（per-agent 隔离，V1 默认行为）
- 如果 `agents.memory_group` 非空（如 `"code-quality"`），则同组所有 agent 的 `memory_key` 都设为 `"code-quality"`
- `AgentMemoryStore` 按 `memory_key` 而非 `agent_id` 构建/检索 BM25/HNSW 索引
- 这意味着 "Code Reviewer" 和 "Security Scanner" 如果都在 `"code-quality"` group 中，它们共享同一个 memory 索引

**检索时的 key 解析**：
```rust
fn resolve_memory_key(agent_def: &AgentDef) -> &str {
    if agent_def.memory_group.is_empty() {
        &agent_def.id  // 默认：per-agent 隔离
    } else {
        &agent_def.memory_group  // 共享 group
    }
}
```

**`agent_id` 字段保留**：即使共享 group，每条 memory 仍记录是哪个 agent 写入的（溯源）。

CREATE VIRTUAL TABLE IF NOT EXISTS agent_memory_fts USING fts5(content, tokenize='unicode61');

-- FTS triggers (同 recall_memory 模式)
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

**与全局 `recall_memory` 完全独立**：不同的表、不同的索引、不同的 `MemoryManager` 实例。主 Agent 的 Memory 操作不经过 `agent_memory` 表，自定义 Agent 的 Memory 操作不经过 `recall_memory` 表。

### 3. `agent_history` — Agent 执行历史

```sql
CREATE TABLE IF NOT EXISTS agent_history (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    workflow_run_id TEXT DEFAULT '',
    trigger TEXT NOT NULL DEFAULT 'manual',      -- manual / workflow / cronjob
    input TEXT NOT NULL,
    output TEXT NOT NULL DEFAULT '',
    iterations_used INTEGER DEFAULT 0,
    success INTEGER NOT NULL DEFAULT 1,
    model_used TEXT NOT NULL DEFAULT '',
    token_input INTEGER DEFAULT 0,               -- 输入 token 数
    token_output INTEGER DEFAULT 0,              -- 输出 token 数
    process_time_ms INTEGER DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_history_agent ON agent_history(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_history_workflow ON agent_history(workflow_run_id);
```

### 4. `workflows` — Workflow 定义

```sql
CREATE TABLE IF NOT EXISTS workflows (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    input_schema TEXT NOT NULL DEFAULT '{}',     -- JSON: 输入参数定义
    output_schema TEXT NOT NULL DEFAULT '{}',
    config TEXT NOT NULL DEFAULT '{}',           -- JSON: max_concurrent, timeout, trust_mode
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

**`config` 字段内容**：
```json
{
  "max_concurrent": 3,          // 并行节点数上限
  "timeout_secs": 300,          // 单节点超时
  "trust_mode": "inherit",      // inherit / trusted / readonly
  "on_node_failure": "abort"    // abort / continue / skip
}
```

### 5. `workflow_nodes` — 节点（库无关格式）

```sql
CREATE TABLE IF NOT EXISTS workflow_nodes (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    node_type TEXT NOT NULL,          -- agent / input / output / human_approval / transform
    agent_id TEXT DEFAULT '',         -- node_type='agent' 时关联 agents 表
    label TEXT NOT NULL DEFAULT '',
    position_x REAL NOT NULL DEFAULT 0,  -- 画布坐标（仅前端使用）
    position_y REAL NOT NULL DEFAULT 0,
    config TEXT NOT NULL DEFAULT '{}',   -- JSON: 节点特定配置
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_wf_nodes_workflow ON workflow_nodes(workflow_id);
```

**库无关设计**：后端存储节点/边的结构化数据（`node_type`, `agent_id`, `config`），`position_x/y` 仅用于前端画布渲染。后端 executor 不解析 React Flow 的 `{nodes, edges, position}` JSON，而是从 `workflow_nodes` + `workflow_edges` 表读取自己的 DAG 表示。React Flow 只是视图层，可替换为任何其他图库。

**节点类型**：

| `node_type` | 说明 | `config` 内容 |
|-------------|------|--------------|
| `input` | Workflow 输入入口 | 输入变量 schema |
| `output` | Workflow 输出口 | 输出格式定义 |
| `agent` | Agent 执行节点 | `input_template`, `output_key`, `tools_override` |
| `human_approval` | 人工审核 | `prompt`, `timeout_secs`, `on_timeout` |
| `transform` | 数据转换 | `template` (Jinja2-like), `output_key` |

### 6. `workflow_edges` — 边（含条件路由）

```sql
CREATE TABLE IF NOT EXISTS workflow_edges (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    source_node_id TEXT NOT NULL REFERENCES workflow_nodes(id) ON DELETE CASCADE,
    target_node_id TEXT NOT NULL REFERENCES workflow_nodes(id) ON DELETE CASCADE,
    source_handle TEXT DEFAULT '',         -- 输出端口（多输出节点）
    target_handle TEXT DEFAULT '',         -- 输入端口（多输入节点）
    label TEXT NOT NULL DEFAULT '',
    condition TEXT NOT NULL DEFAULT '',    -- 条件表达式，空=无条件
    data_mapping TEXT NOT NULL DEFAULT '{}', -- JSON: 字段映射
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_wf_edges_workflow ON workflow_edges(workflow_id);
CREATE INDEX IF NOT EXISTS idx_wf_edges_source ON workflow_edges(source_node_id);
CREATE INDEX IF NOT EXISTS idx_wf_edges_target ON workflow_edges(target_node_id);
```

### 7. `workflow_runs` — 执行记录

```sql
CREATE TABLE IF NOT EXISTS workflow_runs (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',  -- pending/running/completed/failed/cancelled
    input TEXT NOT NULL DEFAULT '{}',
    output TEXT NOT NULL DEFAULT '{}',
    error TEXT NOT NULL DEFAULT '',
    total_token_input INTEGER DEFAULT 0,     -- 汇总
    total_token_output INTEGER DEFAULT 0,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_wf_runs_workflow ON workflow_runs(workflow_id);
CREATE INDEX IF NOT EXISTS idx_wf_runs_status ON workflow_runs(status);
```

### 8. `workflow_run_node_results` — 节点级结果（含可观测性字段）

```sql
CREATE TABLE IF NOT EXISTS workflow_run_node_results (
    id TEXT PRIMARY KEY,
    workflow_run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    node_id TEXT NOT NULL,
    agent_history_id TEXT DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending',  -- pending/running/completed/failed/skipped
    input TEXT NOT NULL DEFAULT '{}',
    output TEXT NOT NULL DEFAULT '{}',
    error TEXT NOT NULL DEFAULT '',
    token_input INTEGER DEFAULT 0,           -- 节点级 token 统计
    token_output INTEGER DEFAULT 0,
    cost_usd REAL DEFAULT 0.0,               -- 节点级成本估算
    latency_ms INTEGER DEFAULT 0,            -- 节点级延迟
    started_at TEXT,
    finished_at TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_wf_run_nodes ON workflow_run_node_results(workflow_run_id);
```

---

## 后端架构设计

### 新增模块结构

```
core/src/
├── agent_registry/          ← NEW
│   ├── mod.rs               ← AgentRegistry 协调层
│   ├── definition.rs        ← AgentDef + DB CRUD
│   ├── memory.rs            ← AgentMemoryStore (per-agent memory 隔离)
│   └── history.rs           ← AgentHistoryStore
├── workflow/                ← NEW
│   ├── mod.rs               ← WorkflowEngine 协调层
│   ├── definition.rs        ← WorkflowDef + NodeDef + EdgeDef + DB CRUD
│   ├── planner.rs           ← Kahn 拓扑排序 + 环检测 + 并行分组
│   ├── executor.rs          ← 分 stage 执行 + Subagent 调用
│   ├── context.rs           ← 结构化 State + 条件路由
│   └── trust.rs             ← TrustMode 策略
├── subagent/                ← SMALL CHANGE: 新增 new_with_memory()
├── runtime/                 ← EXISTING: 只读访问 brain.config / brain.skill_manager
├── memory/                  ← EXISTING: 不修改
└── ...
```

### 1. Agent Registry (`core/src/agent_registry/`)

#### `AgentDef`

```rust
pub struct AgentDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub model: String,
    pub skills: Vec<String>,
    pub tools: Vec<String>,
    pub permission_mode: String,
    pub permission_rules: serde_json::Value,
    pub max_iterations: usize,
    pub max_context_tokens: usize,
    pub memory_enabled: u8,  // 0/1/2
    pub icon: String,
    pub color: String,
    pub created_at: String,
    pub updated_at: String,
}
```

#### `AgentRegistry` — CRUD + 组件构建

```rust
pub struct AgentRegistry {
    storage: Storage,
}

impl AgentRegistry {
    pub fn create(&self, def: AgentDef) -> Result<AgentDef>;
    pub fn get(&self, id: &str) -> Result<AgentDef>;
    pub fn list(&self) -> Result<Vec<AgentDef>>;
    pub fn update(&self, id: &str, updates: &AgentDefUpdate) -> Result<AgentDef>;
    pub fn delete(&self, id: &str) -> Result<()>;

    /// 从 AgentDef 构建 SubagentConfig
    pub fn build_subagent_config(&self, def: &AgentDef) -> SubagentConfig {
        SubagentConfig {
            system_prompt: def.system_prompt.clone(),
            tools: if def.tools.is_empty() { /* inherit all */ vec![] } else { def.tools.clone() },
            max_iterations: def.max_iterations,
            max_context_tokens: def.max_context_tokens,
        }
    }

    /// 从 AgentDef 构建 PermissionConfig
    pub fn build_permission_config(&self, def: &AgentDef, base: &PermissionConfig) -> PermissionConfig {
        let mut config = base.clone();
        config.mode = PermissionMode::from_str(&def.permission_mode);
        // 合并 def.permission_rules 到 config.rules
        config
    }

    /// 从 AgentDef 构建 ModelConfig（fallback 到全局 default_model）
    pub fn build_model_config(&self, def: &AgentDef, brain: &Brain) -> ModelConfig {
        if def.model.is_empty() {
            brain.config.models.get(&brain.current_model_name()).cloned().unwrap()
        } else {
            brain.config.models.get(&def.model).cloned().unwrap_or_default()
        }
    }
}
```

#### `AgentMemoryStore` — Per-Agent Memory（复用 MemoryManager 模式，不复制代码）

```rust
pub struct AgentMemoryStore {
    storage: Storage,
    embedding_model: Arc<EmbeddingModel>,      // 共享全局 embedding model
    salience_scorer: SalienceScorer,           // 共享全局 salience config
    bm25_indexes: Mutex<HashMap<String, BM25Index>>,   // per-agent_id, lazy init
    hnsw_indexes: Mutex<HashMap<String, HNSWIndex>>,   // per-agent_id, lazy init
}

impl AgentMemoryStore {
    pub fn store(&self, agent_id: &str, role: &str, content: &str, source: &str) -> Result<String>;
    pub fn search(&self, agent_id: &str, query: &str, top_k: usize) -> Result<Vec<ScoredRecord>>;
    pub fn search_hybrid(&self, agent_id: &str, query: &str, top_k: usize) -> Result<Vec<ScoredRecord>>;
    pub fn consolidate(&self, agent_id: &str) -> Result<ConsolidationReport>;
    pub fn prune_cold(&self, agent_id: &str) -> Result<usize>;
    pub fn stats(&self, agent_id: &str) -> Result<MemoryStats>;

    /// 执行前检索相关记忆，格式化为 context 注入文本
    pub fn build_context_injection(&self, agent_id: &str, query: &str, max_tokens: usize) -> String {
        let memories = self.search_hybrid(agent_id, query, 5)?;
        if memories.is_empty() { return String::new(); }

        let mut text = String::from("## Relevant Memory from Previous Executions\n\n");
        for (i, mem) in memories.iter().enumerate() {
            let entry = format!("### Memory {} (importance: {:.2})\n{}\n\n",
                i + 1, mem.importance, mem.content);
            // token 预算控制
            if text.len() + entry.len() > max_tokens * 4 { break; }
            text.push_str(&entry);
        }
        text
    }
}
```

**关键设计**：
- 复用 `EmbeddingModel`、`SalienceScorer`、`BM25Index`、`HNSWIndex`、`MemoryConsolidator` 的实现（这些是独立的 struct，不绑定 `MemoryManager`）。
- BM25/HNSW 索引按 `agent_id` 分别构建，lazy-initialize（首次访问某 Agent 的 memory 时构建）。不活跃 Agent 的索引在 LRU 超时后卸载。
- 与全局 `MemoryManager` 完全独立：不同的表、不同的索引、不同的 Mutex。

### 2. Subagent 扩展（`core/src/subagent/mod.rs` 的小改）

新增一个构造函数和 memory 注入逻辑：

```rust
impl Subagent {
    /// 现有构造函数 — 不变
    pub fn new(role_name, config, model_config, registry, permission_config) -> Self { ... }

    /// 新增：带 memory 的构造函数
    pub fn new_with_memory(
        role_name: &str,
        config: SubagentConfig,
        model_config: &ModelConfig,
        registry: ToolRegistry,
        permission_config: PermissionConfig,
        memory_store: Option<Arc<AgentMemoryStore>>,
        agent_id: Option<String>,
    ) -> Self {
        let mut sa = Self::new(role_name, config, model_config, registry, permission_config);
        sa.memory_store = memory_store;
        sa.agent_id = agent_id;
        sa
    }

    /// 新增：在 run_with_sender 的开头，注入 memory
    /// 在 self.context.add(Message::user(task)) 之前调用
    fn inject_memory(&mut self, task: &str) {
        if let (Some(ref store), Some(ref agent_id)) = (&self.memory_store, &self.agent_id) {
            let injection = store.build_context_injection(agent_id, task, 2000);
            if !injection.is_empty() {
                // 注入到 ContextEngine 的 Active Memory segment
                self.context.set_active_memory(&injection);
            }
        }
    }

    /// 新增：在 run_with_sender 结束后，存储 memory
    fn persist_memory(&self, task: &str, output: &str) {
        if let (Some(ref store), Some(ref agent_id)) = (&self.memory_store, &self.agent_id) {
            let _ = store.store(agent_id, "user", task, "conversation");
            let _ = store.store(agent_id, "assistant", output, "conversation");
        }
    }
}
```

**Subagent struct 新增两个 field**：
```rust
pub struct Subagent {
    // ... 现有字段 ...
    memory_store: Option<Arc<AgentMemoryStore>>,  // NEW
    agent_id: Option<String>,                     // NEW
}
```

这是对 `subagent/mod.rs` 的唯一改动。现有 `new()` 和 `run_with_sender()` 的行为不变（`memory_store` 为 `None` 时走原逻辑）。

### 3. Workflow Engine (`core/src/workflow/`)

#### `WorkflowPlanner` — DAG → 执行计划

```rust
pub struct ExecutionPlan {
    pub stages: Vec<ExecutionStage>,
}

pub struct ExecutionStage {
    pub nodes: Vec<String>,  // 同一 stage 内的节点 ID，可并行执行
}

impl WorkflowPlanner {
    pub fn plan(nodes: &[NodeDef], edges: &[EdgeDef]) -> Result<ExecutionPlan> {
        // 1. 构建邻接表 (source -> [targets])
        // 2. Kahn 算法拓扑排序
        // 3. 同层节点分组（indegree 同时归零的分到同一 stage）
        // 4. 检测环（剩余 indegree > 0 的节点构成环）
        // 5. 返回 ExecutionPlan
    }
}
```

#### `WorkflowContext` — 结构化 State（非字符串模板）

```rust
/// 结构化共享状态 — 所有节点的输入/输出存储在这里
/// 比字符串模板更安全：无模板注入风险、有类型校验、可字段级合并
pub struct WorkflowContext {
    /// 每个节点的输出（结构化 JSON）
    node_outputs: RwLock<HashMap<String, serde_json::Value>>,
    /// 全局共享 state（所有节点可读写）
    shared: RwLock<serde_json::Value>,
    /// Workflow 输入
    input: serde_json::Value,
}

impl WorkflowContext {
    /// 从上游节点的输出 + edge 的 data_mapping 解析出当前节点的输入
    pub fn resolve_input(
        &self,
        node_id: &str,
        incoming_edges: &[EdgeDef],
    ) -> serde_json::Value {
        let mut input = serde_json::Map::new();

        for edge in incoming_edges {
            let upstream_output = self.node_outputs.read().get(&edge.source_node_id)
                .cloned().unwrap_or(serde_json::Value::Null);

            let mapping: DataMapping = serde_json::from_str(&edge.data_mapping).unwrap_or_default();

            if mapping.pass_through {
                // 完整传递上游 output，以 source_node label 为 key
                input.insert(edge.label.clone(), upstream_output);
            } else {
                // 字段级映射：从 upstream_output 中提取指定字段
                if let Some(ref source_field) = mapping.source_field {
                    if let Some(val) = upstream_output.get(source_field) {
                        let target_key = mapping.target_field
                            .as_ref().unwrap_or(source_field);
                        input.insert(target_key.clone(), val.clone());
                    }
                }
            }
        }

        // 注入全局 shared state
        input.insert("_shared".into(), self.shared.read().clone());
        // 注入 workflow 原始输入
        input.insert("_workflow_input".into(), self.input.clone());

        serde_json::Value::Object(input)
    }

    pub fn set_output(&self, node_id: &str, output: serde_json::Value) {
        self.node_outputs.write().insert(node_id.to_string(), output);
    }

    pub fn update_shared(&self, key: &str, value: serde_json::Value) {
        if let Some(obj) = self.shared.write().as_object_mut() {
            obj.insert(key.to_string(), value);
        }
    }
}

#[derive(Deserialize, Default)]
struct DataMapping {
    source_field: Option<String>,
    target_field: Option<String>,
    pass_through: bool,
}
```

**与字符串模板的对比**：

| 维度 | 字符串模板 `{{node-1.output}}` | 结构化 State |
|------|-------------------------------|-------------|
| 模板注入风险 | 有（上游输出含 `{{` 会误替换） | 无 |
| 类型校验 | 无 | 有（JSON schema） |
| 字段级合并 | 困难 | 原生支持 |
| 调试 | 困难（字符串拼接） | 容易（结构化 JSON） |
| 多上游汇聚 | 字符串拼接 | Map 合并 |

#### 条件路由 — Router 模型（参考 LangGraph）

reviewer 正确指出：独立条件边存在多边竞争、stage barrier 交互等复杂语义问题。参考 LangGraph 的 `conditional_edges` 模型，改用**节点级 Router**：一个节点有一个可选的 `router` 配置，返回目标节点名（或节点名列表用于 fork）。

**语义对比**：

| 模型 | 语义 | 竞态问题 |
|------|------|---------|
| 独立条件边（原方案） | 每条边各自 `if condition` | 多条边同时满足 → fork? switch? 不明确 |
| Router（新方案） | 节点输出后，router 返回目标 | **无竞态**：router 返回唯一结果 |

```rust
/// 节点的路由配置 — 在 NodeDef.config 中
///
/// router 是一个 JSON 配置，定义了基于节点输出的路由规则：
/// {
///   "rules": [
///     { "condition": "success == true", "targets": ["fixer_node"] },
///     { "condition": "success == false", "targets": ["reporter_node"] },
///     { "condition": "issues.length > 5", "targets": ["fixer_node", "reporter_node"] }
///   ],
///   "default": ["output_node"]  // 所有规则都不匹配时的默认目标
/// }
///
/// 如果节点没有 router 配置，则所有出边无条件传递（原行为）。

pub struct RouterConfig {
    rules: Vec<RouterRule>,
    default_targets: Vec<String>,
}

pub struct RouterRule {
    condition: ConditionExpr,
    targets: Vec<String>,  // 多个 target = fork（并行执行）
}

impl RouterConfig {
    /// 根据节点输出决定下一步执行哪些节点
    /// 返回节点 ID 列表（空 = 终止）
    pub fn route(&self, output: &serde_json::Value) -> Vec<String> {
        for rule in &self.rules {
            if rule.condition.evaluate(output) {
                return rule.targets.clone();
            }
        }
        self.default_targets.clone()
    }
}

struct ConditionExpr {
    field: String,       // "success", "score", "issues.length"
    operator: CondOp,    // ==, !=, >, <, >=, <=, contains
    value: serde_json::Value,
}

impl ConditionExpr {
    fn evaluate(&self, output: &serde_json::Value) -> bool {
        let actual = output.get(&self.field);
        match self.operator {
            CondOp::Eq => actual == Some(&self.value),
            CondOp::Ne => actual != Some(&self.value),
            CondOp::Gt => /* numeric comparison */,
            CondOp::Contains => /* string contains */,
        }
    }
}
```

**与 Planner 的集成**：

```
WorkflowPlanner:
  1. Kahn 拓扑排序时，忽略 router 条件（按所有出边排序）
  2. 运行时，节点执行后调用 router.route(output)
  3. Router 返回的 targets 即为实际要执行的下游节点
  4. 不在 targets 中的下游节点标记为 skipped
  5. Router 返回多个 target → 下一个 stage 中这些节点并行执行（fork）
```

**条件分支示例**：

```
                    ┌─ router rule: "success == true" → [Fixer]
Agent A (Reviewer) ─┤
                    └─ router rule: "success == false" → [Reporter]

Agent A 输出 {success: true, result: "..."}
  → router.route(output) 返回 ["Fixer"]
  → Fixer 执行，Reporter 标记为 skipped

Agent A 输出 {success: false, result: "..."}
  → router.route(output) 返回 ["Reporter"]
  → Reporter 执行，Fixer 标记为 skipped
```

**Fork 示例**：

```
Agent A 输出 {success: true, issues: [1,2,3,4,5,6]}  // 6 个 issues
  → router rule: "issues.length > 5" → [Fixer, Reporter]
  → Fixer 和 Reporter 在下一个 stage 并行执行
```

**与 `workflow_edges` 表的关系**：
- `workflow_edges` 仍然存储所有可能的边（包括条件边），用于前端画布渲染和 planner 排序。
- 边的 `condition` 字段被 `router` 配置取代。如果节点有 `router`，出边的 `condition` 字段忽略。
- 如果节点没有 `router`，出边无条件传递（原行为）。

#### TrustMode — Workflow 级权限策略

```rust
pub enum TrustMode {
    /// 继承全局配置，Ask 级工具正常弹出审批
    /// 适用场景：低风险 workflow
    Inherit,

    /// 信任模式：所有工具自动允许（相当于 workflow 级 Yolo）
    /// 适用场景：用户明确信任的自动化 workflow
    /// 审批通过 HumanApproval 节点显式插入
    Trusted,

    /// 只读模式：仅允许 ReadOnly 级工具
    /// 适用场景：纯分析/审查 workflow
    Readonly,
}

impl TrustMode {
    pub fn build_permission_config(&self, base: &PermissionConfig, agent_def: &AgentDef) -> PermissionConfig {
        let mut config = AgentRegistry.build_permission_config(agent_def, base);
        match self {
            TrustMode::Trusted => {
                config.mode = PermissionMode::Yolo;
            }
            TrustMode::Readonly => {
                config.mode = PermissionMode::Paranoid;
                config.auto_allow_up_to = Some(DangerLevel::ReadOnly);
            }
            TrustMode::Inherit => { /* 用 agent_def 的 permission_mode */ }
        }
        config
    }
}
```

**设计理由**：reviewer 正确指出 "Approval 自动拒绝 = workflow 里的 coding agent 基本废了"。`Trusted` 模式让用户可以创建一个可信的自动化 workflow，bash/write 等工具自动放行，同时在关键节点插入 `HumanApproval` 节点做人工检查。

#### `WorkflowExecutor` — 分 stage 执行

```rust
pub struct WorkflowExecutor {
    registry: Arc<AgentRegistry>,
    memory_store: Arc<AgentMemoryStore>,
    history_store: Arc<AgentHistoryStore>,
    brain: Arc<Brain>,               // 只读：获取 config / skill_manager
    session_manager: Arc<Mutex<SessionManager>>,
}

impl WorkflowExecutor {
    pub async fn execute(
        &self,
        workflow: &WorkflowDef,
        input: serde_json::Value,
        session_id: &str,
        event_tx: broadcast::Sender<Envelope>,
        cancel_token: CancellationToken,
    ) -> Result<serde_json::Value> {
        let plan = WorkflowPlanner::plan(&workflow.nodes, &workflow.edges)?;
        let ctx = Arc::new(WorkflowContext::new(input.clone()));
        let trust_mode = TrustMode::from_config(&workflow.config);
        let max_concurrent = workflow.config.max_concurrent.unwrap_or(3);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));
        let mut total_tokens = (0i64, 0i64);

        // 创建 workflow_run 记录
        let run_id = self.create_workflow_run(&workflow.id, session_id, &input)?;

        for (stage_idx, stage) in plan.stages.iter().enumerate() {
            // 检查取消
            if cancel_token.is_cancelled() { break; }

            // 并行执行 stage 内所有节点
            let mut handles = Vec::new();
            for node_id in &stage.nodes {
                let node = workflow.nodes.iter().find(|n| &n.id == node_id).unwrap();

                // 检查条件边 — 如果所有入边都不满足，跳过
                if !self.should_execute(node, &workflow.edges, &ctx) {
                    self.mark_node_skipped(&run_id, &node.id)?;
                    continue;
                }

                let input = ctx.resolve_input(&node.id, &workflow.edges);
                let handle = self.spawn_node_execution(
                    node.clone(), input, ctx.clone(), trust_mode.clone(),
                    semaphore.clone(), cancel_token.clone(),
                    event_tx.clone(), run_id.clone(),
                );
                handles.push(handle);
            }

            // 等待 stage 内所有节点完成
            let results = futures::future::join_all(handles).await;

            // 收集结果
            for (node_id, result) in results {
                match result {
                    Ok((output, tokens)) => {
                        ctx.set_output(&node_id, output);
                        total_tokens.0 += tokens.0;
                        total_tokens.1 += tokens.1;
                    }
                    Err(e) => {
                        match workflow.config.on_node_failure {
                            FailurePolicy::Abort => return Err(e),
                            FailurePolicy::Continue => { /* 标记 failed，继续 */ }
                            FailurePolicy::Skip => { /* 标记 skipped，下游也 skip */ }
                        }
                    }
                }
            }
        }

        // 获取 output 节点的输出
        let output = ctx.get_output_node_result();
        self.complete_workflow_run(&run_id, &output, total_tokens)?;
        Ok(output)
    }

    async fn execute_agent_node(
        &self,
        node: &NodeDef,
        input: serde_json::Value,
        agent_def: &AgentDef,
        trust_mode: &TrustMode,
        cancel_token: CancellationToken,
        event_tx: &broadcast::Sender<Envelope>,
    ) -> Result<(serde_json::Value, (i64, i64))> {
        // 1. 从 AgentDef 构建组件
        let subagent_config = self.registry.build_subagent_config(agent_def);
        let model_config = self.registry.build_model_config(agent_def, &self.brain);
        let permission_config = trust_mode.build_permission_config(
            &self.brain.config.permissions, agent_def);

        // 2. 构建 ToolRegistry
        let registry = if agent_def.tools.is_empty() {
            self.brain.build_tool_registry(AgentMode::Build)
        } else {
            ToolRegistry::from_names(&agent_def.tools)
        };

        // 3. 构建 Subagent（带 memory）
        let memory = if agent_def.memory_enabled > 0 {
            Some(self.memory_store.clone())
        } else { None };

        let mut subagent = Subagent::new_with_memory(
            &agent_def.name, subagent_config, &model_config,
            registry, permission_config, memory, Some(agent_def.id.clone()),
        );

        // 4. 组装输入（结构化 JSON → 可读文本）
        let task = self.format_agent_input(&node.config, &input);

        // 5. 执行（Subagent 内部会注入 memory + 执行后存储 memory）
        let result = tokio::select! {
            _ = cancel_token.cancelled() => return Err(anyhow!("cancelled")),
            r = subagent.run_with_sender(&task, Some(event_tx.clone().into())) => r?,
        };

        // 6. 记录历史
        self.history_store.record(AgentHistoryEntry {
            agent_id: agent_def.id.clone(),
            session_id: session_id.to_string(),
            workflow_run_id: run_id.clone(),
            trigger: "workflow".to_string(),
            input: task,
            output: result.output.clone(),
            iterations_used: result.iterations_used,
            success: result.success,
            model_used: model_config.model_id.clone(),
            token_input: result.token_input,
            token_output: result.token_output,
            process_time_ms: result.elapsed_ms,
            ..
        })?;

        // 7. 返回结构化输出
        let output = serde_json::json!({
            "result": result.output,
            "success": result.success,
            "iterations": result.iterations_used,
        });

        Ok((output, (result.token_input, result.token_output)))
    }
}
```

#### 取消传播

```
用户点击 Cancel
  │
  ▼
WorkflowExecutor 收到 CancellationToken.cancel()
  │
  ├── 当前 stage 中正在运行的节点:
  │   ├── tokio::select! { _ = cancel_token.cancelled() => return Err("cancelled") }
  │   └── Subagent 的 run_with_sender 被 abort
  │
  ├── 后续 stages 不再执行
  │
  └── workflow_run 记录标记为 cancelled
```

`CancellationToken` 是 `Clone` 的，从 `WorkflowExecutor` 传到每个 `execute_agent_node`，再通过 `tokio::select!` 传播到 `Subagent::run_with_sender()`。Subagent 内部的 LLM stream 和 tool execution 也通过 `CancellationToken` 感知取消。

### 4. 与现有系统的关系

| 现有组件 | 改动 | 复用方式 |
|---------|------|---------|
| `RunManager` / `Run` | **不动** | Workflow 不创建 Run，不经过 RunManager |
| `Brain` | **只读访问** | `brain.config` / `brain.skill_manager` (pub 字段) |
| `Subagent` | **小改**：新增 `new_with_memory()` + 2 个 field | Workflow agent 节点的执行体 |
| `MemoryManager` | **不动** | 主 Agent 的全局 Memory 不受影响 |
| `Storage` (SQLite) | **扩展**：新增 8 张表 | 同一个 `~/.agverse/memory.db` |
| `SkillManager` | **不动** | Agent 执行时通过 brain.skill_manager 获取 |
| `ToolRegistry` | **不动** | `from_names()` 或 `brain.build_tool_registry()` |
| `PermissionPolicy` | **不动** | TrustMode 构建 `PermissionConfig` 传入 Subagent |
| `ContextEngine` | **不动** | Subagent 内部的 ContextEngine 照常工作 |
| `SessionManager` | **不动** | Workflow Run 创建 session，复用现有机制 |

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

### 新增前端结构

```
app/src/
├── features/
│   ├── agents/                    ← NEW
│   │   ├── agentSlice.ts
│   │   ├── types.ts
│   │   └── thunks.ts
│   └── workflow/                  ← NEW
│       ├── workflowSlice.ts
│       ├── types.ts
│       └── thunks.ts
├── components/
│   ├── agents/                    ← NEW
│   │   ├── AgentList.tsx
│   │   ├── AgentEditor.tsx        ← 扩展 NewAgentModal
│   │   └── AgentMemoryViewer.tsx
│   └── workflow/                  ← NEW
│       ├── WorkflowEditor.tsx     ← React Flow 画布
│       ├── WorkflowSidebar.tsx    ← Node Palette
│       ├── AgentNode.tsx
│       ├── InputNode.tsx
│       ├── OutputNode.tsx
│       ├── TransformNode.tsx
│       ├── ApprovalNode.tsx
│       ├── EdgeConfigPanel.tsx
│       └── WorkflowRunView.tsx
```

### Agent Editor（修复 NewAgentModal）

扩展现有 `NewAgentModal.tsx`，增加：
- **Tools**：多选（从可用 tools 列表选择，空 = 继承全部）
- **Permission Mode**：下拉（paranoid/standard/developer/permissive/yolo）
- **Memory**：开关 + 模式（stateless/standard/deep）
- **Max Iterations / Max Context Tokens**：数字输入
- **Icon / Color**：用于 React Flow 节点样式

### React Flow Workflow Editor

```
┌─────────────────────────────────────────────────────────────────┐
│  Toolbar: [Save] [Run] [Validate]                               │
├──────────┬──────────────────────────────────────────────────────┤
│  Node    │              React Flow Canvas                       │
│  Palette │                                                      │
│          │   ┌────────┐     ┌────────────┐     ┌────────┐      │
│  > Input │   │ Input  │────►│  Agent A   │────►│ Output │      │
│  > Agent │   └────────┘     │ Code       │     └────────┘      │
│  > Trans │                  │ Reviewer   │                      │
│  > Apprv │                  └────────────┘                      │
│  > Output│                       │ condition: output.success   │
│          │                       ▼                              │
│  Agents: │                  ┌────────────┐                      │
│  [Code   │                  │  Agent B   │                      │
│   Revwr] │                  │  Fixer     │                      │
│  [Bldr]  │                  └────────────┘                      │
│  [Test]  │                                                      │
├──────────┴──────────────────────────────────────────────────────┤
│  Node Config / Edge Config Panel                                 │
└─────────────────────────────────────────────────────────────────┘
```

#### 数据流：React Flow ↔ 后端

```
React Flow (前端)                     Database (后端)
  nodes: Node[]                       
  edges: Edge[]                       
       │                              
  保存时:                              
       ├─ nodes → workflow_nodes 表 (node_type, agent_id, config, position)
       ├─ edges → workflow_edges 表 (source, target, condition, data_mapping)
       │
  加载时:                              
       └─ workflow_nodes + workflow_edges → 转换为 React Flow nodes/edges
```

React Flow 的 `nodes` 和 `edges` 是前端运行时状态，保存时序列化为后端表格式。加载时从后端表反序列化为 React Flow 格式。**后端 executor 不接触 React Flow 的数据结构**。

---

## Tauri Commands

### Agent CRUD

```rust
#[tauri::command]
async fn create_agent(name, description, system_prompt, model, skills, tools,
    permission_mode, permission_rules, max_iterations, max_context_tokens,
    memory_enabled, icon, color, state) -> Result<AgentDef, String>;

#[tauri::command]
async fn list_agents(state) -> Result<Vec<AgentDef>, String>;

#[tauri::command]
async fn get_agent(id, state) -> Result<AgentDef, String>;

#[tauri::command]
async fn update_agent(id, updates, state) -> Result<AgentDef, String>;

#[tauri::command]
async fn delete_agent(id, state) -> Result<(), String>;

#[tauri::command]
async fn search_agent_memory(agent_id, query, top_k, state) -> Result<Vec<Value>, String>;

#[tauri::command]
async fn get_agent_history(agent_id, limit, state) -> Result<Vec<Value>, String>;

#[tauri::command]
async fn run_agent_standalone(agent_id, input, session_id, state) -> Result<String, String>;
```

### Workflow CRUD + Execution

```rust
#[tauri::command]
async fn create_workflow(name, description, state) -> Result<WorkflowDef, String>;

#[tauri::command]
async fn list_workflows(state) -> Result<Vec<WorkflowDef>, String>;

#[tauri::command]
async fn get_workflow(id, state) -> Result<WorkflowDef, String>;

#[tauri::command]
async fn save_workflow(id, name, description, nodes, edges, config, state) -> Result<WorkflowDef, String>;

#[tauri::command]
async fn delete_workflow(id, state) -> Result<(), String>;

#[tauri::command]
async fn validate_workflow(id, state) -> Result<ValidationResult, String>;

#[tauri::command]
async fn execute_workflow(id, input, session_id, state) -> Result<String, String>;

#[tauri::command]
async fn cancel_workflow_run(run_id, state) -> Result<(), String>;

#[tauri::command]
async fn get_workflow_run(run_id, state) -> Result<WorkflowRun, String>;

#[tauri::command]
async fn list_workflow_runs(workflow_id, limit, state) -> Result<Vec<WorkflowRun>, String>;
```

---

## Context 传递机制

### 结构化 State 传递（非字符串模板）

```
上游 Agent A 输出:
{
  "result": "代码审查完成，发现3个问题...",
  "issues": ["null check missing", "unwrap without context"],
  "success": true,
  "metadata": {"files_reviewed": 5}
}

       │
       │  Edge data_mapping:
       │  { "source_field": "result", "target_field": "context", "pass_through": false }
       │
       │  Edge condition:
       │  "success == true"  ← 条件路由
       ▼

下游 Agent B 输入 (结构化 JSON):
{
  "task": "根据审查结果修复代码",
  "context": "代码审查完成，发现3个问题...",   // ← 来自 Agent A.result
  "_shared": { ... },                         // ← 全局共享 state
  "_workflow_input": { ... }                   // ← 原始输入
}
```

### Agent 输入组装

结构化 JSON → 可读文本（注入 Subagent 的 user message）：

```rust
fn format_agent_input(node_config: &Value, input: &Value) -> String {
    let task_template = node_config.get("input_template")
        .and_then(|t| t.as_str()).unwrap_or("Complete the following task:");

    // 安全的模板渲染：只替换 {{field}} 不做递归求值
    let task = render_template_safe(task_template, input);

    let mut msg = format!("## Task\n{}\n\n", task);

    // 注入上游 context
    if let Some(obj) = input.as_object() {
        let context_fields: Vec<_> = obj.iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .collect();
        if !context_fields.is_empty() {
            msg.push_str("## Context from Upstream\n\n");
            for (key, val) in context_fields {
                msg.push_str(&format!("### {}\n{}\n\n", key, format_value(val)));
            }
        }
    }

    msg
}
```

### 条件分支示例

```
                    ┌─ condition: "success == true" ───► Agent B (Fixer)
Agent A (Reviewer) ─┤
                    └─ condition: "success == false" ──► Agent C (Reporter)
```

- Agent A 输出 `{success: true, result: "..."}` → 条件 `success == true` 满足 → Agent B 执行
- 条件 `success == false` 不满足 → Agent C 被标记为 `skipped`

---

## Agent 进化机制

### "越用越强" 的实现

每次自定义 Agent 执行时：

1. **执行前 — Memory 检索注入**：用 input 作为 query 检索 `agent_memory`（hybrid search: BM25 + HNSW + Salience），top-5 结果注入 Subagent 的 ContextEngine Active Memory segment。限制 2000 tokens。
2. **执行后 — Memory 存储**：对话存储到 `agent_memory`（user message + assistant output）。
3. **执行后 — History 记录**：记录到 `agent_history`（含 token/cost/latency）。
4. **定期 — Consolidation**：对 `agent_memory` 执行去重（`MemoryConsolidator`）和冷记忆淘汰（`prune_cold`）。
5. **Deep 模式 — Reflection**：`memory_enabled = 2` 时，后台从对话中提取事实写入 `agent_memory`（`source = "reflection"`）。

### Experimental: Auto-Skill Generation（人工确认后生效）

> **标记为 experimental**：自动 skill 生成的质量在业界未被证明可靠。产出为**草稿**，必须人工确认后才生效。

- Reflector 分析 `agent_history`，识别重复模式（如 "Code Reviewer" 连续 5 次都检查了 null safety）
- 生成 skill 草稿（`reflector-{slug}.md`），写入 `~/.agverse/skills/drafts/`
- 前端通知用户审核草稿
- 用户确认后移动到 `~/.agverse/skills/`，下次 SkillManager scan 时自动加载
- **未确认的草稿不会被加载**

---

## 隔离保证

| 维度 | 主 Agent | 自定义 Agent |
|------|---------|-------------|
| Memory 表 | `recall_memory` | `agent_memory` |
| Memory 索引 | 全局 BM25/HNSW | per-agent_id BM25/HNSW |
| Memory Mutex | `MemoryManager` 的 Mutex | `AgentMemoryStore` 的 Mutex |
| Session | `sessions` (type='main') | `sessions` (type='subagent') |
| Config | `config.toml` | `agents` 表 |
| 执行路径 | `RunManager` → `Run` | `WorkflowExecutor` → `Subagent` |
| 工具 | `ToolRegistry::with_defaults()` | `ToolRegistry::from_names()` 或继承 |
| 权限 | 全局 `PermissionConfig` | TrustMode + per-agent `PermissionConfig` |
| 事件流 | `broadcast::Sender<Envelope>` | 同一 broadcast（通过 `subagent_id` 区分） |

**主 Agent 的 Memory 操作完全不经过 `agent_memory` 表，自定义 Agent 的 Memory 操作完全不经过 `recall_memory` 表。两者使用不同的 Mutex，不存在锁竞争。**

---

## Tasks

### Phase 1: 后端基础设施（Agent 持久化 + Memory 隔离）

| ID | Task | Status |
|----|------|--------|
| T1 | 实现 8 张新表 schema + `add_column_if_not_exists` 迁移工具函数 | Todo |
| T2 | 实现 `AgentRegistry` (CRUD + `build_subagent_config` / `build_permission_config` / `build_model_config`) | Todo |
| T3 | 实现 `AgentMemoryStore` (store / search / search_hybrid / consolidate / build_context_injection) | Todo |
| T4 | 实现 `AgentHistoryStore` (record / list / get_recent) | Todo |
| T5 | 扩展 `Subagent`：新增 `new_with_memory()` + 2 个 field + memory 注入/存储逻辑 | Todo |
| T6 | 实现 Tauri Commands: agent CRUD + memory search + history + run_standalone | Todo |
| T7 | 验证：主 Agent 功能不受影响（现有测试全部通过） | Todo |

### Phase 2: 后端 Workflow 引擎

| ID | Task | Status |
|----|------|--------|
| T8 | 实现 `WorkflowDef` + `NodeDef` + `EdgeDef` + DB CRUD | Todo |
| T9 | 实现 `WorkflowPlanner` (Kahn 拓扑排序 + 环检测 + 并行分组) | Todo |
| T10 | 实现 `WorkflowContext` (结构化 state + data_mapping + 条件路由评估) | Todo |
| T11 | 实现 `TrustMode` (Inherit / Trusted / Readonly) | Todo |
| T12 | 实现 `WorkflowExecutor` (stage 并行 + semaphore + cancel 传播 + 节点级 token/cost 统计) | Todo |
| T13 | 实现 Tauri Commands: workflow CRUD + execute + cancel + run history | Todo |
| T14 | 新增事件类型: `WorkflowNodeStarted` / `WorkflowNodeEnded` / `WorkflowCompleted` | Todo |

### Phase 3: 前端 Agent CRUD

| ID | Task | Status |
|----|------|--------|
| T15 | 安装 `@xyflow/react` 依赖 | Todo |
| T16 | 实现 `agentSlice` + thunks | Todo |
| T17 | 扩展 `NewAgentModal` → `AgentEditor` (增加 tools/permissions/memory/max_iterations) | Todo |
| T18 | 实现 `AgentList` 侧边面板 | Todo |
| T19 | 实现 `AgentMemoryViewer` | Todo |

### Phase 4: 前端 Workflow Editor

| ID | Task | Status |
|----|------|--------|
| T20 | 实现 `workflowSlice` + thunks | Todo |
| T21 | 实现 `WorkflowEditor` 画布 (React Flow + 自定义节点/边) | Todo |
| T22 | 实现 `WorkflowSidebar` (Node Palette + Agent 拖拽) | Todo |
| T23 | 实现自定义节点组件 (Agent / Input / Output / Transform / Approval) | Todo |
| T24 | 实现 `EdgeConfigPanel` (data_mapping + condition 配置) | Todo |
| T25 | 实现 `WorkflowRunView` (节点状态可视化 + 事件流 + token/cost 显示) | Todo |

### Phase 5: 集成测试与打磨

| ID | Task | Status |
|----|------|--------|
| T26 | 端到端: 创建 Agent → Workflow 中使用 → 执行 → 验证 Memory 积累 | Todo |
| T27 | 回归测试: 主 Agent 功能完全正常 | Todo |
| T28 | Workflow 验证器 UI (环检测/孤立节点/缺失配置) | Todo |
| T29 | 条件路由端到端测试 | Todo |
| T30 | TrustMode 各模式测试 | Todo |

### Phase 6: Experimental — Agent 进化

| ID | Task | Status |
|----|------|--------|
| T31 | Reflector 分析 agent_history → 生成 skill 草稿 (写入 drafts/) | Todo |
| T32 | 前端 skill 草稿审核 UI | Todo |
| T33 | 确认后 skill 生效 + 加载测试 | Todo |

---

## Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| Agent Memory 的 per-agent BM25/HNSW 索引内存膨胀 | Med | Med | 索引 lazy-init + LRU 淘汰（不活跃 Agent 的索引超时卸载） |
| Workflow 节点超时导致整个 Workflow 卡住 | High | Med | 每节点 timeout（可配置）+ `on_node_failure` 策略（abort/continue/skip） |
| 大量 Agent 并行执行撞 LLM API rate limit | High | High | `Semaphore` 限制并行度（默认 3）+ 指数退避重试 |
| React Flow 画布数据与后端 DAG 不一致 | Med | Low | 保存时后端强制 `validate_workflow`；运行前再次验证 |
| Agent Memory 注入导致 context 过长 | Med | Med | 限制 top_k=5 + 总 token ≤ 2000 + salience 截断 |
| 主 Agent 性能受影响 | High | Low | 独立 Mutex + 独立表 + Workflow 执行在独立 tokio task |
| 条件表达式解析出错 | Med | Med | 解析失败时默认 `true`（传递）+ 记录 warning 日志 |
| Trusted 模式下 Agent 执行危险操作 | High | Med | 前端创建 Trusted workflow 时二次确认 + 审计日志 |
| Auto-skill 生成垃圾内容 | Med | Med | **Experimental**：草稿必须人工确认后生效 |

---

## Success Criteria

1. 用户可以在 `AgentEditor` 中创建自定义 Agent，保存后持久化到 SQLite。
2. 用户可以查看/编辑/删除 Agent。
3. 用户可以在 React Flow 画布上拖拽 Agent 节点，连线组成 Workflow，保存到数据库。
4. 用户可以配置**节点级 Router**（如 `success == true` 时走 Fixer，否则走 Reporter），Workflow 执行时自动路由。
5. 用户可以设置 Workflow 的 **TrustMode**（Trusted 模式下 bash/write 自动放行）。
6. 用户点击 Run 执行 Workflow，前端实时显示每个节点的运行状态、token 消耗、延迟。
7. 每次自定义 Agent 执行后，对话存储到该 Agent 的独立 Memory。
8. 同一 Agent 再次执行时，能检索到之前的 Memory 并注入 context。
9. 主 Agent（现有 chat）的行为和性能不受任何影响。
10. 用户可以取消正在执行的 Workflow，所有节点停止。
11. Workflow 执行结果和每个节点的输入/输出/token/cost 可追溯。
12. (Experimental) Reflector 可生成 skill 草稿，用户审核后生效。

---

## Open Questions

1. **Agent Memory 跨 Agent 共享？**
   - V1 已支持 opt-in 的 `memory_group`：同组 agent 共享一个 memory 索引（`memory_key` = group name）。默认 `memory_group` 为空（per-agent 隔离）。是否需要更细粒度的共享规则（如 read-only access to other agent's memory）留到 V2。

2. **Workflow 是否支持循环？**
   - 建议：V1 仅 DAG。通过 `HumanApproval` 节点 + 手动重跑实现 "审查→修复→再审查" 循环。V2 考虑 `Loop` 节点。

3. **Agent system prompt 是否支持模板变量？**
   - 建议：V1 不支持。system prompt 是静态文本。V2 考虑 Jinja2 模板。

4. **Workflow 是否嵌套？**
   - 建议：V1 不支持。V2 考虑 `SubWorkflow` 节点类型。

5. **`teams/` 模块的处置？**
   - `core/src/teams/`（`AgentTeam` + `MessageBus`）与 `WorkflowContext` 是两种不同的通信模型（异步 inbox vs 同步 state）。为避免并存导致困惑，**标记 `teams/` 为 deprecated**，在模块顶部添加 `#![deprecated]` 注释。V1 不删除代码（保持编译），但不接入新功能。V2 如果做循环工作流（agent 间来回协商），再重新启用 MessageBus。V3 如确认不用则移除。

6. **前端 Workflow 入口位置？**
   - 建议：Sidebar 新增 "Workflows" 入口，打开全屏画布视图。不干扰现有 Chat 视图。

7. **Node 类型是否用 trait 层次替代扁平枚举？**
   - V1 用扁平枚举（`agent` / `input` / `output` / `transform` / `human_approval`），简单够用。V3 如果做插件系统（第三方 node 类型），再迁移到 `WorkflowNode` trait + 具体实现。

---

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-30 | agent_core | Created as Draft (v1) |
| 2026-06-30 | agent_core | Rev1: (1) Explicit Subagent as execution primitive, (2) Dropped "zero modification" claim, (3) Conditional routing moved to V1, (4) Structured state replaces string templates, (5) Added TrustMode, (6) Added migration framework, (7) Added token/cost/latency fields, (8) Added semaphore + cancel propagation, (9) Auto-skill marked experimental, (10) Aligned positioning as DAG workflow |
| 2026-06-30 | agent_core | Rev2: (1) Added SubagentConfig extension design with full AgentDef→SubagentConfig mapping, (2) Added Skills pathway design (Tool injection + Content injection dual path), (3) Added Memory Sharing Group (`memory_group` / `memory_key`), (4) Replaced per-edge conditions with node-level Router model (LangGraph-style), (5) Added teams/ deprecation note, (6) Added Node trait hierarchy as Open Question for V3 |

---
*Generated by AI Agent (agent_core)*
*Model: glm-latest | Timestamp: 2026-06-30T00:00:00+08:00*
