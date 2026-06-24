# 2026-06-24 前端 React 代码审查

## 审查概述

审查范围：`app/src/` 下全部前端 React 代码，共 33 个 TypeScript/TSX 文件、约 5900 行代码。

技术栈：React 19 + TypeScript 5.8 + Vite 7 + Redux Toolkit + Tauri 2 IPC + marked/DOMPurify + lucide-react/react-icons。

总体评价：**架构设计有深度，rAF 批量分发和 Gap 自愈机制是亮点；但存在多个状态管理 Bug、性能热路径无 memoize、零测试覆盖等问题，需要按优先级逐步修复。**

---

## 架构亮点

### 1. rAF 批量事件分发 (`useAgentEventListener.ts`)

流式输出时后端每秒可发几十个 `MessageUpdate` 事件。监听器用 `requestAnimationFrame` 将一个帧内到达的所有事件攒批，在帧末统一 `dispatch`，将重渲染频率从"每 token 一次"降到了 ~60/s。卸载时还会 drain 缓冲区，确保不丢事件。

### 2. Gap 检测自愈 (`chatSlice.ts:843-859`)

每个 `RunEvent` 携带 per-Run 单调递增的 `seq`。Reducer 发现 `seq` 跳跃时，通过 `window.dispatchEvent(new CustomEvent('agent-event-gap'))` 通知监听层（不直接在 reducer 里 dispatch thunk），监听层收到后调用 `replay_since` 从后端 JSONL 日志补全丢失的事件。

### 3. 流式 Markdown 快速路径 (`MarkdownContent.tsx`)

流式期间跳过 markdown 解析，渲染纯文本（`whiteSpace: pre-wrap`）；`isStreaming=false` 后才执行 `marked.parse` + `DOMPurify.sanitize` 并 `useMemo` 缓存。超长流式文本（>4000 字符）回退为 markdown 渲染，避免长输出格式太差。彻底解决了 2026-06-20 审查中报告的 O(n²) 问题。

### 4. 会话缓存切换 (`chatSlice.ts:782-802`)

`entriesBySession` / `processingBySession` / `subagentsBySession` 三个字典缓存切换前的会话状态。切回已访问过的会话时直接从缓存恢复，无需等后端 `resume_session`，实现瞬时切换。

### 5. IME 组合输入处理 (`ChatInput.tsx:99-136`)

`isComposingRef` 跟踪 IME 状态，`compositionend` 延迟 50ms 清除（兼容 Safari/Chrome 在 macOS 上 `compositionend` 先于 `keydown` 触发的时序问题），确保中文/日文输入时 Enter 确认候选词不会误发消息。

### 6. Block 级渲染缓存 + WeakMap selector

- `EntryRow` 用 `memo` + ID-based `useSelector`（`selectEntryById`），每个 entry 独立订阅，互不干扰。
- `selectEntryById` 用 `WeakMap<ChatEntry[], Record<string, ChatEntry>>` 缓存 entries 数组到 ID 映射的字典，避免每次查找 O(n) 遍历。
- `groupBlocksIntoItems` 在 `AgentTurnUI` 中用 `useMemo` 缓存，只在 `entry.blocks` 变化时重新分组。

---

## P0 — Bug 与安全

### P0-1. Subagent `message_end` 事件被忽略，streaming block 可能永不关闭

`chatSlice.ts:916`:

```tsx
case 'message_end':
  if (!ev.subagent_id) handleMessageEnd(state, ev.message_id);
  break;
```

subagent 的 `message_end` 事件被**完全跳过**。subagent 的 streaming block 只在 `subagent_ended` 事件（`handleSubagentEnd`）中被关闭。如果 subagent 异常退出、后端只发了 `message_end` 没发 `subagent_ended`，对应的 thinking block 的 `isStreaming` 永远为 `true`，前端表现为卡在 "Thinking..." 状态。

**修法**：移除 `if (!ev.subagent_id)` 守卫，让 subagent 的 `message_end` 也走 `handleSubagentEnd`（需新增一个轻量版只关闭 streaming block），或在 `handleSubagentMessageEnd` 中关闭对应 subagent 的 streaming blocks。

### P0-2. ChatInput highlight overlay `dangerouslySetInnerHTML` 未经 DOMPurify

`ChatInput.tsx:198`:

```tsx
<pre
  ref={overlayRef}
  className="highlight-overlay"
  aria-hidden="true"
  dangerouslySetInnerHTML={{ __html: highlightedHTML + '<br />' }}
/>
```

`highlightedHTML` 由 `useAutocomplete.ts:223-240` 生成，虽然对 mention token 做了 `&` `<` `>` 转义，但：
1. 未转义 `"` 和 `'`（当前上下文是文本节点内，暂时安全，但后续扩展到属性值时有风险）。
2. 转义逻辑是手写的，容易遗漏边界情况。
3. 与 `MarkdownContent.tsx` 使用 DOMPurify 的安全基线不一致。

