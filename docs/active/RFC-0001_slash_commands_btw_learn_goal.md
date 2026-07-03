# RFC-0001: Slash Commands — /btw, /learn, /goal

```yaml
---
id: RFC-0001
type: RFC
title: "Slash Commands: /btw (Ephemeral Side-Channel), /learn (Persistent Memory), /goal (Goal-Pinned Task Tracking)"
status: Draft
author: agent_core
created: 2026-07-02
updated: 2026-07-03
reviewers: [code-review-pass-1]
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
| 上下文注入 | `ContextEngine`（7 段语义上下文，**per-Run 独立**）管理对话历史；`steer_run` 经 `RunCommand::Steer` 中途注入 | `/btw` 只读快照需经 RunManager 取活跃 Run 的 context |
| 持久记忆 | `MemoryManager`（Core/Recall/Archival 三层）；`CoreMemory` 基于 **SQLite**，`MemoryBlock { id, label, content, max_chars, updated_at }`，方法 `append/create/replace/get` | `/learn` 经 `memory.lock().core_mut()` 复用 |
| 反思学习 | `Reflector`（离线）、`DiffPreferenceEngine`（diff 观察） | `/learn` 可复用反思的提炼逻辑 |
| Todo 追踪 | `RunEvent::TodoUpdated` + `TodoItemPayload { id, description, status }`；`TodoList` 为 Brain 共享 `Arc<Mutex<TodoList>>` | `/goal` 复用 TodoPanel，注意跨 Run 共享 |
| 事件系统 | `RunEvent` enum + broadcast channel + JSONL 持久化；前端统一监听 `agent-event` 单通道 | 新增 2 个 RunEvent 变体；`/btw`、`/learn` 走独立 `btw-event`/`learn-event` 通道（不持久化、无 seq） |
| LLM client | `Brain::build_client()` 基于 `current_model_config()` 构建 `OpenAIClient`（含 fallback 链）；**无 `client_factory`** | `/btw`/`/learn` 新增 `build_client_for(purpose)` 按用途选模型 |

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
后端: 通过 brain.build_client_for("btw") 创建独立 LLM client（轻量模型）
后端: 构造单次请求（system + context_snapshot + user_question），client.chat_completion_stream(&msgs, &[]) 流式调用
后端: 流式返回 → app_handle.emit("btw-event", { btw_id, type: "delta", text })（独立通道）
后端: 完成 → emit("btw-event", { btw_id, type: "done" })
  ↓
前端: 流式更新临时气泡（agent 侧），完成后气泡可关闭
  ↓
关键: 不调用 context.add()，不写入 session messages，不写入 event log
       与主 run 完全隔离，互不阻塞
```

#### 1.2 并行执行设计

`/btw` 的核心价值在于"主 run 在工作时随时可问"。实现要点：

1. **独立 LLM client 实例**：`btw_query` 通过 `brain.build_client_for("btw")` 创建全新的 `OpenAIClient`（基于 `config.get_model(btw_model)`），不与主 run 的 client 共享连接状态。
2. **只读 context 快照（关键路径重构）**：Brain **不持有 context**——每个 Run 自带 `ContextEngine`。快照必须经 `RunManager` → 查找 session 的活跃 `RunHandle` → 读取其共享 context。当前 `RunHandle` 未暴露 context，需新增机制：在 `RunHandle` 增加 `Arc<RwLock<Vec<Message>>>`（由 Run 在 turn 边界刷新），`context_snapshot()` 读取其 clone（不持写锁）。主 run 可继续写 context，`/btw` 用快照版本。
3. **独立事件通道**：`/btw` 的流式 delta 通过 `app_handle.emit("btw-event", ...)` 直接发送，**不走** Run 的 broadcast channel，因此**不进 JSONL event log、无 seq 追踪**。前端需新增第二个 `listen("btw-event")`（现有 `useAgentEventListener` 仅监听 `agent-event`）。
4. **无工具权限**：`/btw` 请求不注册任何 tool，LLM 只能纯文本回答，不会触发文件操作或命令执行，避免与主 run 产生资源竞争。
5. **Prompt cache 复用（需验证）**：若 `/btw` 的 system prompt 前缀与主 run 稳定前缀完全一致，provider 的 prompt cache 会命中。但 `/btw` 的 system prompt 文案不同（"基于当前项目上下文简短回答"），cache 命中率可能不及预期；首版不依赖此优化。

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

