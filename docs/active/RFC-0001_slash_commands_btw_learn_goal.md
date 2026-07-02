# RFC-0001: Slash Commands — /btw, /learn, /goal

```yaml
---
id: RFC-0001
type: RFC
title: "Slash Commands: /btw (Ephemeral Side-Channel), /learn (Persistent Memory), /goal (Goal-Pinned Task Tracking)"
status: Draft
author: agent_core
created: 2026-07-02
updated: 2026-07-02
reviewers: []
related: [ADR-0001, PLAN-0001, PLAN-0002]
supersedes: ~
superseded_by: ~
tags: [commands, memory, agent, ui, input]
---
```

## Summary

实现三个输入框斜杠命令：`/btw`（临时旁路问答，不污染主上下文，可与主 run 并行）、`/learn`（将经验固化为持久记忆）、`/goal`（设定置顶目标，驱动任务分解与自省追踪）。三者形成"试探→固化→驱动"的完整闭环。

## Motivation

当前输入框的 `/` 命令仅有 `/subagents`、`/clear`、`/help` 三个静态条目，`/btw` 虽在列表中但后端未实现。用户缺少三种关键交互能力：

1. **无干扰旁路提问**：Agent 正在执行长任务时，用户无法在不打断主流程、不污染上下文的前提下快速提问。
2. **无法固化经验**：用户在 Debug 中获得的经验无法一键沉淀为持久记忆，下次同类问题需重新排查。
3. **目标容易漂移**：长对话中 AI 容易跑题，缺少一个"置顶目标"机制来驱动任务拆解、进度追踪和自我纠偏。

### 现有基础设施

| 能力 | 现状 | 可复用程度 |
|------|------|-----------|
| 斜杠命令 autocomplete | `useAutocomplete.ts` 中静态 `COMMANDS` 数组，4 条目 | 直接扩展 |
| 消息发送 | `invoke('send_message', { message, sessionId })` → 创建 Run | `/goal` 可复用；`/btw` 需新通道；`/learn` 需新通道 |
| 上下文注入 | `ContextEngine` 管理对话历史，`steer_run` 可中途注入消息 | `/btw` 参考但需隔离 |
| 持久记忆 | `MemoryManager`（Core/Recall/Archival 三层），`core_memory` block 可读写 | `/learn` 直接复用 |
| 反思学习 | `Reflector`（离线）、`DiffPreferenceEngine`（diff 观察） | `/learn` 可复用反思的提炼逻辑 |
| Todo 追踪 | `RunEvent::TodoUpdated` + 前端 `state.todo` + `TodoPanel` | `/goal` 复用 TodoPanel |
| 事件系统 | `RunEvent` enum + broadcast channel + JSONL 持久化 | 需新增 2 个事件类型 |
| LLM client | `Brain` 持有 `client_factory`，可低成本创建独立实例 | `/btw` 并行所需 |

## Detailed Design

### Goals

- `/btw`：发起单次 LLM 问答，读取当前上下文但不写入主历史，**支持与主 run 并行执行**，结果以临时气泡展示
- `/learn`：将用户输入通过**轻量模型**提炼为结构化记忆条目，写入 `core_memory`，跨 Session 生效
- `/goal`：设定置顶目标，自动触发任务拆解（todo），复用现有 `TodoPanel` 渲染，在后续每轮自省检查进度

### Non-Goals

- 不实现 `/learn` 的向量嵌入索引（首版仅写入 core_memory block）
- 不实现 `/goal` 的全自动 autopilot（首版仅追踪 + 提醒，不自动执行子任务）
- 不实现 `/goal` 的跨 Session 持久化（目标仅存在于当前 run 生命周期内）
- 不修改 `send_message` 的现有行为（三个命令走独立通道）
- 不为三命令添加 tool approval（`/btw` 无工具权限，`/learn` 仅写 memory 文件，`/goal` 仅注入 system prompt）
- 不实现 `/learn` 的即时去重（交由已有 `MemoryConsolidator` 异步处理）

---

### 1. `/btw` — Ephemeral Side-Channel（支持与主 run 并行）

#### 1.1 交互流程

