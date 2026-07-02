# RFC-0002: Steering — Human-in-the-Loop Mid-Run Message Injection

```yaml
---
id: RFC-0002
type: RFC
title: Steering — Human-in-the-Loop Mid-Run Message Injection
status: Draft
author: AI Agent (agent_core)
created: 2026-07-02
updated: 2026-07-02
reviewers: [zniverse]
related: []
supersedes: ~
superseded_by: ~
tags: [steering, ux, human-in-the-loop, agent-loop]
---
```

## Summary

完善 Steering（中途引导）功能的 UX 闭环：当 Agent 正在运行时，用户可以发送"插嘴"消息，该消息以 **Pending** 状态排队等候，并在 Agent 当前原子操作（单个 turn）结束后立即注入上下文、重新规划。

## Motivation

### 当前痛点

后端基础设施已就绪：`steering_queue`（`VecDeque<Message>`）、`RunCommand::Steer`、`steer_run` Tauri 命令、ChatInput 中的 steer 按钮均已实现。但 **UX 闭环缺失**：

1. **用户无反馈**：发送 steer 消息后没有任何视觉提示（toast / badge / 队列指示器），用户不确定消息是否已被接收
2. **消息不可见**：steer 消息不会作为 UserRow 出现在聊天历史中——"凭空消失"
3. **无后端事件**：后端不发射 `SteerQueued` / `SteerInjected` 事件，前端无从得知 steering 状态变化
4. **Enter 键被阻塞**：`handleSend` 在 `isProcessing` 时返回 `return;`——用户只能点击那个 opacity: 0.7 的小按钮，发现率极低
5. **多消息队列不可管理**：排队了多条 steer 消息无法查看、删除、重新排序

### 为什么 Steerable 是必备？

- **纠偏成本高**：不允许中途引导时，Agent 可能在错误前提下写出数百行代码或跑挂环境
- **动态上下文**：用户常在看到 Agent 第一步搭出的骨架后才想起补充要求（如"数据库加 SSL"）
- **Human-in-the-Loop 标配**：这是当前 Coding Agent 的行业标准

## Detailed Design

### Goals

- G1: steer 消息在聊天历史中可见（显示为特殊样式的 UserRow），附带 pending / injected 状态标记
- G2: 后端发射 `SteerQueued` 和 `SteerInjected` 事件，前端据此更新消息状态
- G3: 在 ChatInput 区域显示 `Pending N message(s)` 徽标，让用户清楚知道队列中有多少消息
- G4: `Enter` 键在 `isProcessing` 期间自动走 steer 路径，而非被阻塞
- G5: 支持查看和取消已排队的 steer 消息

### Non-Goals

- N1: 不改变 steer 消息注入的时序语义（仍然在 turn boundary 注入）
- N2: 不实现 steer 消息的优先级排序或重新排序功能
- N3: 不引入新的 RunState（steering 是一个队列动作，不改变 Run 状态机）
- N4: 不在本 RFC 中实现"暂停 → 编辑 → 恢复"这种更复杂的编辑式引导

### Proposed Changes

---

#### Phase 1: Backend Events（后端事件补全）

##### [MODIFY] [event.rs](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/event.rs)

新增两个 `RunEvent` 变体：

```rust
/// A steering message was queued (not yet injected into context).
SteerQueued {
    steer_id: String,
    message: String,
    queue_depth: usize,  // 队列中总共有几条
},

/// A steering message was injected into the agent's context.
SteerInjected {
    steer_id: String,
    message: String,
},

/// A steering message was cancelled (by user or because run ended).
SteerCancelled {
    steer_id: String,
    reason: String,
},

/// A steering message failed to inject (e.g. context limit).
SteerFailed {
    steer_id: String,
    error: String,
},
```

##### [MODIFY] [run.rs] & [agent/mod.rs]