**修法**：对 `highlightedHTML` 也加一层 `DOMPurify.sanitize`，或改为 React 组件渲染（`parseMentions` 返回 token 数组，用 `<span>` 组件渲染 mention 高亮），彻底移除 `dangerouslySetInnerHTML`。

### P0-3. `retryFromEntry` 不清除旧 turn entries，对话历史重复

`chatSlice.ts:990-1002`:

```tsx
retryFromEntry: (state, action) => {
  const idx = state.entries.findIndex((e) => e.id === entryId);
  if (idx === -1) return;
  state.entries.push({
    id: `user-${Date.now()}`,
    type: 'user',
    text: userText,
  });
  state.isProcessing = true;
}
```

重试只是在末尾追加新的 user message，不清除原消息之后的 turn entries。用户编辑并重试第 3 条消息后，对话变成：`[user1, turn1, user2(edited), turn2(旧), user2(new), turn2(新)]`，历史混乱。

**修法**：`state.entries.truncate(idx + 1)` 截断到被重试的 entry 之后，再 push 新消息。

### P0-4. DeepSeek `<think>` 标签跨 chunk 解析失败

`chatSlice.ts:253-268`:

```tsx
while (textChunk.includes('<think>') || textChunk.includes('</think>')) {
  const thinkStartIdx = textChunk.indexOf('<think>');
  // ...
}
```

如果 `<think>` 被分割在两个 `message_update` 事件之间（第一个 chunk 是 `"He<thin"`，第二个是 `"k>hello"`），`includes('<think>')` 匹配失败，内容被当作普通 assistant 文本处理。虽然这种切割概率不高，但在长文本中会发生。

**修法**：维护一个跨 chunk 的 buffer，检测到不完整的 `<think` 前缀时暂存到 buffer，等下一个 chunk 拼接后再解析。或在后端 `TokenAccumulator` 层面保证不切割标签（更优）。

---

## P1 — 性能

### P1-1. `useTokenCount` 每次状态变更全量遍历

`useTokenCount.ts:5-16`:

```tsx
return useAppSelector((state) => {
  return state.chat.entries.reduce((sum, e) => {
    if (e.type === 'user' && e.text) return sum + roughTokenCount(e.text);
    if (e.type === 'turn' && e.blocks)
      return sum + e.blocks.reduce((s, b) => { /* ... */ }, 0);
    return sum;
  }, 0);
});
```

每次 Redux 状态变更（包括 60/s 的流式 token 更新）都遍历**所有 entries 和所有 blocks** 重新计算 token 数。长对话中这是显著的性能浪费。

**修法**：用 `createSelector` 做 memoized selector，依赖 `entries` 引用。Immer 保证未修改的 entries 引用不变，只有新消息追加时才重新计算。

### P1-2. `selectPendingApprovalCount` 无 memoize

`chatSlice.ts:1180-1197`:

```tsx
export function selectPendingApprovalCount(state: { chat: ChatState }): number {
  let count = 0;
  for (const entry of state.chat.entries) {
    // 遍历所有 entries 的所有 blocks
  }
  for (const sa of Object.values(state.chat.subagents)) {
    // 遍历所有 subagents 的所有 blocks
  }
  return count;
}
```

同样每次状态变更全量遍历。在 `App.tsx:105` 中通过 `useSelector(selectPendingApprovalCount)` 订阅，触发 App 级重渲染。

**修法**：`createSelector` memoize，或降级为只在 `approval_required` / `approval_resolved` 事件时更新的独立计数器。

### P1-3. `highlightedHTML` 无 `useMemo`

`useAutocomplete.ts:223-240`:

```tsx
const highlightedHTML = (() => {
  const tokens = parseMentions(input);
  return tokens.map((t) => { /* ... */ }).join('');
})();
```

IIFE 每次组件渲染都重新执行。虽然 `input` 通常较短，但 `ChatInput` 是 `memo` 组件，任何导致重渲染的 prop 变化都会触发这段计算。

**修法**：`useMemo(() => { ... }, [input])`。

### P1-4. `MutationObserver` 观察范围过大

`useAutoScroll.ts:62-73`:

```tsx
observer.observe(el, { childList: true, subtree: true, characterData: true });
```

`subtree: true` + `characterData: true` 监听整个聊天历史中每个文本节点的变更。流式输出时每个 token 都触发 MutationObserver 回调，然后检查 `isNearBottom` 并可能设置 `scrollTop`。

**修法**：缩小为 `childList: true` only（只监听子元素增删，不监听文本变更），或对回调做 `debounce(100ms)`。