**UI 渲染**: 在 `ChatArea` 中，`btwEntries` 渲染为特殊样式的气泡（区别于主对话），带有 "BTW" 标签和关闭按钮。不与主 `entries` 混合。即使 `isProcessing === true`，`/btw` 的输入和渲染也不受影响。`btwEntries` 需 per-session 隔离（`btwEntriesBySession`，类比 `entriesBySession`）。

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
    // AppState 不直接持有 brain —— 经 RunManager 访问；克隆 Arc<Brain> 后释放锁。
    let run_manager = state.run_manager.clone();
    let brain = run_manager.lock().await.brain().clone(); // Arc<Brain>（Brain: Clone）
    let btw_id = uuid::Uuid::new_v4().to_string();

    tokio::spawn(async move {
        // 1. context 只读快照 —— Brain 不持有 context，context 属于 Run。
        //    经 RunManager 查找 session 的活跃 RunHandle，读取其共享快照（clone，不持锁）。
        //    需新增 RunManager::context_snapshot_for_session() 与 RunHandle::context_snapshot()
        //    （后者读取 Run 在 turn 边界刷新的 Arc<RwLock<Vec<Message>>>）。
        let snapshot = run_manager
            .lock().await
            .context_snapshot_for_session(&session_id)
            .unwrap_or_default(); // 无活跃 run 时退化为空上下文

        // 2. 独立 client —— build_client_for("btw") 用配置的轻量模型构建（不复用主 run 的 client）。
        let client = match brain.build_client_for("btw") {
            Ok(c) => c,
            Err(e) => {
                let _ = app_handle.emit("btw-event", BtwEvent {
                    btw_id: btw_id.clone(), event_type: "error", text: e.to_string(),
                });
                return;
            }
        };

        // 3. 构造单次请求（无工具 —— 传空 tools 切表）。
        let system_prompt = format!(
            "You are a helpful assistant. Answer concisely based on the project context.\n\
             Do not use tools. Keep your answer brief and focused.\n\n\
             --- Project Context ---\n{}",
            render_snapshot(&snapshot)
        );
        let messages = vec![Message::system(system_prompt), Message::user(question)];

        // 4. 流式调用 —— chat_completion_stream(&[Message], &[ToolDefinition]) 返回 StreamEvent 流。
        match client.chat_completion_stream(&messages, &[]).await {
            Ok(mut stream) => {
                use futures::StreamExt;
                while let Some(item) = stream.next().await {
                    if let Ok(StreamEvent::TextDelta(text)) = item {
                        let _ = app_handle.emit("btw-event", BtwEvent {
                            btw_id: btw_id.clone(), event_type: "delta", text,
                        });
                    }
                }
            }
            Err(e) => {
                let _ = app_handle.emit("btw-event", BtwEvent {
                    btw_id: btw_id.clone(), event_type: "error", text: e.to_string(),
                });
                return;
            }
        }

        let _ = app_handle.emit("btw-event", BtwEvent {
            btw_id: btw_id.clone(), event_type: "done", text: String::new(),
        });
    });

    Ok(btw_id)
}
```

**关键约束**:
- 不调用 `context.add()` — 问答不进入主上下文
- 不调用 `session_manager.save()` — 不持久化到 session
- 不写入 JSONL event log（独立通道，无 seq 追踪）
- 不注册 tool — `chat_completion_stream(&msgs, &[])` 传空 tools，LLM 无法执行工具
- 创建独立 client — 不与主 run 共享连接
- context 快照经 `RunManager::context_snapshot_for_session()` 获取（见 §1.2）；轻量模型经 `build_client_for("btw")`（见 §1.5）

#### 1.5 BTW 模型配置

复用现有 provider/model 架构（`[providers.X.models.Y]` + `model_id`），**不**引入裸 `[btw]` section。在 `Config` 新增两个可选模型名（缺省回退到 `default_model`）：

```toml
# config.toml —— 引用一个已定义的轻量模型（provider/model 命名）
btw_model   = "openai/gpt-4o-mini"
learn_model = "openai/gpt-4o-mini"
```

```rust
// config.rs
pub struct Config {
    pub default_model: String,
    pub btw_model: Option<String>,   // /btw 用；缺省回退 default_model
    pub learn_model: Option<String>, // /learn 用；缺省回退 default_model
    // ...
}