1. **`poll_commands` 和 `wait_for_resume` 中的 `Steer` 分支**：使用前端传入的 `steer_id` 封装 `SteerEntry`，并发射 `RunEvent::SteerQueued`
2. **Turn boundary 注入行为变更**：
   - 将 `run.rs` (L638-643 和 L773-776) 中的 `while let Some` 改为 `if let Some`。
   - **行为变更理由**：确保一次只弹出一并注入一条 steer 消息。如果用户短时间排了 3 条 steer，这将用 3 个 turn 来处理。这避免了 LLM 一次收到过多指令导致混淆，同时与旧版 `agent/mod.rs` 中的行为保持一致。
   - 注入后在 `context.add` 后发射 `RunEvent::SteerInjected`。

修改后的 `poll_commands`（示例）：

```rust
RunCommand::Steer { steer_id, message } => {
    self.steering_queue.push_back(SteerEntry {
        id: steer_id.clone(),
        message: Message::user(&message),
        raw_text: message.clone(),
    });
    self.emit(RunEvent::SteerQueued {
        steer_id,
        message,
        queue_depth: self.steering_queue.len(),
    });
}
```

引入辅助结构：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SteerEntry {
    id: String,
    message: Message,
    raw_text: String,
    timestamp: u64,
}
```

**关键设计细节补充**：
- **`SteerEntry` 必须实现 `Serialize`/`Deserialize`**：以便支持状态持久化和跨进程通信。
- **Run 结束清理**：如果 Agent 完成运行（返回 `TurnOutcome::Stop` / `Final`）或被强制中止时，遍历 `steering_queue`，对所有剩余条目发射 `RunEvent::SteerCancelled { reason: "Run ended before message could be injected" }`。
- **LLM 提示前缀**：在注入上下文前，自动为 steer 消息内容加上前缀：`[USER STEER MID-RUN] 以下是用户中途注入的引导指令，请优先遵循：\n`。这能确保 LLM 意识到这是中途的调整指令，而非原始的补充。

将 `steering_queue` 类型从 `VecDeque<Message>` 改为 `VecDeque<SteerEntry>`，以便在注入时发射包含 `steer_id` 的事件。

##### [MODIFY] [command.rs](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/command.rs)

1. `RunCommand::Steer` 增加 `steer_id` 字段：
```rust
Steer { steer_id: String, message: String },
```

2. 新增取消 steer 的命令：

```rust
/// Cancel a pending steer message by its ID.
CancelSteer { steer_id: String },
```

**`CancelSteer` 后端处理逻辑**：
收到此命令后，后端在 `steering_queue` 中查找对应的 `steer_id`，如果找到则将其移除（`retain` 或 `remove`），并发射 `RunEvent::SteerCancelled { steer_id, reason: "Cancelled by user" }`。**竞态条件处理**：如果找不到（说明消息已经从 queue pop 出来，正在或已经注入上下文），则视为消息已注入不可撤销，静默忽略（no-op），不报错。

##### [MODIFY] [mod.rs (agent)](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/agent/mod.rs)

同步修改 `Agent.steering_queue` 类型和 `steer()` 方法签名以使用 `SteerEntry`。

---

#### Phase 2: Tauri Bridge（Tauri 命令扩展）

##### [MODIFY] [lib.rs](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/app/src-tauri/src/lib.rs)

1. `steer_run` 增加 `steer_id` 参数，由前端传入：
   ```rust
   #[tauri::command]
   async fn steer_run(state: State<'_, AppState>, run_id: String, steer_id: String, message: String) -> Result<(), String> {
       // ...
   }
   ```

2. 新增 `cancel_steer` 命令：
   ```rust
   #[tauri::command]
   async fn cancel_steer(state: State<'_, AppState>, run_id: String, steer_id: String) -> Result<(), String> {
       // ...
   }
   ```

---

#### Phase 3: Frontend State（前端状态管理）

##### [MODIFY] [types.ts](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/app/src/features/chat/types.ts)

新增类型：

```typescript
export interface SteerMessage {
  steerId: string;
  text: string;
  status: 'pending' | 'injected';
  timestamp: number;
}