### P1-5. `AgentTurn.tsx` 大量内联 style 对象破坏 `memo`

`AgentTurn.tsx` 全文有 20+ 处内联 `style` 对象：

```tsx
<div style={{ marginLeft: '6px', paddingLeft: '12px', borderLeft: '1px solid var(--text-muted)', display: 'flex', flexDirection: 'column', gap: '8px', marginTop: '6px', paddingBottom: '4px' }}>
```

每次渲染创建新对象 → `React.memo` 的浅比较判定 props 变化 → 子组件无谓重渲染。

**修法**：提取为模块级 `const` 常量（如 `const iterationBodyStyle = { ... }`），或移到 CSS class 中。

---

## P2 — 架构与代码质量

### P2-1. Reducer 内 `Promise.resolve().then()` 副作用

`chatSlice.ts:852-856`:

```tsx
// 在 agentEventReceived reducer 内部
Promise.resolve().then(() => {
  window.dispatchEvent(new CustomEvent('agent-event-gap', {
    detail: { runId: ev.run_id, fromSeq: prev },
  }));
});
```

Redux reducer 必须是纯函数。虽然 `Promise.resolve().then()` 延迟到 reducer 返回后执行（不影响当前状态计算），但这是**反模式**——Redux Toolkit 的 `listenerMiddleware` 或自定义 middleware 才是正确的副作用出口。

**修法**：在 `store.ts` 中添加 `listenerMiddleware`，监听 `agentEventReceived` action，在 middleware 中检测 gap 并 dispatch `resyncRun`。移除 `window.dispatchEvent` 桥接。

### P2-2. `switchModel` thunk save/switch 不一致风险

`settingsSlice.ts:181-202`:

```tsx
dispatch(setDefaultModel(modelKey));
try {
  await invoke('save_config', { config: newConfig });
} catch (e) {
  dispatch(setDefaultModel(currentConfig.default_model)); // 回滚 UI
  return rejectWithValue(String(e));
}
try {
  await invoke('switch_model', { name: modelKey });
} catch (e) {
  return rejectWithValue(String(e)); // ← 配置已改，但运行时模型没切
}
```

如果 `save_config` 成功但 `switch_model` 失败，配置文件中的 `default_model` 已改为新模型，但运行时仍在用旧模型。下次重启后会加载新模型——存在"配置与运行时不一致"窗口。

**修法**：先 `switch_model` 成功后再 `save_config`，或 `switch_model` 失败时也回滚配置。

### P2-3. 保存逻辑重复 (Sidebar vs useAutoSaveSession)

`Sidebar.tsx:191-210` 中的 `saveAndCacheCurrent` 和 `useAutoSaveSession.ts:39-57` 中的保存逻辑**几乎完全相同**：

```tsx
// 两处都是：
const msgs = entriesToMessages(chatState.entries);
const { eventLog, processTimeMs, thoughtTimeMs } = entriesToEventLog(chatState.entries, chatState.subagents);
dispatch(saveSessionMessages({ sessionId, messages: msgs, cwd, modelUsed, ... }));
```

维护时容易只改一处遗漏另一处。

**修法**：提取为 `useSaveSession()` 自定义 hook 或 `saveCurrentSession` thunk，两处共用。

### P2-4. `AgentTurn.tsx` 947 行需拆分

单个文件包含 10+ 组件：`ProcessingTimer`、`TurnIterationUI`、`ToolBlockUI`、`ApprovalBlockUI`、`EditFileWidget`、`SubagentSpawnWidget`、`SubagentCard`、`TurnFooter`、`AgentTurnUI`，加上 diff 解析逻辑（`parseUnifiedDiff`、`parseEditSummary`）。

**修法**：拆分为 `AgentTurn/` 目录：
```
AgentTurn/
├── index.tsx              ← AgentTurnUI (re-export)
├── TurnIterationUI.tsx
├── ToolBlockUI.tsx
├── ApprovalBlockUI.tsx
├── EditFileWidget.tsx
├── SubagentWidgets.tsx    ← SubagentSpawnWidget + SubagentCard
├── TurnFooter.tsx
├── ProcessingTimer.tsx
└── utils.ts               ← parseUnifiedDiff, parseEditSummary, getToolIcon
```

### P2-5. `as any` 类型断言绕过类型检查

`AgentTurn.tsx:245-247`:

```tsx
const approvalBlock = iteration.toolBlocks.find(
  (tb) => tb.type === 'approval' && (tb as any).tool_name === name && (tb as any).status !== 'pending'
);
const approvalStatus = approvalBlock ? (approvalBlock as any).status : undefined;
```

已经 `tb.type === 'approval'` 了，应该用类型守卫安全地收窄类型。