// brain.rs —— 按用途构建 client（btw/learn 不需要 fallback 链）
pub fn build_client_for(&self, purpose: &str) -> Result<OpenAIClient> {
    let name = match purpose {
        "btw"   => self.config.btw_model.as_deref().unwrap_or(self.current_model_name()),
        "learn" => self.config.learn_model.as_deref().unwrap_or(self.current_model_name()),
        _ => self.current_model_name(),
    };
    let cfg = self.config.get_model(name)
        .with_context(|| format!("model '{name}' not found"))?.clone();
    Ok(OpenAIClient::new(cfg))
}
```

`/btw`、`/learn` 的提问/提炼通常是概念性或结构化任务，轻量模型即可，成本低、响应快。

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
       (model: learn_model，经 build_client_for("learn")；system: "Extract a durable rule. Output JSON.")
  ↓
后端: 写入 core_memory（SQLite，经 memory.lock().core_mut().create/append，自动持久化）
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
    let run_manager = state.run_manager.clone();
    let brain = run_manager.lock().await.brain().clone();
    let _ = session_id; // 当前不依赖 session（core_memory 跨 session 生效）

    tokio::spawn(async move {
        // 1. 轻量模型提炼 —— build_client_for("learn")，chat_completion(&[Message], &[])。
        let client = match brain.build_client_for("learn") {
            Ok(c) => c,
            Err(e) => {
                let _ = app_handle.emit("learn-event", LearnEvent {
                    event_type: "error", error: e.to_string(),
                });
                return;
            }
        };
        let msgs = vec![Message::system(LEARN_SYSTEM_PROMPT), Message::user(content.clone())];
        let extraction = match client.chat_completion(&msgs, &[]).await {
            Ok((text, _)) => text,
            Err(e) => {
                let _ = app_handle.emit("learn-event", LearnEvent {
                    event_type: "error", error: e.to_string(),
                });
                return;
            }
        };

        // MemoryEntry 不含 category/tags —— 实际 MemoryBlock 只有 {id,label,content,max_chars,updated_at}。
        let entry: MemoryEntry = serde_json::from_str(&extraction).unwrap_or(MemoryEntry {
            title: content.chars().take(60).collect(),
            rule: content,
        });

        // 2. 写入 core_memory（SQLite，自动持久化，无需 save_to_file）。
        //    经 brain.memory（Option<Arc<Mutex<MemoryManager>>>）-> core_mut()。
        //    create(id, label, content) 新建 block；或 append 到已有 "knowledge" block。
        let saved = if let Some(ref mem) = brain.memory {
            let mut mgr = mem.lock(); // parking_lot::Mutex
            let block_id = format!("learn_{}", uuid::Uuid::new_v4().simple());
            mgr.core_mut().create(&block_id, &entry.title, &entry.rule).is_ok()
        } else {
            false
        };

        // 3. 通知前端。
        let _ = app_handle.emit("learn-event", LearnEvent {
            event_type: if saved { "saved" } else { "error" },
            title: entry.title,
            rule: entry.rule,
            error: if saved { None } else { Some("memory disabled or write failed".into()) },
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
  "rule": "the actionable rule, written as an instruction"
}

（`category`/`tags` 需扩展 `MemoryBlock` 结构方可支持，列为后续工作；首版仅提炼 title + rule。）

User input: {content}
```

**Learn 模型配置**:

复用 §1.5 的 `learn_model` 配置项（`Config.learn_model`，缺省回退 `default_model`），经 `brain.build_client_for("learn")` 构建 client。不另设 `[learn]` section。

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

`core_memory` 的 block 在 `refresh_context_segments`（每轮）经 `memory_manager.core().to_context_string()` 注入 ACTIVE MEMORY 段（segment 5，`PerTurn` 刷新）。`/learn` 写入的新 block 在下一次 turn 刷新时自动注入 system prompt；且 SQLite 持久化，跨 session 生效，无需额外处理。

#### 2.5 去重

不在 `/learn` 时做即时去重。已有 `MemoryConsolidator`（每 20 turns 运行）会异步进行记忆合并和去重。用户可以放心多次 `/learn`，冗余条目会被 Consolidator 清理（`MemoryConsolidator` 见 `memory/consolidation.rs`，由 Run 每 20 turns 触发，见 `run.rs`）。

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
       2. self.emit(RunEvent::GoalSet)（方法名是 emit）
       3. 将 goal 注入 per-turn 刷新的 context 段（避免被 compaction 压缩）
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

**Goal 注入**（`run.rs` `run()` method）——注入到 per-turn 刷新段，**不**用一次性 system note（会被 compaction 压缩）：

```rust
// 在添加 user message 之前，解析 /goal 前缀
if message.starts_with("/goal ") {
    let g = message.strip_prefix("/goal ").unwrap().trim().to_string();
    self.goal = Some(g.clone());

    // 注入到 per-turn 刷新的 context 段（复用 EXECUTION PLAN 段 segment 7，PerTurn；
    // 或新增 Context::set_goal()）。每轮重新拼入，避免被 compaction 压缩丢失。
    let goal_prompt = format!(
        "## PRIMARY GOAL (pinned)\n\
        {}\n\n\
        You MUST keep this goal in mind throughout the conversation. \
        Break it into subtasks, track progress, and proactively drive toward completion. \
        If the conversation drifts, remind the user of this goal.",
        g
    );
    self.context.set_goal(&goal_prompt); // 新增 per-turn setter（见下）

    self.emit(RunEvent::GoalSet { goal: g.clone() }); // 方法名是 emit（非 emit_event）

    let todos = self.decompose_goal(&g).await?;
    self.emit(RunEvent::TodoUpdated { items: todos });
}
```

**Context 段刷新**（`context.rs` / `run.rs` `refresh_context_segments`）：新增薄 setter，goal 文本每轮重新拼入 EXECUTION PLAN 段：

```rust
// context.rs
pub fn set_goal(&mut self, text: &str) { self.goal_text = Some(text.to_string()); }
// refresh_context_segments 中：execution_plan = goal_text(if any) + todo list
```

**目标拆解**（`run.rs`）——复用 Run 的 `self.client`，`chat_completion(&[Message], &[])`：

```rust
async fn decompose_goal(&self, goal: &str) -> Result<Vec<TodoItemPayload>, RunError> {
    let msgs = vec![
        Message::system(GOAL_DECOMPOSE_SYSTEM),
        Message::user(format!(
            "Break down this goal into concrete, actionable subtasks (3-8 items).\n\
            Output JSON array: [{{\"description\": \"...\", \"status\": \"pending\"}}]\n\n\
            Goal: {goal}"
        )),
    ];
    let (response, _) = self.client.chat_completion(&msgs, &[]).await?;
    let items: Vec<TodoItemPayload> = serde_json::from_str(&response)?;
    // 写入共享 TodoList（brain.todo_list: Arc<Mutex<TodoList>>）并返回 payload
    Ok(items)
}
```

**每轮自省**（`run_loop` 中 LLM 回复后）——偏离提醒同样经 per-turn 段注入，而非一次性 note：

```rust
if let Some(ref goal) = self.goal {
    if !self.goal_completed {
        let progress = self.assess_goal_progress(goal).await?; // 内部用 chat_completion
        if progress.all_completed {
            self.goal_completed = true;
            self.emit(RunEvent::GoalCompleted { goal: goal.clone() });
        }
        // 偏离提醒：由 refresh_context_segments 把 goal 重新注入 EXECUTION PLAN 段，
        // 而非 self.context.add(Message::system(...))（一次性 note 易被压缩）。
    }
}
```

