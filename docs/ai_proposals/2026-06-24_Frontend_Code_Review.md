# Frontend Code Review Report

> **Date:** 2026-06-24  
> **Author:** WorkBuddy (AI Agent)  
> **Scope:** `/Users/zniverse/Documents/projects/rust-projects/agent_core/app/`  
> **Tech Stack:** React 19 + TypeScript + Redux Toolkit + Tauri v2 API

---

## 1. 项目概述

agent_core 前端是一个基于 React 19 的桌面应用 UI，通过 Tauri v2 与 Rust 后端通信。它负责实时流式对话渲染、工具调用可视化、权限审批、子 Agent 嵌套展示、项目管理（Projects/Sessions）等核心功能。

**架构模式：** 经典 Redux feature-based slices + 容器组件（App.tsx）+ 展示组件（components/）

---

## 2. 🔴 严重问题 (Critical)

### 2.1 Reducer 中 dispatch side-effect — 违反 Redux 基本原则

**文件:** `features/chat/chatSlice.ts:638-648`

```typescript
// 在 agentEventReceived reducer 内部
Promise.resolve().then(() => {
  window.dispatchEvent(new CustomEvent('agent-event-gap', {
    detail: { runId: ev.run_id, fromSeq: prev },
  }));
});
```

**问题：** Redux reducers 必须是**纯函数**。这里在 reducer 内部创建 Promise 并触发 `window.dispatchEvent`。副作用导致：
- Redux DevTools 时间旅行调试时事件被重复触发
- 服务端渲染（如果未来支持）会 crash
- 不可预测的行为：如果 reducer 因异常被重试，gap detection 会被重复触发

**修复建议：** 将 gap detection 逻辑移到 `useAgentEventListener` hook 中：
```typescript
useEffect(() => {
  return listen('agent-event', (event) => {
    // 检测 gap，直接 dispatch resyncRun
  });
}, []);
```

---

### 2.2 `selectEntryById` 的 `WeakMap` 缓存完全无效

**文件:** `features/chat/chatSlice.ts:914-926`

```typescript
const entryMapCache = new WeakMap<ChatEntry[], Record<string, ChatEntry>>();

export function selectEntryById(state, entryId) {
  const entries = state.chat.entries;
  let map = entryMapCache.get(entries);  // ← 永远不会命中！
```

**问题：** Redux 的 immutable update 意味着每次 action 后 `state.chat.entries` 是**全新的数组引用**。`WeakMap` 以数组为 key，但 key 每次都是新的，缓存永远 miss。结果：
- 每次 `selectEntryById` 调用都创建新的 `map` 对象 → GC 压力
- `EntryRow` 组件在每次 re-render 时都执行 O(n) 查找

**修复建议：** 使用 `createSelector`（Reselect）：
```typescript
export const selectEntryById = createSelector(
  [(state: RootState) => state.chat.entries, (_, entryId: string) => entryId],
  (entries, entryId) => entries.find(e => e.id === entryId)
);
```

---

### 2.3 `switchModel` thunk 缺少完整 rollback

**文件:** `features/settings/settingsSlice.ts:95-115`

```typescript
export const switchModel = createAsyncThunk(
  'settings/switchModel',
  async ({ modelKey, currentConfig }, { dispatch, rejectWithValue }) => {
    dispatch(setDefaultModel(modelKey));  // 乐观更新
    await invoke('save_config', { config: newConfig });
    await invoke('switch_model', { name: modelKey });  // 失败！
    // 没有 rollback → UI 显示新模型，后端仍是旧模型
  }
);
```

**问题：** 第二步 `switch_model` 失败后没有 rollback。用户看到新模型已选，但实际上后端未切换。

**修复建议：**
```typescript
try {
  await invoke('save_config', { config: newConfig });
  await invoke('switch_model', { name: modelKey });
} catch (e) {
  dispatch(setDefaultModel(currentConfig.default_model));
  return rejectWithValue(String(e));
}
```

---

### 2.4 `useAutoScroll` 的 `MutationObserver` 强制同步 reflow

**文件:** `hooks/useAutoScroll.ts:53-65`

```typescript
const observer = new MutationObserver(() => {
  const isNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 200;
  if (isNearBottom) el.scrollTop = el.scrollHeight;
});
observer.observe(el, { childList: true, subtree: true, characterData: true });
```