```
用户输入: /btw Python asyncio 怎么防止死锁？
  ↓
前端: 解析 /btw 前缀，提取 question = "Python asyncio 怎么防止死锁？"
  ↓
前端: dispatch(btwAsked({ question }))  → 在 chat 中插入临时气泡（用户侧）
  ↓
前端: invoke('btw_query', { sessionId, question })
       ⚡ 不检查 isProcessing — 主 run 可同时执行
  ↓
后端: 读取当前 session 的 context_engine 快照（只读，不取锁）
后端: 通过 brain.client_factory.create() 创建独立 LLM client 实例
后端: 构造单次 LLM 请求（system: "基于当前项目上下文简短回答", messages: [context_snapshot, user_question]）
后端: 流式返回 → emit("btw-event", { btw_id, type: "delta", text })
后端: 完成 → emit("btw-event", { btw_id, type: "done" })
  ↓
前端: 流式更新临时气泡（agent 侧），完成后气泡可关闭
  ↓
关键: 不调用 context.add()，不写入 session messages，不写入 event log
       与主 run 完全隔离，互不阻塞
```

#### 1.2 并行执行设计

`/btw` 的核心价值在于"主 run 在工作时随时可问"。实现要点：

1. **独立 LLM client 实例**：`btw_query` 通过 `brain.client_factory.create()` 创建全新的 client，不与主 run 的 client 共享连接状态。
2. **只读 context 快照**：读取 `context_engine` 的当前消息列表的不可变引用（`Arc<Vec<Message>>` 或 clone），不获取写锁。主 run 可以继续往 context 写入新消息，`/btw` 用的是快照时的版本。
3. **独立事件通道**：`/btw` 的流式 delta 通过 `app_handle.emit("btw-event", ...)` 直接发送，不走 Run 的 broadcast channel。前端通过独立的事件监听器接收。
4. **无工具权限**：`/btw` 请求不注册任何 tool，LLM 只能纯文本回答，不会触发文件操作或命令执行，避免与主 run 产生资源竞争。
5. **Prompt cache 复用**：由于 context 快照与主 run 的 prefix 重叠，LLM provider 的 prompt cache 会自动命中，`/btw` 只需为新增的 question 和 answer 支付 token。

```
时间线示例:

主 Run:   ████████████ tool exec ████████████ model call ████████████ ...
                ↑                                                ↑
BTW #1:         ──── delta delta delta done ────
BTW #2:                                     ──── delta delta done ────

两个 /btw 都不打断主 run，各自独立流式返回
```

#### 1.3 前端设计

**Redux 状态扩展** (`chatSlice.ts`):

```typescript
// types.ts 新增
interface BtwEntry {
  id: string;
  question: string;
  answer: string;
  isStreaming: boolean;
  startTime: number;
  endTime?: number;
}

// ChatState 新增
interface ChatState {
  // ...existing
  btwEntries: BtwEntry[];
}
```

**Reducer**:
- `btwAsked`: push `{ id, question, answer: '', isStreaming: true, startTime }`
- `btwDelta`: 找到 entry by id，`answer += delta`
- `btwDone`: `isStreaming = false, endTime = now`

**事件监听**：在 `useAgentEventListener` hook 中新增 `"btw-event"` 监听器，dispatch 对应 reducer。

**UI 渲染**: 在 `ChatArea` 中，`btwEntries` 渲染为特殊样式的气泡（区别于主对话），带有 "BTW" 标签和关闭按钮。不与主 `entries` 混合。即使 `isProcessing === true`，`/btw` 的输入和渲染也不受影响。

#### 1.4 后端设计

**新 Tauri 命令** (`lib.rs`):

```rust
#[tauri::command]
async fn btw_query(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
    session_id: String,
    question: String,
) -> Result<String, String> {
    let brain = state.brain.clone();
    let btw_id = uuid::Uuid::new_v4().to_string();

    tokio::spawn(async move {
        // 1. 获取 context 只读快照（clone 当前消息列表，不持锁）
        let snapshot = brain.get_context_snapshot(&session_id).await;

        // 2. 创建独立 LLM client（不复用主 run 的 client）
        let client = brain.client_factory.create();

        // 3. 构造单次请求（无工具）
        let system_prompt = format!(
            "You are a helpful assistant. Answer concisely based on the project context.\n\
             Do not use tools. Keep your answer brief and focused.\n\n\
             --- Project Context ---\n{}",
            snapshot
        );
        let messages = vec![
            Message::system(system_prompt),
            Message::user(question),
        ];

        // 4. 流式调用
        match client.stream(&messages, &brain.btw_model, None).await {
            Ok(mut stream) => {
                while let Some(chunk) = stream.next().await {
                    let _ = app_handle.emit("btw-event", BtwEvent {
                        btw_id: btw_id.clone(),
                        event_type: "delta",
                        text: chunk,
                    });
                }
            }
            Err(e) => {
                let _ = app_handle.emit("btw-event", BtwEvent {
                    btw_id: btw_id.clone(),
                    event_type: "error",
                    text: e.to_string(),
                });
                return;
            }
        }

        let _ = app_handle.emit("btw-event", BtwEvent {
            btw_id: btw_id.clone(),
            event_type: "done",
            text: String::new(),
        });
    });

    Ok(btw_id)
}
```

