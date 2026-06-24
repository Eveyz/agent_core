# PLAN-0001: Agent 自主规划 Todo 系统

```yaml
---
id: PLAN-0001
type: PLAN
title: Agent Self-Planning Todo System
status: Draft
author: zniverse
created: 2026-06-25
updated: 2026-06-25
reviewers: []
related: [ADR-0001]
supersedes: ~
superseded_by: ~
tags: [agent, planning, todo, frontend]
---
```

## Objective

让 Agent 对复杂任务自主生成 todo list，按列表逐步执行并勾选完成，前端实时展示计划进度。对标 Claude Code / Codex 的 plan-then-execute 体验。

## Background

当前代码库已有 `TodoList` 数据结构和 `todo_write` / `todo_read` / `todo_update` 三个 tool，但存在三个断裂：

1. **Runtime 路径未接入** — TodoList 只在 CLI 的 `main.rs` 手动注册，`Brain` / `Run` 完全不知道它的存在
2. **Context 断流** — Segment 7 `execution_plan` 存在但永远是空的，模型看不到自己的 plan，无法保持方向感
3. **前端无感知** — `RunEvent` 没有 todo 相关事件，前端无法展示 plan 进度

此外，现有 `todo_write` 是逐条添加（传 id + description），不如 Claude Code 的批量覆盖模式好用。

## Scope

### In Scope

- `Brain` 持有 `Arc<Mutex<TodoList>>`，`build_tool_registry` 自动注册 todo tools
- `todo_write` 改为批量覆盖模式（传 items 数组，一次性替换整个 list）
- `Run::refresh_context_segments` 将 todo 注入 Segment 7 `execution_plan`
- 新增 `RunEvent::TodoUpdated` 事件，tool 执行后推送当前 todo 快照
- Tauri bridge 透传事件（已有 `agent-event` 机制，无需新 command）
- 前端 `ChatState` 增加 `todo` 字段，处理 `todo_updated` 事件
- 前端新增 `TodoPanel` 组件，持久展示计划进度
- `prompt.rs` 更新规划引导

### Out of Scope

- TaskBoard / Task DAG 重构（已有体系，不碰）
- 前端 Redux 扁平化重构（见 `ai_proposals/2026-06-23_Architecture_Data_Flow_Diagrams.md`，另案）
- TodoList 持久化到 SQLite（后续可加，本次不做）
- Subagent 内部 todo（子 Agent 用自己的 context，不共享主 Agent 的 todo）

## Design

### 数据流全景

```
模型调用 todo_write(items)
  → Tool 修改 Brain.todo_list (Arc<Mutex<TodoList>>)
  → Tool 返回 list 摘要作为 result string
  → Run 检测到 todo tool 被调用
  → Run emit RunEvent::TodoUpdated { items: [...] }
  → Tauri emit("agent-event", envelope)
  → Redux chatSlice 处理 todo_updated → state.todo 更新
  → TodoPanel 组件重渲染

下一 turn:
  → Run::refresh_context_segments()
  → brain.todo_list.lock().to_context_string()
  → context.set_execution_plan(plan_str)
  → 模型在 system prompt 中看到:
    "== Current Plan ==
     [x] 1 completed: Read auth module
     [~] 2 in_progress: Add OAuth handler
     [ ] 3 pending: Write tests"
```

### 后端改动

#### 1. Brain 持有 TodoList

`core/src/runtime/brain.rs`:

```rust
pub struct Brain {
    pub config: Config,
    pub memory: Option<Arc<Mutex<MemoryManager>>>,
    pub skill_manager: Option<Arc<Mutex<SkillManager>>>,
    pub todo_list: Arc<Mutex<TodoList>>,        // ← 新增
    current_model_name: String,
}
```

`build_tool_registry()` 中注册：
```rust
crate::tools::todo::register_todo_tools(&mut registry, self.todo_list.clone());
```

#### 2. todo_write 改为批量覆盖

`core/src/tools/todo.rs` — 重新设计 `TodoWriteTool`：

```
工具名: todo_write
参数: { items: [{ description: string, status?: string, depends_on?: string[] }] }
行为: 清空现有 list，用传入的 items 重建（自动分配 ID 1,2,3...）
返回: 新 list 的 to_context_string()
```

保留 `todo_read`（返回当前 list）和 `todo_update`（单条状态更新）。

#### 3. Run 注入 execution_plan + 发事件

`core/src/runtime/run.rs` — `refresh_context_segments()` 末尾加：

```rust
// Segment 7: EXECUTION PLAN
let plan_str = self.brain.todo_list.lock().to_context_string();
if !plan_str.is_empty() {
    self.context.set_execution_plan(&plan_str);
}
```

`run_turn()` 中，tool 执行完毕后加：

```rust
// 如果 todo tool 被调用，推送当前 todo 快照
let todo_changed = tool_calls.iter().any(|c| {
    matches!(c.function.name.as_str(), "todo_write" | "todo_update")
});
if todo_changed {
    let items = self.brain.todo_list.lock().items.clone();
    self.emit(RunEvent::TodoUpdated { items });
}
```