**Todo 作用域注意**：`TodoList` 是 Brain 共享的 `Arc<Mutex<TodoList>>`（跨 Run）。`/goal` 拆解的 todo 写入共享列表并 emit `TodoUpdated`。首版接受共享语义（与现有 todo 工具一致）；若需 per-Run 隔离，需将 todo 下沉到 Run（见决策 11）。

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

**前端类型同步**（`types.ts` / `eventHandlers.ts`）：`RunEventType` 追加 `'goal_set' | 'goal_completed'`；`RunEventPayload` 追加 `goal?: string`；`processSingleEvent` 增加 `case 'goal_set'` / `'goal_completed'`。`goal` / `goalCompleted` 需 per-session 隔离（`goalBySession`，类比 `todoBySession`）。

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

为 `AutocompleteItem`（`useAutocomplete.ts`）新增可选 `description` 字段，并在 `ChatInput.tsx` 的 dropdown 渲染中显示命令说明。

#### 4.2 输入框命令高亮

在 `highlightedHTML` 中，对 `/btw`、`/learn`、`/goal` 命令前缀添加特殊高亮（类似 `@mention` 的 token 样式），让用户明确知道输入的是命令而非普通文本。

---

### 5. 事件类型总览

| 事件 | 传输方式 | 持久化 | 写入主上下文 | 用途 |
|------|----------|--------|-------------|------|
| `btw-event` (delta/done/error) | `app_handle.emit` 独立通道 | 否 | 否 | `/btw` 流式回答（与主 run 并行） |
| `learn-event` (saved/error) | `app_handle.emit` 独立通道 | 否（记忆本身持久化到 SQLite core_memory） | 否 | `/learn` 完成通知 |
| `GoalSet` | Run broadcast | 是 (JSONL) | 是（goal 注入 system） | `/goal` 目标设定 |
| `GoalCompleted` | Run broadcast | 是 (JSONL) | 否 | `/goal` 目标完成 |
| `TodoUpdated` | Run broadcast | 是 (JSONL) | 否 | `/goal` 任务进度更新（复用现有事件） |

> 独立通道（`btw-event`/`learn-event`）不进 JSONL、无 seq；前端需新增第二个 `listen()`，现有 `useAgentEventListener` 仅监听 `agent-event`。

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
| 7 | `/btw` context 快照获取路径 | **RunManager → RunHandle 共享快照** | Brain 不持有 context；新增 `RunManager::context_snapshot_for_session()` + `RunHandle::context_snapshot()`（`Arc<RwLock<Vec<Message>>>`，Run 在 turn 边界刷新） |
| 8 | `/btw`/`/learn` 模型配置 | **复用 provider/model 架构** | 新增 `Config.btw_model`/`learn_model`（模型名，缺省回退 `default_model`）+ `Brain::build_client_for(purpose)`；不引入裸 `[btw]`/`[learn]` section |
| 9 | `/goal` 注入方式 | **per-turn 刷新段** | 一次性 system note（`add`）会被 compaction 压缩；goal 经 `set_goal()` 每轮重新拼入 EXECUTION PLAN 段 |
| 10 | `/learn` 写入 API | **CoreMemory::create/append** | `MemoryBlock { id,label,content,max_chars,updated_at }` 无 `category`/`tags`；SQLite 自动持久化，无 `save_to_file`；`category`/`tags` 列为后续扩展 |
| 11 | `/goal` todo 作用域 | **首版共享 TodoList** | `TodoList` 为 Brain 共享 `Arc<Mutex<TodoList>>`；首版与现有 todo 工具一致，接受跨 Run 共享；per-Run 隔离为后续工作 |

---

### API Changes

#### 新增 Tauri 命令（Non-breaking）