**关键约束**:
- 不调用 `context.add()` — 问答不进入主上下文
- 不调用 `session_manager.save()` — 不持久化到 session
- 不写入 JSONL event log
- 不注册 tool — LLM 无法执行任何工具
- 创建独立 client — 不与主 run 共享连接
- `brain.btw_model` — 轻量模型配置（见 §1.5）

#### 1.5 BTW 模型配置

在 `config.toml` 中新增:

```toml
[btw]
# model = "gpt-4o-mini"        # 轻量模型，成本低、响应快
# max_tokens = 1000             # 限制回答长度
# temperature = 0.3             # 低温度，回答更确定
```

默认使用 `gpt-4o-mini`（或当前 provider 的等价轻量模型）。`/btw` 的提问通常是概念性/解释性的，不需要最强模型的推理能力。

#### 1.6 前端命令解析

在 `ChatInput.tsx` 的 `handleSend` 中，发送前拦截:

```typescript
const handleSend = useCallback(() => {
  const trimmed = input.trim();
  if (!trimmed) return;
  
  // 命令拦截 — /btw 和 /learn 不检查 isProcessing
  if (trimmed.startsWith('/btw ')) {
    const question = trimmed.slice(5).trim();
    if (question) {
      setInput('');
      onBtwQuery(question);  // 新 prop
    }
    return;
  }
  if (trimmed.startsWith('/learn ')) {
    const content = trimmed.slice(7).trim();
    if (content) {
      setInput('');
      onLearn(content);  // 新 prop
    }
    return;
  }
  // /goal 走正常 send_message 通道，需要创建 run
  if (trimmed.startsWith('/goal ')) {
    // 不拦截 — 交给正常 handleSend 流程
    // 后端在 run.run() 中解析 /goal 前缀
  }
  
  // 原有逻辑（包括 /goal）...
  if (isProcessing) return;
  setInput('');
  onSend(trimmed);
}, [input, isProcessing, onSend, onBtwQuery, onLearn]);
```

**关键**：`/btw` 和 `/learn` 的拦截在 `isProcessing` 检查之前，因此主 run 执行中也可使用。

---

### 2. `/learn` — Persistent Memory Injection

#### 2.1 交互流程

```
用户输入: /learn 以后别用 X 库的 fetch 方法，它有内存泄露，改用 Y
  ↓
前端: 解析 /learn 前缀，提取 content
前端: dispatch(learnRequested({ content })) → 显示 "learning..." 状态
  ↓
前端: invoke('learn_memory', { sessionId, content })
       ⚡ 不检查 isProcessing — 可与主 run 并行
  ↓
后端: 通过轻量模型提炼 content → 结构化记忆条目
       (model: gpt-4o-mini, system: "Extract a durable rule. Output JSON.")
  ↓
后端: 写入 core_memory (追加到 ~/.agverse/agverse.md 的 memory block)
后端: emit("learn-event", { type: "saved", memory: { title, rule } })
  ↓
前端: 显示 "✓ Learned: {title}" 确认气泡
```

#### 2.2 后端设计

**新 Tauri 命令** (`lib.rs`):

```rust
#[tauri::command]
async fn learn_memory(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
    session_id: Option<String>,
    content: String,
) -> Result<(), String> {
    let brain = state.brain.clone();

    tokio::spawn(async move {
        // 1. 使用轻量模型提炼
        let client = brain.client_factory.create();
        let extraction = match client.complete(LEARN_SYSTEM_PROMPT, &content, &brain.learn_model).await {
            Ok(resp) => resp,
            Err(e) => {
                let _ = app_handle.emit("learn-event", LearnEvent {
                    event_type: "error",
                    error: e.to_string(),
                });
                return;
            }
        };

        let memory_entry: MemoryEntry = match serde_json::from_str(&extraction) {
            Ok(entry) => entry,
            Err(_) => {
                // JSON 解析失败，降级为原始内容
                MemoryEntry {
                    title: content.chars().take(60).collect(),
                    rule: content,
                    category: "knowledge",
                    tags: vec![],
                }
            }
        };

        // 2. 写入 core_memory
        let mut core = brain.memory.core_memory.write();
        core.add_block(MemoryBlock {
            label: memory_entry.title,
            content: memory_entry.rule,
            category: memory_entry.category,
            tags: memory_entry.tags,
        });
        core.save_to_file();  // 持久化到 ~/.agverse/agverse.md

        // 3. 通知前端
        let _ = app_handle.emit("learn-event", LearnEvent {
            event_type: "saved",
            title: memory_entry.title,
            rule: memory_entry.rule,
        });
    });

    Ok(())
}
```