#### 4. 新增 RunEvent 变体

`core/src/runtime/event.rs`:

```rust
pub enum RunEvent {
    // ... 现有变体 ...

    // ── Planning ───────────────────────────────────────────────
    TodoUpdated {
        items: Vec<TodoItemPayload>,
    },
}
```

其中 `TodoItemPayload`：
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItemPayload {
    pub id: String,
    pub description: String,
    pub status: String,        // "pending" | "in_progress" | "completed" | "blocked"
}
```

从 `TodoItem` 转换时忽略 `depends_on` / `created_at` / `completed_at`（前端不需要）。

#### 5. Prompt 更新

`core/src/prompt.rs` — `DEFAULT_PRINCIPLES` 中的 Task Decomposition 段替换为：

```
## Planning Protocol

For complex tasks (3+ steps, multi-file, "implement"/"refactor"/"add feature"):
1. FIRST call todo_write with a list of steps
2. Before starting each step, call todo_update to mark it in_progress
3. After completing each step, call todo_update to mark it completed
4. If the plan changes, call todo_write again with the updated list

For simple tasks (1-2 tool calls): just do them, skip the todo list.
```

删除现有的 "Task Decomposition Protocol" + "task_create" + "task_plan" + "task_ready" + "task_execute" 相关内容（那是 TaskBoard 的，和 todo 是两套东西，混在一起只会让模型困惑）。

### Legacy Agent 路径同步

`core/src/agent/mod.rs` — `refresh_context_segments()` 同样补上 execution_plan 注入。
`AgentBuilder::build()` 中注册 todo tools（如果 CLI 路径也想用的话）。

### Tauri Bridge

**无需改动。** 现有 `send_message` 中的事件转发逻辑已经把所有 `RunEvent` 通过 `emit("agent-event", &event)` 推给前端。`TodoUpdated` 会自动透传。

### 前端改动

#### 1. Redux State

`app/src/features/chat/chatSlice.ts`:

```typescript
interface TodoItem {
  id: string;
  description: string;
  status: 'pending' | 'in_progress' | 'completed' | 'blocked';
}

interface ChatState {
  // ... 现有字段 ...
  todo: TodoItem[];           // ← 新增
}
```

`RunEventPayload` 增加：
```typescript
export type RunEventType =
  | ... // 现有
  | 'todo_updated';           // ← 新增

// payload 字段
items?: TodoItem[];
```

`processSingleEvent` 增加分支：
```typescript
case 'todo_updated':
  state.todo = ev.items ?? [];
  break;
```

#### 2. TodoPanel 组件

`app/src/components/chat/TodoPanel.tsx`:

- 从 `state.chat.todo` 读取
- 渲染为 checklist：`[x]` / `[~]` / `[ ]` + description
- 进度条：`2/3 completed`
- 当 `todo` 为空时不渲染
- 位置：聊天区域上方，持久展示（跨 turn 不消失）
- 新 Run 开始时清空（`run_started` 事件或 `userMessageSent` 时 `state.todo = []`）

#### 3. Run 结束时清空

`handleAgentEnd` 中加 `state.todo = []`，避免上一个任务的 plan 残留。

## Tasks

| ID | Task | 涉及文件 | Status |
|----|------|---------|--------|
| T1 | Brain 持有 TodoList + 注册 tools | `core/src/runtime/brain.rs` | Todo |
| T2 | todo_write 改为批量覆盖 | `core/src/tools/todo.rs` | Todo |
| T3 | Run 注入 execution_plan + emit TodoUpdated | `core/src/runtime/run.rs`, `core/src/runtime/event.rs` | Todo |
| T4 | Legacy Agent 同步 execution_plan 注入 | `core/src/agent/mod.rs` | Todo |
| T5 | Prompt 更新 | `core/src/prompt.rs` | Todo |
| T6 | 前端 chatSlice 处理 todo_updated | `app/src/features/chat/chatSlice.ts` | Todo |
| T7 | TodoPanel 组件 | `app/src/components/chat/TodoPanel.tsx` | Todo |
| T8 | cargo test + cargo check | — | Todo |

## Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| 模型不主动 plan | Med | Med | Prompt 引导 + 给 few-shot 例子 |
| todo_updated 事件频率过高 | Low | Low | 只在 todo tool 被调用时 emit，不是每 turn |
| TodoList 并发冲突 | Low | Low | `Arc<Mutex<TodoList>>`，lock 时间极短 |
| 前端 plan 残留 | Low | Med | Run 结束时清空 state.todo |

## Success Criteria

- 复杂任务（如 "implement X"）时模型自主调用 `todo_write` 生成计划
- 前端实时展示 todo 进度，checkbox 随 `todo_update` 更新
- 模型在后续 turn 的 system prompt 中能看到当前 plan 状态
- 简单任务（如 "read main.rs"）不触发 todo
- `cargo test` 全部通过
- `cargo check` 无 warning

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-25 | zniverse | Created as Draft |