**问题：** `subtree: true, characterData: true` 意味着**任何文本节点的任何变化**都会触发回调。streaming 时每秒 30-60 次 text 更新，每次回调中的 `scrollHeight` 读取会**强制浏览器同步 layout/reflow**（Forced Synchronous Layout），这是前端性能的头号杀手。

**影响：** 长会话 streaming 时明显的 UI 卡顿，CPU 占用飙升。

**修复建议：**
```typescript
let pendingScroll = false;
const observer = new MutationObserver(() => {
  if (pendingScroll) return;
  pendingScroll = true;
  requestAnimationFrame(() => {
    pendingScroll = false;
    // scroll logic
  });
});
```

---

### 2.5 `MarkdownContent` 在 streaming 时高频解析 markdown

**文件:** `components/chat/MarkdownContent.tsx:45-55`

```typescript
const html = useMemo(
  () => (renderAsMarkdown ? parseMarkdown(content) : null),
  [renderAsMarkdown, content],
);
```

**问题：** `marked.parse()` + `DOMPurify.sanitize()` 是**同步但 CPU 密集型**操作。当 `content.length > STREAM_PLAINTEXT_LIMIT (4000)` 时，streaming 期间每次 token 更新都会触发 full re-parse。对于 10KB+ 的 markdown，parse 时间可达 10-30ms。在 streaming 场景下（每秒 10-30 次更新），主线程被持续占用，帧率下降。

**修复建议：**
- 增加 debounce（200ms）到 markdown parse
- 或使用 Web Worker offload parse
- 或增加更大的 streaming buffer（如 20KB 才切换）

---

## 3. 🟡 重要问题 (High)

### 3.1 `App.tsx` 是 God Component（800+ 行）

`App` 组件直接管理：
- 全局状态订阅（9+ 个 `useSelector`）
- 业务逻辑（send, retry, abort, steer, title edit）
- 生命周期（session resume, auto-save, theme）
- 子 Agent 详情页渲染
- 键盘事件监听

**问题：** 单一组件职责过重，测试困难，任何小改动都可能导致整个应用 re-render。

**修复建议：** 拆分为：
```
App.tsx → Layout container
  ├── SidebarContainer.tsx
  ├── ChatArea.tsx
  │     ├── ChatHeader.tsx
  │     ├── ChatHistory.tsx
  │     └── ChatInputContainer.tsx
  └── SettingsModal.tsx
```

---

### 3.2 `chatSlice.ts` 双重存储 session 数据

**文件:** `features/chat/chatSlice.ts:64-72`

```typescript
interface ChatState {
  entries: ChatEntry[];           // 当前 session
  entriesBySession: Record<string, ChatEntry[]>;  // 所有 session 缓存
}
```

**问题：** 当前 session 的数据同时存在 `entries` 和 `entriesBySession[sessionId]` 中。streaming 更新只修改 `entries`，`entriesBySession` 中的缓存变为**过时数据**。切换 session 时可能恢复脏数据。

**修复建议：** 删除 `entries`，当前 session 通过 `entriesBySession[activeSessionId]` 派生，或使用单一 `Record<string, ChatEntry[]>` 存储。

---

### 3.3 `useAutocomplete` 没有 debounce 或 race 处理

**文件:** `hooks/useAutocomplete.ts:45-65`

```typescript
const fetchDirectoryEntries = useCallback(async (query: string) => {
  const entries = await invoke('search_files', { query, path: projectPath });
  setAutocompleteItems(mapped);
}, [projectPath]);
```

**问题：** 每次按键都触发 `invoke('search_files')`。用户输入 `@src/components/ChatInput.tsx` 时，会触发 20+ 次后端调用。没有 cancel / abort 机制，响应可能乱序。

**修复建议：** 添加 debounce（150ms）和 race cancellation：
```typescript
const fetchDirectoryEntries = useMemo(() => 
  debounce(async (query: string) => {
    // ...
  }, 150),
  [projectPath]
);
```

---

### 3.4 `AgentTurn.tsx` 的 diff 解析器鲁棒性不足

**文件:** `components/chat/AgentTurn.tsx:470-520`

```typescript
function parseUnifiedDiff(diffStr: string): DiffRow[] {
  const m = line.match(/@@ -\d+(?:(\d+))? \+(\d+)(?:(\d+))? @@/);
```

**问题：**
- 不支持 rename/mode change / binary diff
- `oldLine` 解析依赖第二个 regex，如果第一个 match 成功但第二个失败会 crash
- 不支持 `+++/---` 的 quoted path 格式