**LLM 提炼 Prompt**:
```
You are a memory extraction assistant. The user wants to save a learning.
Extract a durable, generalizable rule from their input.

Output JSON only (no markdown, no code fences):
{
  "title": "short title (max 60 chars)",
  "rule": "the actionable rule, written as an instruction",
  "category": "preference | bug_avoidance | environment | convention | knowledge",
  "tags": ["relevant", "tags"]
}

User input: {content}
```

**Learn 模型配置**:

```toml
[learn]
# model = "gpt-4o-mini"        # 轻量模型，提炼任务简单且频繁
# max_tokens = 500
# temperature = 0.1             # 极低温度，结构化输出更稳定
```

#### 2.3 前端设计

**Redux 状态**:
```typescript
interface LearnEntry {
  id: string;
  input: string;
  status: 'pending' | 'saved' | 'error';
  title?: string;
  rule?: string;
  timestamp: number;
}

// ChatState 新增
learnEntries: LearnEntry[];
```

**Reducer**:
- `learnRequested`: push `{ status: 'pending' }`
- `learnSaved`: 更新 `status: 'saved', title, rule`
- `learnError`: 更新 `status: 'error'`

**UI**: 在 ChatArea 中渲染为带灯泡图标的卡片，显示提炼后的标题和规则，带 "Learned" 标签。

#### 2.4 跨 Session 生效

`core_memory` 已在 `Brain::new()` 中加载，每次 `create_run_with_workdir` 都会注入到 system prompt。因此 `/learn` 写入的条目在下一次 `send_message` 时自动生效，无需额外处理。

#### 2.5 去重

不在 `/learn` 时做即时去重。已有 `MemoryConsolidator`（每 20 turns 运行）会异步进行记忆合并和去重。用户可以放心多次 `/learn`，冗余条目会被 Consolidator 清理。

---

### 3. `/goal` — Goal-Pinned Task Tracking

#### 3.1 交互流程

```
用户输入: /goal 实现用户的登录注册功能，并包含 JWT 校验
  ↓
前端: 不拦截 /goal — 走正常 handleSend → invoke('send_message', { message: trimmed, sessionId })
       (复用现有 send_message 通道，/goal 前缀由后端解析)
  ↓
后端: send_message → create_run → run.run()
后端: run.run() 检测到 /goal 前缀 → 解析出 goal
后端: 1. 将 goal 存入 run.goal (新字段)
       2. emit GoalSet 事件
       3. 将 goal 注入 system prompt (高优先级区域)
       4. 触发 LLM 任务拆解 → 生成 todo list
       5. emit TodoUpdated 事件
  ↓
后续每轮 turn:
  后端: 检查 todo 完成情况，若偏离 goal 则在回复末尾追加提醒
  后端: 更新 todo → emit TodoUpdated
  ↓
前端: 目标横幅（AppHeader 下方）+ TodoPanel 实时更新
所有 todo 完成 → emit GoalCompleted 事件 → 前端显示完成状态
```

#### 3.2 后端设计

**Run 结构扩展** (`run.rs`):

```rust
pub struct Run {
    // ...existing fields
    goal: Option<String>,           // 置顶目标
    goal_completed: bool,           // 目标是否已完成
}
```

**Goal 注入** (`run.rs` `run()` method):

```rust
// 在添加 user message 之前，解析 /goal 前缀
if message.starts_with("/goal ") {
    let g = message.strip_prefix("/goal ").unwrap().trim().to_string();
    self.goal = Some(g.clone());

    // 注入 goal 到 context（高优先级 system 消息）
    let goal_prompt = format!(
        "## PRIMARY GOAL (pinned)\n\
        {}\n\n\
        You MUST keep this goal in mind throughout the conversation. \
        Break it into subtasks, track progress, and proactively drive toward completion. \
        If the conversation drifts, remind the user of this goal.",
        g
    );
    self.context.add_system_note(&goal_prompt);

    // 发出 GoalSet 事件
    self.emit_event(RunEvent::GoalSet { goal: g.clone() });

    // 触发任务拆解
    let todos = self.decompose_goal(&g).await?;
    self.emit_event(RunEvent::TodoUpdated { items: todos });
}
```