| 命令 | 参数 | 返回 | 可在 isProcessing 时调用 | 说明 |
|------|------|------|:---:|------|
| `btw_query` | `{ sessionId, question }` | `btw_id: String` | ✅ | 旁路问答：`run_manager.brain()` 取 `Arc<Brain>` → `build_client_for("btw")` 建 client → `context_snapshot_for_session()` 取快照 → `chat_completion_stream` 流式 → `btw-event` 通道回传 |
| `learn_memory` | `{ sessionId?, content }` | `()` | ✅ | 提炼记忆：`build_client_for("learn")` + `chat_completion` → `brain.memory.lock().core_mut().create/append` 写入 SQLite |

> `AppState` 不直接持有 `brain`；经 `state.run_manager.lock().await.brain()` 访问（返回 `&Arc<Brain>`，可 clone）。

#### 新增 RunManager / RunHandle / Brain / Context / Config（Non-breaking）

```rust
// manager.rs —— 查找 session 的活跃 Run 并取 context 快照
impl RunManager {
    pub async fn context_snapshot_for_session(&self, session_id: &str) -> Option<Vec<Message>>;
}
impl RunHandle {
    // 共享 context 快照（Arc<RwLock<Vec<Message>>>，由 Run 在 turn 边界刷新）
    pub fn context_snapshot(&self) -> Vec<Message>;
}

// brain.rs —— 按用途构建 client（btw/learn 用配置的轻量模型）
impl Brain {
    pub fn build_client_for(&self, purpose: &str) -> Result<OpenAIClient>;
}

// config.rs —— 可选轻量模型名（缺省回退 default_model）
pub struct Config {
    pub btw_model: Option<String>,
    pub learn_model: Option<String>,
}

// context.rs —— per-turn 刷新的 goal 段（避免被 compaction 压缩）
impl Context {
    pub fn set_goal(&mut self, text: &str);
}
```

#### 新增 RunEvent（Non-breaking）

```rust
// event.rs 新增（serde tag=snake_case → goal_set / goal_completed）
GoalSet { goal: String },
GoalCompleted { goal: String },
```

#### 现有 `send_message` 无变更

`/goal` 复用现有 `send_message` 通道，后端在 `Run::run()` 中解析 `/goal` 前缀。不修改 `send_message` 签名。

#### 前端 Redux 新增（均需 per-session 隔离）

- `ChatState.btwEntries: BtwEntry[]` + `btwEntriesBySession`
- `ChatState.learnEntries: LearnEntry[]` + `learnEntriesBySession`
- `ChatState.goal: string | null` + `goalBySession`；`ChatState.goalCompleted: boolean` + `goalCompletedBySession`
- `RunEventType` 追加 `'goal_set' | 'goal_completed'`；`RunEventPayload` 追加 `goal?: string`
- 新增第二个 `listen("btw-event")` / `listen("learn-event")`（现有 `useAgentEventListener` 仅监听 `agent-event`）
- `AutocompleteItem`（`useAutocomplete.ts`）新增可选 `description` 字段

### Backwards Compatibility

- 现有 `/subagents`、`/clear`、`/help` 行为不变
- 现有 `send_message` 签名不变
- 新增的事件类型是 enum 变体追加，不影响旧前端解析（`default` 分支忽略未知事件）
- `core_memory` 写入经 `create`/`append`（新增独立 block），不破坏现有 block
- 前端新 Redux 字段有默认值，旧 session resume 不受影响
- `btw_model`/`learn_model` 为可选配置项，缺失时回退到 `default_model`（不引入裸 `[btw]`/`[learn]` section）

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
| 2026-07-03 | agent_core | 代码审查修订（v2）：对齐实际 API——/btw 经 RunManager 取 context 快照 + `build_client_for` + `chat_completion_stream`；/learn 经 `core_mut().create/append` 写 SQLite（无 category/tags/save_to_file）；/goal 注入 per-turn 段（非 `add_system_note`）；模型配置改用 `btw_model`/`learn_model`；修正 `emit`/`AppState` 访问路径 |

---
*Generated by AI Agent (agent_core)*
*Model: glm-latest | Timestamp: 2026-07-03T00:00:00+08:00*