**修复建议：** 使用成熟的 diff 解析库（如 `diff` npm 包）或至少添加输入验证。

---

### 3.5 `ErrorBoundary` 重置后可能无限循环 crash

**文件:** `components/ErrorBoundary.tsx`

```typescript
handleReset = (): void => {
  this.setState({ hasError: false, error: null });
};
```

**问题：** 重置 state 后重新渲染相同的 children。如果导致 crash 的根本原因（props/state）没变，会立即再次 crash，进入无限循环。

**修复建议：**
```typescript
handleReset = (): void => {
  window.location.reload();  // 或导航到安全页面
};
```

---

### 3.6 缺少细粒度的 ErrorBoundary

`ErrorBoundary` 只在 `main.tsx` 中包裹了 `App`。App 内部的任何组件 crash 都会导致**整个应用白屏**。

**修复建议：** 在关键区域添加边界：
```tsx
<ErrorBoundary>
  <Sidebar />
</ErrorBoundary>
<ErrorBoundary>
  <ChatArea />
</ErrorBoundary>
```

---

### 3.7 `ChatInput.tsx` 的 icon 条件渲染效率低

**文件:** `components/chat/ChatInput.tsx:160-180`

```tsx
{item.icon === 'folder' && <FolderIcon />}
{item.icon === 'file' && <FileIcon />}
// ... 20+ 个条件
```

**问题：** 每次 render 评估所有条件。应使用映射表：
```tsx
const ICON_MAP: Record<string, ComponentType> = {
  folder: FolderIcon,
  file: FileIcon,
  // ...
};
const IconComponent = ICON_MAP[item.icon];
```

---

### 3.8 `useTokenCount` 全量计算无缓存

**文件:** `hooks/useTokenCount.ts`

```typescript
export function useTokenCount(): number {
  return useAppSelector((state) => {
    return state.chat.entries.reduce((sum, e) => {
      // 遍历所有 entries + blocks
    }, 0);
  });
}
```

**问题：** 每次 state 变化都 O(n) 遍历所有 entries。100+ entries 的会话中，每次 streaming update 都重新计算。

**修复建议：** 使用 `createSelector` 缓存，或增量更新 token count。

---

### 3.9 `store.ts` 缺少序列化检查中间件

**文件:** `store.ts`

```typescript
export const store = configureStore({
  reducer: { chat, settings, project },
});
```

**问题：** 没有配置 `serializableCheck`，非序列化数据可能被放入 state 而不被警告。这在调试时很难追踪。

**修复建议：**
```typescript
configureStore({
  reducer: { ... },
  middleware: (getDefaultMiddleware) => 
    getDefaultMiddleware({
      serializableCheck: {
        ignoredActions: [],
      },
    }),
});
```

---

### 3.10 `useGitBranch` 使用 `window.alert` 阻塞主线程

**文件:** `hooks/useGitBranch.ts:45-55`

```typescript
try {
  await invoke('switch_git_branch', { path, branch });
} catch (e) {
  window.alert(String(e));  // ← 阻塞！
}
```

**问题：** `window.alert` 在 Tauri 中弹出原生模态对话框，阻塞整个应用直到用户点击确认。

**修复建议：** 使用 toast / 内联错误提示替代 `window.alert`。

---

## 4. 🟢 中等问题 (Medium)

### 4.1 `chatSlice.ts` 1400+ 行，职责过重

包含：类型定义、辅助函数、事件处理器、reducer、thunk、selector。应拆分为：
```
features/chat/
  types.ts
  utils.ts          // helper functions
  eventHandlers.ts  // handleTurnStart, handleToolEnd, etc.
  chatSlice.ts      // reducer only
  selectors.ts      // selectEntryById, etc.
  thunks.ts         // resyncRun
```

### 4.2 `App.css` 魔法数字泛滥

大量硬编码像素值、颜色值。没有设计 token 系统（spacing scale, type scale）。

### 4.3 `AgentTurnUI` 的 `useMemo` 依赖在 streaming 时频繁失效

`entry` 对象每次 Redux update 都是新的引用，`useMemo` 的缓存基本无效。应考虑：
- 使用 `React.memo` + 自定义比较函数
- 或使用虚拟列表（如果会话很长）

### 4.4 `localStorage` 访问在 slice 顶层执行

**文件:** `features/project/projectSlice.ts:45-46`