**目标拆解** (`run.rs`):

```rust
async fn decompose_goal(&self, goal: &str) -> Result<Vec<TodoItemPayload>, RunError> {
    let prompt = format!(
        "Break down this goal into concrete, actionable subtasks (3-8 items).\n\
        Output JSON array: [{{\"description\": \"...\", \"status\": \"pending\"}}]\n\n\
        Goal: {goal}"
    );
    let response = self.client.complete(GOAL_DECOMPOSE_SYSTEM, &prompt).await?;
    let items: Vec<TodoItemPayload> = serde_json::from_str(&response)?;
    Ok(items)
}
```

**每轮自省** (`run.rs` `run_turn()` method, 在 LLM 回复后):

```rust
// 检查 goal 进度
if let Some(ref goal) = self.goal {
    if !self.goal_completed {
        let progress = self.assess_goal_progress(goal).await?;

        if progress.all_completed {
            self.goal_completed = true;
            self.emit_event(RunEvent::GoalCompleted { goal: goal.clone() });
        } else if progress.drifted {
            // 在回复后追加提醒
            self.context.add_system_note(
                &format!("Reminder: The primary goal is still active: {}", goal)
            );
        }
    }
}
```

#### 3.3 前端设计

**Redux 状态**:
```typescript
interface ChatState {
  // ...existing
  goal: string | null;
  goalCompleted: boolean;
}
```

**新事件处理** (`eventHandlers.ts`):
```typescript
case 'goal_set':
  state.goal = ev.goal ?? null;
  state.goalCompleted = false;
  break;
case 'goal_completed':
  state.goalCompleted = true;
  break;
```

**新 RunEvent** (`event.rs`):
```rust
GoalSet { goal: String },
GoalCompleted { goal: String },
```

**UI 渲染**:
- **目标横幅**：在 `AppHeader` 下方渲染一个紧凑的目标横幅，显示当前 goal 文本和 todo 完成进度（`x/y completed`）。目标完成后横幅变为绿色完成态。
- **Todo 面板**：复用现有 `TodoPanel` 组件（已通过 `state.todo` 渲染）。`/goal` 的任务拆解通过 `TodoUpdated` 事件更新 `state.todo`，无需额外渲染逻辑。
- **横幅交互**：点击横幅可滚动到 TodoPanel 位置或展开/收起 TodoPanel。

---

### 4. 命令注册表更新

#### 4.1 前端 COMMANDS 数组

```typescript
const COMMANDS: AutocompleteItem[] = [
  { label: 'btw',    value: '/btw ',    icon: 'command', description: 'Ask a side question without polluting context' },
  { label: 'learn',  value: '/learn ',  icon: 'command', description: 'Save a learning to persistent memory' },
  { label: 'goal',   value: '/goal ',   icon: 'command', description: 'Set a pinned goal with task decomposition' },
  { label: 'subagents', value: '/subagents ', icon: 'command', description: 'Enable subagent mode' },
  { label: 'clear',  value: '/clear',   icon: 'command', description: 'Clear the conversation' },
  { label: 'help',   value: '/help',    icon: 'command', description: 'Show available commands' },
];
```

为 `AutocompleteItem` 新增可选 `description` 字段，在 dropdown 中显示命令说明。

#### 4.2 输入框命令高亮

在 `highlightedHTML` 中，对 `/btw`、`/learn`、`/goal` 命令前缀添加特殊高亮（类似 `@mention` 的 token 样式），让用户明确知道输入的是命令而非普通文本。

---

### 5. 事件类型总览

| 事件 | 传输方式 | 持久化 | 写入主上下文 | 用途 |
|------|----------|--------|-------------|------|
| `btw-event` (delta/done/error) | `app_handle.emit` 独立通道 | 否 | 否 | `/btw` 流式回答（与主 run 并行） |
| `learn-event` (saved/error) | `app_handle.emit` 独立通道 | 否（但记忆本身持久化到文件） | 否 | `/learn` 完成通知 |
| `GoalSet` | Run broadcast | 是 (JSONL) | 是（goal 注入 system） | `/goal` 目标设定 |
| `GoalCompleted` | Run broadcast | 是 (JSONL) | 否 | `/goal` 目标完成 |
| `TodoUpdated` | Run broadcast | 是 (JSONL) | 否 | `/goal` 任务进度更新（复用现有事件） |