// ChatState 中新增：
steerQueue: SteerMessage[];
steerQueueBySession: Record<string, SteerMessage[]>;

// RunEventType 中新增：
// 'steer_queued' | 'steer_injected'
```

##### [MODIFY] [chatSlice.ts](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/app/src/features/chat/chatSlice.ts)

1. `initialState` 增加 `steerQueue: []` 和 `steerQueueBySession: {}`
2. `cacheCurrentSession` 和 `restoreOrClearSession` 增加对 `steerQueue` 的缓存同步逻辑
3. **新增专门的 reducers（不复用 userMessageSent 以避免破坏 todo 和 isProcessing 状态）**：
   - `steerMessageQueued(state, action: PayloadAction<{ steerId: string; text: string }>)` — 添加 steer 消息到 `steerQueue`，同时向 `entries` push 一条条目（`type: 'user', isSteer: true, steerId, steerStatus: 'pending'`）。**不要**修改 `isProcessing` 或 `todo` 状态。
   - `steerMessageInjected(state, action: PayloadAction<string /*steerId*/>)` — 将对应条目的 `steerStatus` 从 `pending` 改为 `injected`
   - `steerMessageCancelled(state, action: PayloadAction<string /*steerId*/>)` — 移除排队的 entry
4. 导出新 actions

##### [MODIFY] [eventHandlers.ts](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/app/src/features/chat/eventHandlers.ts)

在 `processSingleEvent` 中处理新事件类型：

```typescript
case 'steer_queued': {
  state.steerQueue.push({
    steerId: payload.steer_id,
    text: payload.message,
    status: 'pending',
    timestamp: Date.now(),
  });
  break;
}
case 'steer_injected': {
  const idx = state.steerQueue.findIndex(s => s.steerId === payload.steer_id);
  if (idx !== -1) {
    state.steerQueue[idx].status = 'injected';
  }
  // 同步更新 entries 中对应的 steer 消息状态
  for (const entry of state.entries) {
    if (entry.type === 'user' && entry.isSteer && entry.steerId === payload.steer_id) {
      entry.steerStatus = 'injected';
    }
  }
  break;
}
case 'steer_cancelled':
case 'steer_failed': {
  // 从队列移除，更新 entry 状态，或显示系统提示消息
  state.steerQueue = state.steerQueue.filter(s => s.steerId !== payload.steer_id);
  // 可选：如果是 run end 导致的批量 cancel，在此处触发 UI toast 提示
  break;
}
```

---

#### Phase 4: UI Components（前端 UI）

##### [MODIFY] [ChatInput.tsx](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/app/src/components/chat/ChatInput.tsx)

1. **Enter 键 + isProcessing → 自动 steer**：

   ```typescript
   // 修改 handleKeyDown 中的 Enter 处理逻辑
   if (e.key === 'Enter' && !e.shiftKey) {
     e.preventDefault();
     if (isProcessing && onSteer && input.trim()) {
       onSteer(input.trim());
       setInput('');
     } else {
       handleSend();
     }
   }
   ```

2. **`Pending N message(s)` 徽标**：在 input-actions 区域，当 `pendingSteerCount > 0` 时显示。点击该徽标可以展开一个队列预览，方便用户快速查看所有 pending 的 steer 消息。

3. **Stop 按钮旁的 Send 按钮 opacity 从 0.7 提升**：改为使用 accent-color + pulse 动画，更醒目

4. **Placeholder 动态切换**：在 `isProcessing` 期间，将 `textarea` 的 placeholder 从空白修改为 `"Type to steer the agent..."`，并配合**输入框边框颜色微调（如改为橙色）**，与 SteerRow 的橙色主题一致，大幅提高 steer 功能的发现率。

5. **复用常规消息解析**：Steer 消息输入时，完全复用普通用户消息的解析逻辑（包括 `@file` 引用、`/command` 语法支持、以及附件处理等），保持与标准消息输入的能力一致性。相应的，`steer_run` 需传递所有必要的附件上下文。

##### [MODIFY] [App.tsx](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/app/src/App.tsx)

1. `handleSteer` 改为先 dispatch `steerMessageQueued`，再 `invoke('steer_run')`：

   ```typescript
    const handleSteer = useCallback(async (message: string) => {
      if (!runId || !message.trim()) return;
      const steerId = crypto.randomUUID();
      // 使用专用 reducer，避免 userMessageSent 清空 todo 或影响其他状态
      dispatch(steerMessageQueued({ steerId, text: message }));
      
      try {
        await invoke('steer_run', { runId, steerId, message });
     } catch (e) {
       console.error('Failed to steer run:', e);
       // 乐观更新失败，回滚状态
       dispatch(steerMessageCancelled(steerId));
     }
   }, [runId, dispatch]);
   ```

2. **传递 `pendingSteerCount`** 到 `ChatInput`
3. **增加取消失败的反馈**：如果在 UI 点击取消时，该消息恰好已经被注入（状态为 `injected`，但前端还没收到事件），弹出 toast 提示："该引导消息已被注入，无法取消"。

##### [NEW] [SteerRow.tsx](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/app/src/components/chat/SteerRow.tsx)

显示 steer 消息的组件，类似 `UserRow` 但带有：
- 左侧橙色竖线（区别于普通 user message 的蓝色）
- pending 状态：脉冲动画 + `⏳ Queued — will inject after current step` 标签
- injected 状态：`✅ Injected` 标签
- pending 时可取消（× 按钮）

##### [MODIFY] [EntryRow.tsx](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/app/src/components/chat/EntryRow.tsx)

路由 `type === 'user' && isSteer` 到 `SteerRow`（**重要**：不引入新 type，从而保证 `session` 保存和后端状态恢复 `resumeSession` 的完全兼容）。

##### [MODIFY] [input.css](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/app/src/styles/input.css)

新增 `.steer-pending-badge` 和 `.steer-send-btn` 样式

##### [MODIFY] [chat.css](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/app/src/styles/chat.css)


新增 `.steer-row` 样式，包括：
- 左侧橙色边框
- pending 状态下的脉冲/呼吸动画
- 紧凑布局

---

### API Changes

#### Backend → Frontend Events (新增)

| Event | Payload | 说明 |
|-------|---------|------|
| `steer_queued` | `{ steer_id, message, queue_depth }` | steer 消息入队 |
| `steer_injected` | `{ steer_id, message }` | steer 消息已注入 context |
| `steer_cancelled` | `{ steer_id, reason }` | steer 被用户或系统取消 |
| `steer_failed` | `{ steer_id, error }` | steer 注入失败 |

#### Tauri Commands (变更)

| Command | Before | After |
|---------|--------|-------|
| `steer_run` | `(run_id, message)` | `(run_id, steer_id, message)` |
| `cancel_steer` | — | **新增** `(run_id, steer_id) → Result<(), String>` |

#### Internal Types (变更)

| Type | Before | After |
|------|--------|-------|
| `Run.steering_queue` | `VecDeque<Message>` | `VecDeque<SteerEntry>` |
| `Agent.steering_queue` | `VecDeque<Message>` | `VecDeque<SteerEntry>` |

### Backwards Compatibility

- **Frontend → Backend 接口破坏性变更 (Breaking Change)**：`steer_run` 签名变更（增加了必填的 `steer_id` 参数）。必须确保前后端代码同步发布并匹配使用，否则前端调用 steer 时会遇到 Tauri IPC 参数数量错误。
- **状态恢复兼容性**：在 `chatSlice` 中特意**不**使用 `type: 'steer'`，而是复用 `type: 'user'` 并附加可选属性，这保证了原有的 `entriesToMessages` 和 `resumeSession` 逻辑完全不受影响，旧的 session 也能无缝兼容。
- **Backend events**：新增事件不影响旧客户端——旧 `processSingleEvent` 对 unknown event 默认跳过
- **`SteerEntry` 内含 `Message`**：对 `context.add()` 调用不变，只是外层包装增加了 `id` 字段

## Timeline

| Phase | Description | ETA |
|-------|-------------|-----|
| 1 | Backend Events — `SteerQueued` / `SteerInjected` + `SteerEntry` 类型 | Day 1 |
| 2 | Tauri Bridge — `steer_run` 返回值 + `cancel_steer` 命令 | Day 1 |
| 3 | Frontend State — chatSlice / eventHandlers / types 更新 | Day 2 |
| 4 | UI Components — SteerRow + ChatInput 改造 + CSS | Day 2-3 |

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                        Frontend (React)                          │
│                                                                  │
│  ┌──────────┐   Enter while     ┌───────────┐                   │
│  │ ChatInput│ ─ isProcessing ─► │ handleSteer│                   │
│  │ (textarea)│                   │  (App.tsx) │                   │
│  └──────────┘                   └─────┬─────┘                   │
│       │                               │                          │
│       │ dispatch                      │ 1. dispatch              │
│       │ steerMessageQueued            │    steerMessageQueued     │
│       ▼                              │ 2. invoke('steer_run')    │
│  ┌──────────┐                        ▼                          │
│  │ chatSlice│◄──── agent-event ── Tauri IPC ──────────┐         │
│  │  .steer  │     (steer_queued                       │         │
│  │  Queue[] │      steer_injected)                    │         │
│  └──────────┘                                         │         │
│       │                                               │         │
│       ▼                                               │         │
│  ┌──────────┐  ┌──────────────┐                      │         │
│  │ SteerRow │  │ Pending N    │                      │         │
│  │ (pending)│  │ badge        │                      │         │
│  └──────────┘  └──────────────┘                      │         │
└──────────────────────────────────────────────────────┘         │
                                                                  │
┌─────────────────────────────────────────────────────────────────┐
│                      Backend (Rust)                              │
│                                                                  │
│  steer_run ──► RunCommand::Steer ──► cmd_rx                     │
│                                        │                         │
│                         ┌──────────────┘                        │
│                         ▼                                        │
│  ┌─────────────────────────────────────┐                        │
│  │       Run::poll_commands()          │                        │
│  │                                     │                        │
│  │  1. push to steering_queue          │                        │
│  │  2. emit SteerQueued { steer_id }   │                        │
│  └─────────────────────────────────────┘                        │
│                         │                                        │
│            ┌────────────┘                                       │
│            ▼                                                     │
│  ┌─────────────────────────────────────┐                        │
│  │     Turn Boundary (TurnEnded)       │                        │
│  │                                     │                        │
│  │  1. pop from steering_queue         │                        │
│  │  2. context.add(steer_msg)          │                        │
│  │  3. emit SteerInjected { steer_id } │                        │
│  │  4. → Continue loop (re-plan)       │                        │
│  └─────────────────────────────────────┘                        │
└─────────────────────────────────────────────────────────────────┘
```

## Open Questions

（已全部在最终设计中解决）

## Future Directions (后续迭代方向)

1. 支持 steer 消息的重新排序（拖拽改变注入顺序）
2. 支持导出聊天记录时特别标记 steer 消息
3. 增加 steer 消息的长度限制，避免过长的 steer 消息导致上下文溢出
4. 提供一键"批量注入"的开关配置，允许高级用户一次性注入所有 pending 消息

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-07-02 | AI Agent (agent_core) | Created as Draft |

---
*Generated by AI Agent (agent_core)*
*Model: Claude Opus 4.6 | Timestamp: 2026-07-02T12:15:00+08:00*