```typescript
const savedActiveId = localStorage.getItem(STORAGE_KEY);
```

虽然没有 SSR 问题（Tauri 桌面应用），但模块加载时的副作用难以测试和 mock。

### 4.5 `handleSend` 中 `createSession` 和 `send_message` 不是原子操作

如果 `createSession` 成功但 `send_message` 失败，用户看到空 session 但没有消息。

---

## 5. 📊 架构评估

### ✅ 做得好的地方

| 方面 | 评价 |
|------|------|
| **Redux 切片架构** | Feature-based 清晰，RTK + Immer 减少样板代码 |
| **Streaming 优化** | rAF batching (`useAgentEventListener`)、plaintext fast-path (`MarkdownContent`)、`content-visibility: auto` |
| **组件粒度** | `AgentTurn` → `TurnIterationUI` → `ToolBlockUI` → `ApprovalBlockUI` 层次清晰 |
| **Tauri 集成** | 正确使用 `invoke`、`listen`、`@tauri-apps/plugin-dialog` |
| **TypeScript** | 类型定义较完整，特别是 `RunEventPayload` 的 discriminated union |
| **Gap detection + resync** | 前端自我修复机制设计精良 |
| **Auto-scroll** | `useAutoScroll` 的 force-stick 逻辑处理得当 |
| **Markdown 安全** | `DOMPurify.sanitize` 处理 AI 生成内容 |

### ⚠️ 架构风险

| 风险 | 说明 |
|------|------|
| **God Component** | `App.tsx` 800+ 行，几乎所有业务逻辑集中于此 |
| **性能瓶颈** | `MutationObserver` 同步 reflow + markdown 高频解析 |
| **状态同步** | `entries` / `entriesBySession` 双重存储可能不一致 |
| **无测试** | 零测试覆盖 |
| **Error 处理** | 大量 `console.error` + 静默失败，缺少用户反馈 |
| **Side-effect 泄露** | reducer 中的 `Promise` + `window.dispatchEvent` |

---

## 6. 🛠️ 改进建议（优先级排序）

| 优先级 | 建议 | 影响 |
|--------|------|------|
| **P0** | 从 `agentEventReceived` reducer 移除 side effect | Redux 正确性、调试 |
| **P0** | 修复 `selectEntryById` 缓存（改用 `createSelector`） | 性能 |
| **P0** | `switchModel` thunk 添加完整 rollback | 数据一致性 |
| **P1** | `useAutoScroll` MutationObserver 添加 rAF debounce | Streaming 性能 |
| **P1** | `MarkdownContent` 添加 markdown parse debounce | Streaming 性能 |
| **P1** | `useAutocomplete` 添加 debounce + race cancellation | 稳定性 |
| **P1** | 拆分 `App.tsx` / `chatSlice.ts` | 可维护性 |
| **P2** | 使用 `createSelector` 替换所有自定义 selector | 性能 |
| **P2** | 删除 `entriesBySession` 双重存储 | 内存/正确性 |
| **P2** | 为关键区域添加 `ErrorBoundary` | 健壮性 |
| **P2** | `ErrorBoundary` 重置改为页面刷新 | 健壮性 |
| **P2** | `DOMPurify` 添加明确配置 | 安全性 |
| **P3** | 添加单元测试（Jest/Vitest）+ E2E 测试 | 质量 |
| **P3** | CSS 设计 token 系统化 | 可维护性 |
| **P3** | 考虑使用 Zustand 替代 Redux 减少样板 | 开发效率 |

---

## 7. 📝 总结

**agent_core 前端**是一个功能完整、交互丰富的 AI Agent UI。Streaming 渲染、工具可视化、权限审批、子 Agent 嵌套等复杂场景都处理得相当不错。Redux + Tauri 的集成也很成熟。

**核心风险：**
1. **Reducer 中的 side effect** — 必须修复，违反 Redux 基本原则
2. **Streaming 性能瓶颈** — `MutationObserver` 同步 reflow + markdown 高频解析是卡顿主因
3. **`switchModel` 的部分更新** — 可能导致前后端状态不一致
4. **无缓存的 selector** — 大量 O(n) 遍历在大量数据时累积

如果修复 P0/P1 级别的问题，前端性能和健壮性会有显著提升。整体代码风格一致，组件拆分合理，是一个质量较高的桌面应用前端。

---

> **End of Review — Generated by WorkBuddy**