---

### 6. 决策记录

| # | 问题 | 决策 | 理由 |
|---|------|------|------|
| 1 | `/btw` 是否支持主 run 执行中并行 | **是，必须支持** | 这是 `/btw` 的核心价值——主 run 工作时随时可问。通过独立 LLM client + 只读 context 快照 + 独立事件通道实现 |
| 2 | `/learn` 提炼模型 | **轻量模型（gpt-4o-mini）** | 提炼任务简单且频繁，轻量模型成本低、响应快 |
| 3 | `/goal` 的 todo 渲染 | **复用 TodoPanel** | 已有组件和事件管道，横幅仅显示进度摘要 |
| 4 | `/goal` 跨 Session 持久化 | **不持久化** | 目标仅存在于当前 run 生命周期内，新 session 重新 `/goal` |
| 5 | `/learn` 去重 | **交给 MemoryConsolidator** | 已有异步合并机制（每 20 turns），无需即时去重 |
| 6 | 三命令是否需要 tool approval | **不需要** | `/btw` 无工具权限，`/learn` 仅写 memory 文件，`/goal` 仅注入 system prompt |

---

### API Changes

#### 新增 Tauri 命令（Non-breaking）

| 命令 | 参数 | 返回 | 可在 isProcessing 时调用 | 说明 |
|------|------|------|:---:|------|
| `btw_query` | `{ sessionId, question }` | `btw_id: String` | ✅ | 发起旁路问答（独立 client，并行） |
| `learn_memory` | `{ sessionId?, content }` | `()` | ✅ | 提炼并保存记忆（轻量模型） |

#### 新增 RunEvent（Non-breaking）

```rust
// event.rs 新增
GoalSet { goal: String },
GoalCompleted { goal: String },
```

#### 现有 `send_message` 无变更

`/goal` 命令复用现有 `send_message` 通道，后端在 `run.run()` 中解析前缀。不修改 `send_message` 签名。

#### 新增配置（Non-breaking）

```toml
[btw]
model = "gpt-4o-mini"
max_tokens = 1000
temperature = 0.3

[learn]
model = "gpt-4o-mini"
max_tokens = 500
temperature = 0.1
```

#### 前端 Redux 新增

- `ChatState.btwEntries: BtwEntry[]`
- `ChatState.learnEntries: LearnEntry[]`
- `ChatState.goal: string | null`
- `ChatState.goalCompleted: boolean`

### Backwards Compatibility

- 现有 `/subagents`、`/clear`、`/help` 行为不变
- 现有 `send_message` 签名不变
- 新增的事件类型是 enum 变体追加，不影响旧前端解析（`default` 分支忽略未知事件）
- `core_memory` 写入是追加操作，不破坏现有 block
- 前端新 Redux 字段有默认值，旧 session resume 不受影响
- 新增 `config.toml` section 有合理默认值，缺失时 fallback 到 `gpt-4o-mini`

## Timeline

| Phase | Description | ETA |
|-------|-------------|-----|
| 1 | 前端命令解析 + Redux 状态扩展 + 命令注册表更新 + description 字段 | 2 天 |
| 2 | `/learn` 后端（轻量模型提炼 + core_memory 写入）+ 前端确认气泡 | 1.5 天 |
| 3 | `/btw` 后端（独立 client + 只读快照 + 流式 emit）+ 前端气泡 + 并行测试 | 3 天 |
| 4 | `/goal` 后端（Run 扩展 + goal 注入 + 任务拆解 + 自省）+ 新 RunEvent | 2 天 |
| 5 | `/goal` 前端（目标横幅 + 事件处理 + TodoPanel 联动） | 1 天 |
| 6 | 集成测试 + 三命令联动验证（/btw 试探 → /learn 固化 → /goal 驱动） | 1.5 天 |

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-07-02 | agent_core | Created as Draft |
| 2026-07-02 | agent_core | 决策定型：6 个 Open Questions 全部 resolved，更新 /btw 为并行设计 |

---
*Generated by AI Agent (agent_core)*
*Model: glm-latest | Timestamp: 2026-07-02T00:00:00+08:00*