**修法**：
```tsx
const approvalBlock = iteration.toolBlocks.find(
  (tb): tb is Extract<TurnBlock, { type: 'approval' }> =>
    tb.type === 'approval' && tb.tool_name === name && tb.status !== 'pending'
);
const approvalStatus = approvalBlock?.status;
```

### P2-6. `confirm()` / `prompt()` / `alert()` 阻塞 UI

`Sidebar.tsx:177,184,237`、`useGitBranch.ts:53`:

```tsx
if (confirm('Delete this project and all its sessions?')) { ... }
const name = prompt('New name:', newName);
window.alert(String(e));
```

原生浏览器对话框在 Tauri WebView 中行为不一致（macOS 上可能不显示标题），且阻塞 UI 线程。与项目的自定义模态框（`SettingsModal`）风格不统一。

**修法**：复用已有的模态框基础设施，实现 `ConfirmDialog` / `PromptDialog` 组件。

---

## P3 — 死代码与冗余

### P3-1. 7 个无 `onClick` 的死按钮

`App.tsx:325-329`:

```tsx
<button className="icon-btn"><BoxIcon size={14} /></button>
<button className="icon-btn"><MessageSquareIcon size={14} /></button>
<button className="icon-btn"><TerminalSquareIcon size={14} /></button>
<button className="icon-btn"><FolderIcon size={14} /></button>
<button className="icon-btn"><Maximize2Icon size={14} /></button>
```

`Sidebar.tsx:271-272`:

```tsx
<div className="nav-item"><PlusIcon size={14} /> New Agent</div>
<div className="nav-item"><MessageSquareIcon size={14} /> New requirement</div>
```

这些按钮/导航项没有点击处理器，是占位 UI。要么实现功能，要么移除或加 `disabled` 样式 + `title="Coming soon"`。

### P3-2. `ModelSelector` 的 `onModelChange` prop 从未传入

`ModelSelector.tsx:21` 定义了 `onModelChange?: (key: string) => void`，但 `ChatInput.tsx:218` 中渲染时未传入：

```tsx
<ModelSelector currentModel={currentModel} />
```

`ModelSelector` 内部直接 `dispatch(switchModel(...))`，绕过了父组件。这是关注点混乱——组件应该通过回调通知父组件，由父组件决定副作用。

**修法**：要么移除 `onModelChange` prop（承认组件自包含 dispatch），要么让 `ChatInput` / `App` 传入回调统一管理。

### P3-3. `errorCount` 计算但未使用

`AgentTurn.tsx:843-853`:

```tsx
const { toolCount, thoughtCount } = useMemo(() => {
  let tools = 0, thoughts = 0, errors = 0;
  // ... errors 被计算
  return { toolCount: tools, thoughtCount: thoughts, errorCount: errors };
}, [entry.blocks]);
```

`errors` 被计算并返回为 `errorCount`，但解构只取了 `toolCount` 和 `thoughtCount`。TypeScript `noUnusedLocals` 应该报这个（如果开了的话）。

### P3-4. `AgentRow.tsx` 纯透传

```tsx
export const AgentRow = memo(function AgentRow({ entry }: { entry: ChatEntry }) {
  return (
    <div className="message-row agent-row">
      <AgentTurnUI entry={entry} />
    </div>
  );
});
```

11 行只加了一个 `<div>` wrapper。可以直接在 `EntryRow` 中渲染 `<AgentTurnUI>` + className，移除这个中间层。

---

## 评分

| 维度 | 评价 |
|---|---|
| 架构设计 | 良好，rAF 批量分发 + Gap 自愈 + 会话缓存是亮点 |
| 状态管理 | 中等，Reducer 内副作用 + 保存逻辑重复 + retryFromEntry bug |
| 性能优化 | 中等偏下，热路径 selector 无 memoize + MutationObserver 范围过大 |
| 类型安全 | 中等，多处 `as any` + `as string[]` 强转 |
| 安全 | 中等，MarkdownContent 有 DOMPurify 但 ChatInput highlight 无防护 |
| 测试覆盖 | **零**，无 Vitest / Testing Library，无任何前端测试 |
| 代码组织 | 中等，AgentTurn.tsx 947 行需拆分，死按钮和未使用 prop 需清理 |

---

> [!NOTE]
> **优先修复顺序建议**
> 1. P0-1: subagent `message_end` 事件忽略 (1 行代码修复)
> 2. P0-2: ChatInput highlight 加 DOMPurify (1 行)
> 3. P0-3: `retryFromEntry` 截断旧 entries (1 行)
> 4. P1-1 + P1-2: `useTokenCount` + `selectPendingApprovalCount` 改 memoized selector
> 5. P2-1: Reducer 副作用移到 `listenerMiddleware`
> 6. P2-4: 拆分 `AgentTurn.tsx`
> 7. 添加前端测试基础设施 (Vitest + Testing Library)
