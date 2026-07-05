# AI-NOTE-0007: A++ Code Quality Improvement Roadmap — Frontend (app/src/)

```yaml
---
id: AI-NOTE-0007
type: AI-NOTE
title: A++ Code Quality Improvement Roadmap — Frontend React/TypeScript (app/src/)
status: Draft
author: AI Agent
created: 2026-07-05
updated: 2026-07-05
related: [AI-NOTE-0006, PLAN-0009]
tags: [code-quality, frontend, react, typescript, accessibility, performance, A++]
---
```

## Context

当前前端代码库（`app/src/`）是一个基于 React 18 + TypeScript + Redux Toolkit + Tauri 的桌面应用前端。架构设计扎实——特征切片（features/）与组件分层（components/）清晰分离，Redux 状态管理使用 RTK 规范模式，聊天流的事件驱动状态机（eventHandlers.ts）设计精巧。但在实现层面存在 React 反模式、类型安全漏洞、无障碍缺失、性能隐患等方面的技术债务。

本文档是**从当前状态到 A++** 的详细改进路线图，覆盖 `app/src/` 全部代码（~100 文件），包含 components/、features/、hooks/、utils/ 四大域。每一项改进均包含：

- **问题描述** — 当前状态与风险
- **目标状态** — A++ 标准下的期望
- **具体方案** — 可执行的修改建议（含代码示例）
- **影响范围** — 涉及的文件与模块
- **预估工作量** — S/M/L
- **验收标准** — 如何确认改进完成

---

## 改进总览

| 阶段 | 主题 | 改进项数 | 目标评级 |
|------|------|---------|---------|
| Phase 0 | 关键正确性 Bug 修复 | 6 | → A- |
| Phase 1 | React 反模式与性能 | 7 | A- → A |
| Phase 2 | 类型安全与代码质量 | 6 | A → A+ |
| Phase 3 | 无障碍（Accessibility） | 4 | A+ → A+ |
| Phase 4 | 架构与代码组织优化 | 5 | A+ → A++ |
| Phase 5 | 错误处理与健壮性 | 4 | A++ 巩固 |

---

## Phase 0: 关键正确性 Bug 修复（P0 — 阻塞级）

### 0.1 修复 `setState` 在渲染体中调用 — React 反模式

**问题：** `EditFileWidget.tsx:21-24` 和 `ToolBlockUI.tsx:122-127` 在组件渲染体中直接调用 `setState`（"derived state from props" 反模式）。这会触发额外渲染周期，在 React 并发模式下行为不可预测。

**当前代码（EditFileWidget.tsx）：**
```tsx
const [prevActive, setPrevActive] = useState(active);

if (active !== prevActive) {
  setPrevActive(active);
  setCollapsed(!active);
}
```

**目标状态：** 使用 `useEffect` 同步 prop 变化，或用 `key` prop 重新挂载组件。

**具体方案：**
```tsx
// 方案 A：useEffect 同步
useEffect(() => {
  setCollapsed(!active);
}, [active]);

// 方案 B（更优）：用 key 重新挂载，消除派生状态
// 父组件：<EditFileWidget key={call_id} active={active} ... />
// 子组件初始化一次即可：const [collapsed, setCollapsed] = useState(!active);
```

ToolBlockUI.tsx 同理：
```tsx
useEffect(() => {
  if (!active && is_error) {
    setCollapsed(false);
  }
}, [active, is_error]);
```

**影响范围：** `components/chat/EditFileWidget.tsx`, `components/chat/ToolBlockUI.tsx`
**工作量：** S
**验收标准：** React DevTools Profiler 中无额外渲染周期；`grep -rn "if.*!==.*prev" app/src/components/chat/` 无结果

---

### 0.2 修复 `setTimeout` 未清理 — 内存泄漏

**问题：** 多个组件中的 `setTimeout` 在组件卸载时未清理，导致对已卸载组件调用 `setState`。

**受影响位置：**

| 文件 | 行号 | 描述 |
|------|------|------|
| `BashWidget.tsx` | 56 | 复制按钮 reset |
| `CodeBlock.tsx` | 200 | 复制按钮 reset |
| `AgentConfigTab.tsx` | 104 | 成功消息 reset |
| `DialogManager.tsx` | 44 | input focus 延迟 |
| `ReviewTab.tsx` | 313 | scrollIntoView 延迟 |

**目标状态：** 所有 `setTimeout` 在组件卸载或 effect 重新执行时清理。

**具体方案：**

提取通用 `useTimeout` hook：
```tsx
// hooks/useTimeout.ts
import { useRef, useCallback, useEffect } from 'react';

export function useTimeout() {
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const setTimer = useCallback((fn: () => void, ms: number) => {
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(fn, ms);
  }, []);

  useEffect(() => () => {
    if (timerRef.current) clearTimeout(timerRef.current);
  }, []);

  return setTimer;
}
```

使用：
```tsx
const setTimer = useTimeout();
// ...
setTimer(() => setCopied(false), 2000);
```

对于 ReviewTab.tsx 中的 effect 内 setTimeout：
```tsx
useEffect(() => {
  const timer = setTimeout(() => {
    const element = document.getElementById(`review-file-${filePath}`);
    if (element) element.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }, 150);
  return () => clearTimeout(timer);
}, [filePath]);
```

**影响范围：** 上述 5 个文件 + 新建 `hooks/useTimeout.ts`
**工作量：** S
**验收标准：** 组件卸载后控制台无 "setState on unmounted component" 警告；`grep -rn "setTimeout" app/src/components/ | grep -v clearTimeout` 仅匹配已在 effect 中有清理的调用

---

### 0.3 修复 NodeChatStream CSS 类名不匹配 — 样式完全失效

**问题：** `NodeChatStream.tsx:12` 使用类名 `node-chat-log-item`，但 `NodeChatStream.css:7` 定义的类名是 `node-chat-log`。基础容器样式和 `.system` 变体样式从未应用，执行流以无样式状态渲染。

**当前代码：**
```tsx
// NodeChatStream.tsx:12
className={`node-chat-log-item ${log.type}`}
```
```css
/* NodeChatStream.css:7 */
.node-chat-log { background: var(--bg-hover); border: 1px solid var(--border-color); ... }
.node-chat-log.system { border-color: var(--accent-border); ... }
```

**目标状态：** TSX 与 CSS 类名一致。

**具体方案：** 将 CSS 中的 `.node-chat-log` 重命名为 `.node-chat-log-item`：
```css
.node-chat-log-item { background: var(--bg-hover); border: 1px solid var(--border-color); ... }
.node-chat-log-item.system { border-color: var(--accent-border); ... }
```

**影响范围：** `components/workflow/NodeChatStream.css`
**工作量：** S
**验收标准：** 工作流节点执行流渲染时有正确的背景、边框和 padding

---

### 0.4 修复 EdgeConfigPanel 数据映射未持久化 — 数据丢失

**问题：** `EdgeConfigPanel.tsx` 中 `passThrough`、`sourceField`、`targetField` 三个状态从 `edge.data` 读取，但 **从未写回**。`applyLabel` 函数只调用 `onUpdate({ label })`，数据映射的所有 onChange 处理器仅更新本地状态。用户配置的数据映射在关闭面板后全部丢失。

**当前代码：**
```tsx
const applyLabel = () => { onUpdate({ label }); };
// 这些 onChange 只更新本地状态，从不调用 onUpdate：
onChange={(e) => setPassThrough(e.target.checked)}  // line 79
onChange={(e) => setSourceField(e.target.value)}     // line 91
onChange={(e) => setTargetField(e.target.value)}     // line 101
```

**目标状态：** 数据映射配置通过 `onUpdate` 持久化到父组件状态。

**具体方案：**
```tsx
const applyDataMapping = () => {
  onUpdate({
    data: {
      ...edge.data,
      data_mapping: {
        pass_through: passThrough,
        source_field: sourceField,
        target_field: targetField,
      },
    },
  });
};

// 在 checkbox 和 input 的 onBlur / onChange 中调用：
onChange={(e) => {
  setPassThrough(e.target.checked);
  // 立即同步（checkbox 适合即时更新）
  onUpdate({ data: { ...edge.data, data_mapping: { pass_through: e.target.checked, source_field: sourceField, target_field: targetField } } });
}}
onBlur={applyDataMapping}  // text input 适合 blur 时更新
```

**影响范围：** `components/workflow/EdgeConfigPanel.tsx`
**工作量：** S
**验收标准：** 配置数据映射 → 关闭面板 → 重新打开 → 配置仍存在

---

### 0.5 修复 `cost_usd.toFixed(4)` 在 undefined 时崩溃

**问题：** `NodeInspectorDrawer.tsx:96` 直接调用 `result.cost_usd.toFixed(4)`。如果 `cost_usd` 为 `undefined` 或 `null`（未完成节点或非 agent 节点），会抛出 `TypeError`，导致整个 drawer 崩溃。

**当前代码：**
```tsx
${result.cost_usd.toFixed(4)}
```

**目标状态：** 安全访问可选字段。

**具体方案：**
```tsx
${(result.cost_usd ?? 0).toFixed(4)}
```

**影响范围：** `components/workflow/NodeInspectorDrawer.tsx`
**工作量：** S
**验收标准：** 未完成节点打开 inspector 不崩溃；cost 显示 `0.0000` 而非报错

---

### 0.6 修复 `useEffect` 依赖缺失 — 过时闭包与规则违反

**问题：** 多个组件的 `useEffect` 引用了不在依赖数组中的函数，违反 React Hooks 规则，可能导致过时闭包。

**受影响位置：**

| 文件 | 行号 | 缺失依赖 |
|------|------|---------|
| `CronjobModal.tsx` | 49-54 | `loadJobs`, `loadSkills` |
| `SkillDrafts.tsx` | 57-59 | `loadDrafts` |
| `NewAgentModal.tsx` | 46-76 | `config?.default_model`（被 eslint-disable 抑制） |

**目标状态：** 所有 effect 依赖正确声明，或使用 `useCallback` 稳定引用。

**具体方案：**

以 CronjobModal 为例：
```tsx
const loadJobs = useCallback(async () => {
  try {
    const data = await invoke<CronJob[]>("list_cronjobs");
    setJobs(data);
  } catch (e) {
    console.error("Failed to load jobs:", e);
  }
}, []);

const loadSkills = useCallback(async () => {
  const data = await invoke<SkillInfo[]>("get_skills");
  setSkillsList(data.map((s) => ({ id: s.name, name: s.name })));
}, []);

useEffect(() => {
  if (isOpen) {
    loadJobs();
    loadSkills();
  }
}, [isOpen, loadJobs, loadSkills]);
```

对 NewAgentModal，移除 eslint-disable 并添加 `config?.default_model` 到依赖：
```tsx
useEffect(() => {
  if (isOpen) {
    // ... uses config?.default_model
  }
}, [isOpen, editingAgent, config?.default_model]);
```

**影响范围：** 上述 3 个文件
**工作量：** S
**验收标准：** `npx eslint app/src/components/ui/ app/src/components/agents/ --rule '{"react-hooks/exhaustive-deps": "error"}'` 无警告

---

## Phase 1: React 反模式与性能优化（P1）

### 1.1 消除 `SubagentDetailPage` 中非记忆化对象 — 击穿 memo 性能

**问题：** `SubagentDetailPage.tsx:19-25` 在每次渲染时创建新的 `syntheticEntry` 对象字面量。该对象传给被 `memo()` 包裹的 `AgentTurnUI`，但由于引用每次都变，memo 完全失效——`AgentTurnUI` 及其所有子组件（markdown 解析、代码高亮等）每次都重新渲染。

**当前代码：**
```tsx
const syntheticEntry = {
  id: `subagent-detail-${subagent.id}`,
  type: 'turn' as const,
  blocks: convertSubagentBlocks(subagent.blocks),
  startTime: subagent.startTime,
  endTime: subagent.endTime,
};
```

**目标状态：** 使用 `useMemo` 稳定对象引用。

**具体方案：**
```tsx
const syntheticEntry = useMemo(() => ({
  id: `subagent-detail-${subagent.id}`,
  type: 'turn' as const,
  blocks: convertSubagentBlocks(subagent.blocks),
  startTime: subagent.startTime,
  endTime: subagent.endTime,
}), [subagent.id, subagent.blocks, subagent.startTime, subagent.endTime]);
```

**影响范围：** `components/chat/SubagentDetailPage.tsx`
**工作量：** S
**验收标准：** React DevTools Profiler 中，父组件渲染时 `AgentTurnUI` 不再无意义重渲染

---

### 1.2 消除 `ToolBlockUI` 内联 `<style>` — DOM 膨胀

**问题：** `ToolBlockUI.tsx:217-238` 在每个组件实例内渲染一个 `<style>` 标签，包含全局 CSS 规则。如果一个会话有 50 个工具调用，DOM 中就有 50 个相同的 `<style>` 元素。既是性能问题（DOM 膨胀），也是正确性问题（全局 CSS 重复注入）。

**当前代码：**
```tsx
<div className="step-block">
  <style>{`
    .search-tool-result { font-size: 13px !important; }
    .search-tool-result h1, ... { ... }
    .scrollable-markdown { max-height: 400px; overflow-y: auto; }
    .tool-result-content { ... }
  `}</style>
```

**目标状态：** 样式提取到 CSS 文件，每个类名仅一份。

**具体方案：** 将这些规则移到 `App.css` 或新建 `components/chat/ToolBlockUI.css`：
```css
/* ToolBlockUI.css */
.search-tool-result { font-size: 13px !important; }
.search-tool-result h1, .search-tool-result h2, .search-tool-result h3 { ... }
.scrollable-markdown { max-height: 400px; overflow-y: auto; }
.tool-result-content { ... }
```

然后在组件中 `import './ToolBlockUI.css'` 并移除 `<style>` 标签。

**影响范围：** `components/chat/ToolBlockUI.tsx` + 新建 CSS 文件
**工作量：** S
**验收标准：** DOM 中无内联 `<style>` 标签来自 ToolBlockUI；样式外观不变

---

### 1.3 修复 `SubagentWidgets` 过时计时器 — 时间显示不更新

**问题：** `SubagentWidgets.tsx:66-68` 在 `useMemo` 中使用 `Date.now() - subagent.startTime` 计算已用时间，但依赖数组是 `[subagent, toolCount]`。没有 interval 强制重渲染，时间显示在 subagent 工作期间永不更新——只有 subagent 状态变化时才刷新一次。

**当前代码：**
```tsx
const statusText = useMemo(() => {
  if (subagent.status === 'working') {
    const elapsed = formatTime(Date.now() - subagent.startTime); // stale!
    return `Working · ${toolCount} tools · ${elapsed}`;
  }
}, [subagent, toolCount]);
```

**目标状态：** working 状态下有定时器驱动更新。

**具体方案：** 复用已有的 `ProcessingTimer` 模式（1s interval）：
```tsx
const [now, setNow] = useState(Date.now());
useEffect(() => {
  if (subagent?.status !== 'working') return;
  const interval = setInterval(() => setNow(Date.now()), 1000);
  return () => clearInterval(interval);
}, [subagent?.status]);

const statusText = useMemo(() => {
  if (subagent.status === 'working') {
    const elapsed = formatTime(now - subagent.startTime);
    return `Working · ${toolCount} tools · ${elapsed}`;
  }
}, [subagent, toolCount, now]);
```

**影响范围：** `components/chat/SubagentWidgets.tsx`
**工作量：** S
**验收标准：** subagent 工作期间，计时器每秒更新

---

### 1.4 修复 `WorkflowEditor` 同步 effect — 每次鼠标移动触发 Redux dispatch

**问题：** `WorkflowEditor.tsx:86-93` 的 `useEffect` 依赖 `nodes` 和 `edges`。拖拽节点时，React Flow 在每次 `mousemove` 触发 `onNodesChange`，更新 Redux 中的 `nodes`。每次更新触发该 effect，将所有节点和边映射并 dispatch `updateActiveWorkflowNodes`。对于有大量节点的工作流，这造成严重的拖拽卡顿。

**当前代码：**
```tsx
useEffect(() => {
  if (activeWorkflowId && (nodes.length > 0 || edges.length > 0) && !isExecuting) {
    dispatch(updateActiveWorkflowNodes({
      nodes: nodes.map(n => rfToNodeDef(n, activeWorkflowId)),
      edges: edges.map(e => rfToEdgeDef(e, activeWorkflowId)),
    }));
  }
}, [nodes, edges, activeWorkflowId, dispatch, isExecuting]);
```

**目标状态：** 防抖同步，拖拽期间不触发 Redux dispatch。

**具体方案：**
```tsx
const syncTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

useEffect(() => {
  if (!activeWorkflowId || isExecuting) return;
  if (nodes.length === 0 && edges.length === 0) return;

  // Debounce: wait 500ms after last change before syncing
  if (syncTimerRef.current) clearTimeout(syncTimerRef.current);
  syncTimerRef.current = setTimeout(() => {
    dispatch(updateActiveWorkflowNodes({
      nodes: nodes.map(n => rfToNodeDef(n, activeWorkflowId)),
      edges: edges.map(e => rfToEdgeDef(e, activeWorkflowId)),
    }));
  }, 500);

  return () => {
    if (syncTimerRef.current) clearTimeout(syncTimerRef.current);
  };
}, [nodes, edges, activeWorkflowId, dispatch, isExecuting]);
```

**影响范围：** `components/workflow/WorkflowEditor.tsx`
**工作量：** S
**验收标准：** 拖拽节点时无明显卡顿；松手 500ms 后 Redux 状态同步

---

### 1.5 修复 `NodePropertiesPanel` 每次按键触发 Redux dispatch

**问题：** `NodePropertiesPanel.tsx` 中所有文本输入的 `onChange` 直接调用 `onUpdateNode`，每次 dispatch `updateNodeData` 到 Redux。对于大文本字段（input_template、JSON schema），每次按键触发完整的 Redux 状态更新 + 整个 WorkflowEditor 树重渲染，造成明显输入延迟。

**目标状态：** 本地状态 + 防抖 dispatch。

**具体方案：**
```tsx
// 使用本地状态暂存，blur 或防抖后同步
const [localLabel, setLocalLabel] = useState(nodeData.label ?? '');
const [debounceRef, setDebounceRef] = useState<ReturnType<typeof setTimeout> | null>(null);

const debouncedUpdate = useCallback((data: Record<string, unknown>) => {
  if (debounceRef) clearTimeout(debounceRef);
  const timer = setTimeout(() => {
    onUpdateNode(selectedNode.id, data);
  }, 300);
  setDebounceRef(timer);
}, [selectedNode.id, onUpdateNode, debounceRef]);

// input:
value={localLabel}
onChange={(e) => {
  setLocalLabel(e.target.value);
  debouncedUpdate({ ...nodeData, label: e.target.value });
}}
```

或更简单的方案——使用 `onBlur` 持久化（与 WorkflowToolbar 的 name input 一致）。

**影响范围：** `components/workflow/NodePropertiesPanel.tsx`
**工作量：** M
**验收标准：** 在 input_template 大文本框中连续输入无延迟

---

### 1.6 消除 `MarkdownContent` 死代码 — 未实现的 streaming 快速路径

**问题：** `MarkdownContent.tsx:94` 声明了 `isStreaming` prop 但从未解构使用。`lines 169-195` 有一个 `if (segments)` 分支永远为 true（因为 `useMemo` 总返回数组），下面的 fallback 是不可达死代码。这暗示 streaming 优化从未完成。

**目标状态：** 要么实现 streaming 快速路径，要么移除死代码。

**具体方案（移除死代码）：**
```tsx
// 移除类型中的 isStreaming prop
export const MarkdownContent = memo(function MarkdownContent({
  content,
  className,
  plainText = false,
}: {
  content: string;
  className?: string;
  plainText?: boolean;
}) {
  // ... 直接返回 segments 渲染结果，移除 if (segments) 分支和 fallback
```

同时移除所有调用方传入的 `isStreaming={...}` prop。

**影响范围：** `components/chat/MarkdownContent.tsx` + 所有调用方
**工作量：** S
**验收标准：** `grep -rn "isStreaming" app/src/components/chat/MarkdownContent.tsx` 无结果

---

### 1.7 修复 `ReviewTab` effect 依赖 `fileContents` 对象 — 事件监听器频繁重注册

**问题：** `ReviewTab.tsx:300-324` 的 `useEffect` 依赖 `fileContents` 状态对象。每次文件内容加载完成，整个事件监听器被移除并重新添加，效率低下且可能丢失事件。

**目标状态：** 使用 ref 访问最新状态，effect 只运行一次。

**具体方案：**
```tsx
const fileContentsRef = useRef(fileContents);
fileContentsRef.current = fileContents;

useEffect(() => {
  const handleOpen = (e: Event) => {
    const customEvent = e as CustomEvent;
    if (customEvent.detail?.filePath) {
      const filePath = customEvent.detail.filePath;
      setExpandedFiles(prev => {
        const next = new Set(prev);
        next.add(filePath);
        return next;
      });
      if (!fileContentsRef.current[filePath]) {
        fetchFileContent(filePath);
      }
      // ...
    }
  };
  window.addEventListener('open-right-sidebar', handleOpen);
  return () => window.removeEventListener('open-right-sidebar', handleOpen);
}, []); // 空依赖——只注册一次
```

**影响范围：** `components/review/ReviewTab.tsx`
**工作量：** S
**验收标准：** 加载文件内容时 `open-right-sidebar` 监听器不被重复注册/注销

---

## Phase 2: 类型安全与代码质量（P2）

### 2.1 消除 `any` 类型 — 全面类型安全

**问题：** 前端代码中存在大量 `any` 类型使用，完全绕过 TypeScript 类型检查。

**受影响位置清单（按域分组）：**

| 域 | 文件 | 行号 | `any` 用法 |
|------|------|------|-----------|
| Chat | `ToolBlockUI.tsx` | 58 | `match[3] as any` |
| Chat | `ToolBlockUI.tsx` | 301 | `results.map((r: any, idx) =>` |
| Layout | `FileTree.tsx` | 155,228,230 | `any[]` for directory listing |
| Review | `ReviewTab.tsx` | 14,20,23,26 | `diffRows: any[]`, `useState<any[]>` |
| Review | `OverviewTab.tsx` | 49,90 | `getParsedArgs = (rawArgs: any)`, `(sa: any)` |
| Settings | `PermissionsTab.tsx` | 56 | `useDispatch<any>()` |
| Settings | `MemoryTab.tsx` | 336,366,368 | `ReturnType<typeof useSelector<RootState, any>>`, `provider: any` |
| UI | `CronjobModal.tsx` | 67,291 | `invoke<any[]>("get_skills")` |
| UI | `NewAgentModal.tsx` | 80 | `invoke<any[]>("get_skills")` |
| Agents | `AgentConfigTab.tsx` | 64,188 | `invoke<any[]>`, `provider: any` |
| Workflow | `WorkflowEditor.tsx` | 207 | `data: any` |
| Workflow | `WorkflowCanvas.tsx` | 20-21 | `changes: any` |
| Workflow | `NodePropertiesPanel.tsx` | 13,26-27 | `data: any`, `Record<string, any>` |

**目标状态：** 零 `any` 类型（`any[]` 和裸 `any`），使用具体接口。

**具体方案：**

1. **`invoke<any[]>("get_skills")` → 定义 SkillInfo 接口**（4 处复用）：
```tsx
// features/chat/types.ts
export interface SkillInfo {
  name: string;
  description: string;
  version?: string;
  triggers?: string[];
}
// 使用：invoke<SkillInfo[]>("get_skills")
```

2. **FileTree `any[]` → 定义 DirEntry 接口**：
```tsx
interface DirEntry { name: string; type: 'file' | 'dir'; size: string; }
const result = await invoke<DirEntry[]>('list_directory', { path: node.path });
```

3. **ReviewTab `any[]` → 定义 DiffRow 接口**：
```tsx
interface DiffRow {
  type: 'add' | 'del' | 'context' | 'gap';
  oldLineNo: number | null;
  newLineNo: number | null;
  oldText: string;
  newText: string;
}
```

4. **WorkflowCanvas `changes: any` → 使用 React Flow 类型**：
```tsx
import { type NodeChange, type EdgeChange } from '@xyflow/react';
onNodesChange: (changes: NodeChange[]) => void;
onEdgesChange: (changes: EdgeChange[]) => void;
```

5. **`useDispatch<any>()` → 使用已有的 `useAppDispatch`**：
```tsx
// 替换
const dispatch = useAppDispatch();
```

**影响范围：** 上述所有文件
**工作量：** M
**验收标准：** `grep -rn ": any" app/src/ --include="*.ts" --include="*.tsx" | grep -v node_modules | grep -v ".d.ts"` 返回 0 结果（或仅剩有注释豁免的）

---

### 2.2 消除 `confirm()` / `alert()` 阻塞调用

**问题：** 多个组件使用原生 `confirm()` 和 `alert()`，这些在 Tauri WebView 中行为不一致且阻塞 UI 线程。代码库已有 `DialogManager` / `useConfirmDialog` 替代方案。

**受影响位置：**

| 文件 | 行号 | 调用 |
|------|------|------|
| `AgentDashboard.tsx` | 19 | `confirm(\`Are you sure...?\`)` |
| `ProviderTab.tsx` | 351 | `confirm(\`Delete provider...?\`)` |
| `CronjobModal.tsx` | 92 | `alert(\`Error creating job: ${e}\`)` |

**目标状态：** 统一使用 `useConfirmDialog` hook 和内联错误状态。

**具体方案（以 AgentDashboard 为例）：**
```tsx
import { useConfirmDialog } from '../ui/DialogManager';

const { confirm } = useConfirmDialog();

const handleDelete = async () => {
  const ok = await confirm({
    title: 'Delete Agent',
    message: `Are you sure you want to delete ${agent.name}?`,
    confirmLabel: 'Delete',
    cancelLabel: 'Cancel',
    danger: true,
  });
  if (!ok) return;
  try {
    await dispatch(deleteAgent(agent.id)).unwrap();
    dispatch(setSelectedAgent(null));
  } catch (e) {
    // 设置错误状态显示给用户
    setError(String(e));
  }
};
```

**影响范围：** 上述 3 个文件
**工作量：** S
**验收标准：** `grep -rn "confirm(\|alert(" app/src/components/ | grep -v "useConfirmDialog\|confirmLabel\|confirm \""` 无原生调用

---

### 2.3 修复冗余/死代码 — 逻辑错误与误导

**问题：** 多处冗余三元表达式（两个分支返回相同值）和死代码。

**受影响位置：**

| 文件 | 行号 | 问题 |
|------|------|------|
| `EditFileWidget.tsx` | 43 | `summary ? 'Edited' : 'Edited'` — 两分支相同 |
| `EditFileWidget.tsx` | 51 | `active ? 'var(--text-muted)' : 'var(--text-muted)'` — 两分支相同 |
| `ChatInput.tsx` | 293 | 无 onClick、无 aria-label 的死按钮 |
| `SkillSelector.tsx` | 216 | `/* TODO: open settings */` — 死链接 |
| `AgentMemoryTab.tsx` | 47-48 | 硬编码占位文本假装是真实数据 |
| `AgentSkillsTab.tsx` | 66-68 | 所有 skill 卡片显示相同通用描述 |
| `MarkdownContent.tsx` | 41,59 | 导出但无消费者的函数 |

**目标状态：** 移除死代码，修复冗余逻辑。

**具体方案：**
```tsx
// EditFileWidget.tsx:43 — 简化
const labelPrefix = active ? 'Editing' : is_error ? 'Edit failed:' : 'Edited';

// EditFileWidget.tsx:51 — 简化
color={is_error ? 'var(--danger)' : 'var(--text-muted)'}

// ChatInput.tsx:293 — 移除或实现
// 删除 <button className="icon-btn"><PlusIcon size={16} /></button>

// SkillSelector.tsx:216 — 移除死链接或实现导航
// 改为纯文本：<span>Install skills in Settings</span>

// AgentMemoryTab.tsx:47-48 — 调用 invoke('get_agverse_md') 获取真实数据
// 或明确标记为 coming-soon
```

**影响范围：** 上述 7 个文件
**工作量：** S
**验收标准：** `grep -rn "TODO" app/src/components/ | grep -v "node_modules"` 仅剩有意保留的标记

---

### 2.4 消除 IIFE in JSX — 可读性与性能

**问题：** 多个文件在 JSX 中使用 `{(() => { ... })()}` IIFE 模式计算图标，每次渲染创建新闭包。

**受影响位置：** `BashWidget.tsx:68`, `EditFileWidget.tsx:51`, `ReadFileWidget.tsx:37`, `ToolBlockUI.tsx:245,294`, `NodePropertiesPanel.tsx:139-154`

**目标状态：** 在 return 之前提取为变量或 `useMemo`。

**具体方案：**
```tsx
// 之前：
{(() => { const ToolIcon = getToolIcon(toolName); return <ToolIcon size={13} ... />; })()}

// 之后：
const ToolIcon = getToolIcon(toolName);
// in JSX:
<ToolIcon size={13} className="step-icon" color={...} />
```

对于 `NodePropertiesPanel.tsx:139-154` 的复杂 IIFE，提取为 `useMemo`：
```tsx
const downstream = useMemo(() =>
  edges.filter(e => e.source === selectedNode.id).map(e => {
    const tn = nodes.find(n => n.id === e.target);
    return { id: e.target, label: (tn?.data as Record<string, unknown>)?.label as string ?? e.target };
  }), [edges, nodes, selectedNode.id]);
```

**影响范围：** 上述 6 个文件
**工作量：** S
**验收标准：** `grep -rn "{(()" app/src/components/ | grep -v node_modules` 无结果

---

### 2.5 修复 CSS 变量自引用与 `!important` 滥用

**问题：**
1. `WorkflowEditor.css:18` 和 `WorkflowToolbar.css:38-39,47-48,58,70,75` 中 `var(--x, var(--x))` 自引用——fallback 与变量本身相同，毫无意义。
2. `WorkflowCanvas.css` 和 `nodes.css` 中大量 `!important` 用于覆盖 React Flow 默认样式。

**目标状态：** 移除自引用 fallback；用更高优先级选择器替代 `!important`。

**具体方案：**
```css
/* 之前 */
color: var(--danger, var(--danger));
background: var(--success, var(--success));

/* 之后 */
color: var(--danger, #ef4444);
background: var(--success, #22c55e);
```

对于 React Flow 覆盖，使用更具体的选择器：
```css
/* 之前 */
.react-flow__node-input { background: none !important; ... }

/* 之后——使用更高特异性 */
.workflow-canvas-container .react-flow__node-input { background: none; ... }
```

**影响范围：** `components/workflow/*.css`
**工作量：** S
**验收标准：** `grep -rn "var(--.*var(--" app/src/` 无结果

---

### 2.6 修复 `@ts-ignore` — 使用类型增强

**问题：** `CustomTitleBar.tsx:25-28,52-55` 使用 4 处 `@ts-ignore` 抑制 `WebkitAppRegion` 和 `appRegion` 的类型错误。

**目标状态：** 通过模块增强扩展 `React.CSSProperties` 类型。

**具体方案：** 创建类型增强文件：
```tsx
// types/css-properties.d.ts
import 'react';

declare module 'react' {
  interface CSSProperties {
    WebkitAppRegion?: 'drag' | 'no-drag' | 'none';
    appRegion?: 'drag' | 'no-drag' | 'none';
  }
}
```

然后移除所有 `@ts-ignore`：
```tsx
const dragStyle: React.CSSProperties = {
  WebkitAppRegion: 'drag',
  appRegion: 'drag',
};
```

**影响范围：** `components/layout/CustomTitleBar.tsx` + 新建 `types/css-properties.d.ts`
**工作量：** S
**验收标准：** `grep -rn "@ts-ignore" app/src/ | grep -v node_modules | grep -v ".d.ts"` 无结果

---

## Phase 3: 无障碍（Accessibility）（P3）

### 3.1 消除可点击 `div` — 语义化 HTML

**问题：** 整个前端大量使用 `<div onClick={...}>` 作为交互元素，缺乏 `role`、`tabIndex`、`onKeyDown`。键盘用户无法导航或激活这些控件，屏幕阅读器不会将其识别为交互元素。

**受影响位置（按域统计）：**

| 域 | 文件 | 数量 |
|------|------|------|
| Chat | `AgentTurn.tsx`, `BashWidget.tsx`, `EditFileWidget.tsx`, `TodoPanel.tsx`, `TurnIterationUI.tsx` | ~8 |
| Layout | `Sidebar.tsx`, `FileTree.tsx` | ~8 |
| Agents | `AgentList.tsx` | 1 |
| Settings | `PermissionsTab.tsx` | 1 |
| Review | `OverviewTab.tsx`, `ReviewTab.tsx` | ~4 |
| Workflow | `WorkflowSidebar.tsx`, `WorkflowRunView.tsx` | ~5 |
| Chat | `UserRow.tsx` (spans as buttons) | 5 |

**目标状态：** 所有交互元素使用 `<button>` 或添加完整的 ARIA 属性。

**具体方案（优先使用 `<button>`）：**
```tsx
// 之前
<div className="agent-list-item" onClick={() => onSelect(agent.id)}>
  {agent.name}
</div>

// 之后
<button
  className="agent-list-item"
  onClick={() => onSelect(agent.id)}
  aria-selected={selectedAgentId === agent.id}
  role="option"
>
  {agent.name}
</button>
```

对于必须保留 `<div>` 的情况（如拖拽元素），添加完整 ARIA：
```tsx
<div
  role="button"
  tabIndex={0}
  aria-label="Toggle section"
  aria-expanded={expanded}
  onClick={handleToggle}
  onKeyDown={(e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      handleToggle();
    }
  }}
>
```

**影响范围：** 上述所有文件
**工作量：** L
**验收标准：** 键盘 Tab 导航可达所有交互元素；`grep -rn "div.*onClick" app/src/components/ | grep -v role | grep -v node_modules` 返回 0

---

### 3.2 为模态框/抽屉添加焦点管理与 ARIA

**问题：** `WorkflowRunView`、`NodeInspectorDrawer`、`CronjobModal`、`NewAgentModal` 等模态/抽屉组件缺少 `role="dialog"`、`aria-modal="true"`、Escape 键关闭、焦点陷阱。

**目标状态：** 所有模态框遵循 WAI-ARIA Dialog 模式。

**具体方案：**

提取通用 `useModal` hook：
```tsx
// hooks/useModal.ts
import { useEffect, useRef } from 'react';

export function useModal(isOpen: boolean, onClose: () => void) {
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!isOpen) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
      if (e.key === 'Tab') {
        // 简单焦点陷阱
        const focusable = dialogRef.current?.querySelectorAll(
          'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'
        );
        if (!focusable || focusable.length === 0) return;
        const first = focusable[0] as HTMLElement;
        const last = focusable[focusable.length - 1] as HTMLElement;
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };

    document.addEventListener('keydown', handleKeyDown);
    dialogRef.current?.focus();

    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [isOpen, onClose]);

  return { dialogRef, modalProps: { role: 'dialog', 'aria-modal': true } };
}
```

**影响范围：** 上述 4+ 个模态组件 + 新建 `hooks/useModal.ts`
**工作量：** M
**验收标准：** 所有模态可通过 Escape 关闭；Tab 键焦点不逃逸到背景；屏幕阅读器播报 "dialog"

---

### 3.3 为表单 label 关联 input

**问题：** `NodePropertiesPanel.tsx`、`EdgeConfigPanel.tsx` 等组件中 `<label>` 与 `<input>` 是兄弟元素，缺少 `htmlFor`/`id` 关联。屏幕阅读器无法关联标签与输入框。

**目标状态：** 所有 label 通过 `htmlFor` 关联 input。

**具体方案：**
```tsx
// 之前
<label className="node-properties-label">Label</label>
<input className="settings-input" value={...} onChange={...} />

// 之后
<label className="node-properties-label" htmlFor="node-label-input">Label</label>
<input id="node-label-input" className="settings-input" value={...} onChange={...} />
```

**影响范围：** `components/workflow/NodePropertiesPanel.tsx`, `components/workflow/EdgeConfigPanel.tsx`
**工作量：** S
**验收标准：** axe DevTools 扫描无 "label not associated" 违规

---

### 3.4 为自动补全下拉添加 ARIA combobox 模式

**问题：** `ChatInput.tsx:235-266` 的自动补全下拉缺少 `role="listbox"`、`role="option"`、`aria-selected`、`aria-activedescendant`。屏幕阅读器无法播报下拉选项。

**目标状态：** 遵循 WAI-ARIA Combobox 模式。

**具体方案：**
```tsx
<input
  role="combobox"
  aria-expanded={showAutocomplete}
  aria-controls="mention-listbox"
  aria-activedescendant={selectedIndex >= 0 ? `mention-option-${selectedIndex}` : undefined}
  // ...
/>
{showAutocomplete && (
  <div id="mention-listbox" role="listbox">
    {items.map((item, idx) => (
      <div
        key={item.id}
        id={`mention-option-${idx}`}
        role="option"
        aria-selected={idx === selectedIndex}
        onClick={() => handleSelect(item)}
      >
        {item.label}
      </div>
    ))}
  </div>
)}
```

**影响范围：** `components/chat/ChatInput.tsx`
**工作量：** S
**验收标准：** VoiceOver / NVDA 能播报 "combobox"、选项数量和当前选中项

---

## Phase 4: 架构与代码组织优化（P4）

### 4.1 提取可复用的 Model/Skill/Tool 选择器组件

**问题：** Model 下拉、Skill 下拉（带搜索、checkbox 样式）、Tool 下拉在 `NewAgentModal.tsx:223-317`、`AgentConfigTab.tsx:178-269`、`CronjobModal.tsx` 中近乎逐字复制——约 600 行重复代码。任何修复必须同时修改多处。

**目标状态：** 提取为可复用的 `<ModelDropdown>`、`<SkillMultiSelect>`、`<ToolMultiSelect>` 组件。

**具体方案：**
```tsx
// components/shared/ModelDropdown.tsx
interface ModelDropdownProps {
  value: string;
  onChange: (modelKey: string) => void;
  config: AppConfig;
  label?: string;
}
export function ModelDropdown({ value, onChange, config, label }: ModelDropdownProps) { ... }

// components/shared/SkillMultiSelect.tsx
interface SkillMultiSelectProps {
  selected: string[];
  onChange: (skills: string[]) => void;
  label?: string;
}
export function SkillMultiSelect({ selected, onChange, label }: SkillMultiSelectProps) { ... }

// components/shared/ToolMultiSelect.tsx — 类似
```

在 `NewAgentModal`、`AgentConfigTab`、`CronjobModal` 中复用。

**影响范围：** 新建 `components/shared/` + 修改 3 个消费方文件
**工作量：** M
**验收标准：** 3 个文件中的下拉代码替换为单行组件调用；总行数减少 ~400 行

---

### 4.2 提取可复用的 SessionList 组件

**问题：** `Sidebar.tsx:514-591`（Projects section）和 `627-718`（Chat section）的 session 渲染逻辑几乎相同——相同的 maxInitial 逻辑、相同的 session row 渲染、相同的展开/折叠。约 80 行重复。

**目标状态：** 提取为 `<SessionList>` 组件。

**具体方案：**
```tsx
// components/layout/SessionList.tsx
interface SessionListProps {
  sessions: Session[];
  activeSessionId: string | null;
  onSelectSession: (sessionId: string) => void;
  onDeleteSession?: (sessionId: string) => void;
  title: string;
}
export function SessionList({ sessions, activeSessionId, onSelectSession, onDeleteSession, title }: SessionListProps) {
  const [expanded, setExpanded] = useState(false);
  const maxInitial = 6;
  const visible = expanded ? sessions : sessions.slice(0, maxInitial);
  // ... 渲染逻辑
}
```

**影响范围：** `components/layout/Sidebar.tsx` + 新建 `SessionList.tsx`
**工作量：** S
**验收标准：** Sidebar 中两处 session 列表替换为 `<SessionList>` 调用

---

### 4.3 提取共享的 modifiedFiles 提取逻辑

**问题：** `ReviewTab.tsx:327-399` 和 `OverviewTab.tsx:47-83` 中 `getParsedArgs` helper 和 `modifiedFiles` 提取逻辑近乎逐字复制。

**目标状态：** 提取到共享工具模块。

**具体方案：**
```tsx
// utils/modifiedFilesExtractor.ts
import type { ChatEntry } from '../features/chat/types';

export interface ModifiedFile {
  path: string;
  result: string;
  timestamp: number;
  args?: Record<string, unknown>;
}

export function extractModifiedFiles(entries: ChatEntry[]): ModifiedFile[] {
  const files = new Map<string, ModifiedFile>();
  for (const entry of entries) {
    if (entry.type !== 'turn' || !entry.blocks) continue;
    for (const b of entry.blocks) {
      if (b.type !== 'tool') continue;
      if (b.name === 'edit' || b.name === 'write_file' || b.name === 'write_to_file') {
        const args = getParsedArgs(b.args);
        const filePath = args.file_path as string;
        if (filePath) {
          files.set(filePath, { path: filePath, result: b.result, timestamp: entry.startTime ?? 0, args });
        }
      }
    }
  }
  return Array.from(files.values());
}

export function getParsedArgs(rawArgs: unknown): Record<string, unknown> {
  if (typeof rawArgs === 'string') {
    try { return JSON.parse(rawArgs); } catch { return {}; }
  }
  return (rawArgs as Record<string, unknown>) ?? {};
}
```

**影响范围：** 新建 `utils/modifiedFilesExtractor.ts` + 修改 `ReviewTab.tsx`, `OverviewTab.tsx`
**工作量：** S
**验收标准：** 两个文件中的重复逻辑替换为 import 调用

---

### 4.4 提取 `useTimeout` 和 `useModal` 通用 hooks

**问题：** `setTimeout` 未清理的问题（P0.2）和模态框无障碍问题（P3.2）在多个组件中重复出现。应当提取为通用 hooks 供全局复用。

**目标状态：** `hooks/useTimeout.ts` 和 `hooks/useModal.ts` 可在所有组件中复用。

**具体方案：** 见 P0.2 和 P3.2 中的实现。这两个 hooks 在创建后应替换所有手动 `setTimeout` 和模态框焦点管理代码。

**影响范围：** 新建 2 个 hooks + 修改所有使用了 `setTimeout` 和模态框的组件
**工作量：** S
**验收标准：** 所有 `setTimeout` 使用 `useTimeout`；所有模态使用 `useModal`

---

### 4.5 消除魔法数字 — 提取为命名常量

**问题：** 多个文件中存在未命名的魔法数字。

**受影响位置清单：**

| 文件 | 行号 | 魔法数字 | 含义 |
|------|------|---------|------|
| `utils.ts` | 131 | `5000` | MAX_RESULT_LEN |
| `Sidebar.tsx` | 515,639 | `6` | MAX_INITIAL_SESSIONS |
| `ProviderTab.tsx` | 173-179 | `32768`, `100`, `1800` | 默认 token/迭代/超时 |
| `NewAgentModal.tsx` | 39-40 | `50`, `32000` | 默认迭代/token |
| `WorkflowRunView.tsx` | 41,71 | `20`, `1500` | 历史限制/轮询间隔 |
| `WorkflowRunView.tsx` | 169,175 | `800`, `500` | 输出/输入截断长度 |
| `AgentHistoryTab.tsx` | 18 | `50` | 历史页大小 |
| `SkillDrafts.tsx` | 67 | `100` | 生成限制 |

**目标状态：** 所有魔法数字提取为模块级命名常量。

**具体方案：**
```tsx
// 每个文件顶部
const MAX_RESULT_LEN = 5000;
const MAX_INITIAL_SESSIONS = 6;
const DEFAULT_MAX_CONTEXT_TOKENS = 32768;
const DEFAULT_MAX_ITERATIONS = 100;
const POLL_INTERVAL_MS = 1500;
const RUN_HISTORY_LIMIT = 20;
// ...
```

**影响范围：** 上述所有文件
**工作量：** S
**验收标准：** 代码审查中无裸数字常量

---

## Phase 5: 错误处理与健壮性（P5）

### 5.1 统一 Tauri invoke 错误处理 — 消除静默吞错

**问题：** 多个组件中 `invoke` 调用的错误仅 `console.error`，用户无任何反馈。

**受影响位置：**

| 文件 | 行号 | 问题 |
|------|------|------|
| `ApprovalBlockUI.tsx` | 32 | approve_tool 失败仅 console.error，无状态回滚 |
| `ModeSelector.tsx` | 47 | get_mode 失败静默回退 |
| `Sidebar.tsx` | 83 | open_in_explorer `catch {}` 空体 |
| `SkillsTab.tsx` | 13-18 | refresh 无 try/catch，失败后按钮永久禁用 |
| `SkillDrafts.tsx` | 48-55 | loadDrafts 仅 console.error |
| `WorkflowRunView.tsx` | 44,62,70 | 所有错误仅 console.error |

**目标状态：** 所有 invoke 错误显示用户可见的反馈。

**具体方案：**

创建全局 toast/error 通知机制：
```tsx
// features/ui/uiSlice.ts
const uiSlice = createSlice({
  name: 'ui',
  initialState: { errors: [] as { id: string; message: string }[] },
  reducers: {
    showError: (state, action: PayloadAction<string>) => {
      state.errors.push({ id: crypto.randomUUID(), message: action.payload });
    },
    dismissError: (state, action: PayloadAction<string>) => {
      state.errors = state.errors.filter(e => e.id !== action.payload);
    },
  },
});
```

在组件中使用：
```tsx
// ApprovalBlockUI.tsx
try {
  await invoke('approve_tool', { promptId, choice });
} catch (e) {
  dispatch(showError(`Failed to approve: ${e}`));
  // 回滚乐观更新
  dispatch(toolApprovalResponded({ promptId, approved: false }));
}
```

```tsx
// SkillsTab.tsx
const handleRefresh = async () => {
  setRefreshing(true);
  try {
    await invalidate();
    await refresh();
  } catch (e) {
    dispatch(showError(`Failed to refresh skills: ${e}`));
  } finally {
    setRefreshing(false);
  }
};
```

**影响范围：** 上述 6 个文件 + 新建 `features/ui/uiSlice.ts` + Toast 组件
**工作量：** M
**验收标准：** 任何 invoke 失败时用户能看到错误提示（非控制台）

---

### 5.2 修复 `handleRun` 未处理 promise + 过时闭包

**问题：** `WorkflowEditor.tsx:160-167` 的 `handleRun` 中 `await handleSave()` 无 try/catch，如果 save thunk reject，`runWorkflow` 被静默跳过。且 `activeWorkflow` 在闭包创建时捕获，await 后可能已过时。

**当前代码：**
```tsx
const handleRun = async () => {
  if (!activeWorkflow || dirty) {
    await handleSave();  // 未处理 reject
  }
  if (activeWorkflow) {  // await 后可能过时
    dispatch(runWorkflow({ workflowId: activeWorkflow.id, input: { task: "Run workflow" } }));
  }
};
```

**目标状态：** try/catch + 在 await 前捕获 ID。

**具体方案：**
```tsx
const handleRun = async () => {
  const workflowId = activeWorkflow?.id;
  if (!workflowId) return;

  if (dirty) {
    try {
      await handleSave();
    } catch (e) {
      dispatch(showError(`Failed to save before run: ${e}`));
      return;
    }
  }

  // 验证 workflow 仍存在
  const current = useAppSelector.getState().workflow.workflows.find(w => w.id === workflowId);
  if (!current) {
    dispatch(showError('Workflow no longer exists'));
    return;
  }

  dispatch(runWorkflow({ workflowId, input: { task: "Run workflow" } }));
};
```

**影响范围：** `components/workflow/WorkflowEditor.tsx`
**工作量：** S
**验收标准：** save 失败时用户看到错误而非静默跳过

---

### 5.3 修复 `WorkflowRunView` 轮询 interval 竞态

**问题：** `WorkflowRunView.tsx:54-75` 的 `useEffect` 有两条代码路径：非轮询路径（无 cleanup）和轮询路径（返回 cleanup）。如果 `running` 从 true → false → true 快速切换，interval 可能未被正确清理。且初始 `invoke` 的 `.then(setNodeResults)` 在组件卸载后仍可能执行。

**目标状态：** 使用 `isMounted` 标志或 `AbortController` 防止卸载后 setState。

**具体方案：**
```tsx
useEffect(() => {
  if (!selectedRunId) { setNodeResults([]); return; }

  let isMounted = true;
  setLoadingResults(true);

  const fetchResults = async () => {
    try {
      const results = await invoke<WorkflowRunNodeResult[]>("get_workflow_run_results", { runId: selectedRunId });
      if (isMounted) setNodeResults(results);
    } catch (e) {
      if (isMounted) console.error(e);
    } finally {
      if (isMounted) setLoadingResults(false);
    }
  };

  fetchResults();

  if (running) {
    const interval = setInterval(fetchResults, POLL_INTERVAL_MS);
    return () => {
      isMounted = false;
      clearInterval(interval);
    };
  }

  return () => { isMounted = false; };
}, [selectedRunId, running]);
```

**影响范围：** `components/workflow/WorkflowRunView.tsx`
**工作量：** S
**验收标准：** 组件卸载后无 setState 警告；running 状态快速切换时无重复 interval

---

### 5.4 修复 `DocumentTab` 递归 `tryReadPaths` 未 await — 竞态条件

**问题：** `DocumentTab.tsx:29-51` 的 `tryReadPaths` 在 catch 块中调用 `tryReadPaths(index + 1)` 但未 `await`，创建未等待的 promise 链。多个并行读取可能竞态，`setContent` 的调用顺序不保证与路径顺序一致。

**当前代码：**
```tsx
} catch (err) {
  tryReadPaths(index + 1);  // ← not awaited
}
```

**目标状态：** 使用 `await` 或重构为 `for...of` 循环。

**具体方案：**
```tsx
const tryReadPaths = async (index: number) => {
  for (let i = index; i < relativePaths.length; i++) {
    const fullPath = relativePaths[i];
    try {
      const content = await invoke<string>('read_file', { path: fullPath });
      if (isMounted) {
        setContent(content);
        setLoading(false);
      }
      return; // 成功读取，停止尝试
    } catch (err) {
      continue; // 尝试下一个路径
    }
  }
  // 所有路径都失败
  if (isMounted) {
    setContent(null);
    setLoading(false);
  }
};
```

**影响范围：** `components/review/DocumentTab.tsx`
**工作量：** S
**验收标准：** 多路径文件读取按顺序尝试；正确内容（第一个存在的路径）被加载

---

## 评级矩阵

| 维度 | 当前评级 | 目标评级 | 关键改进 |
|------|---------|---------|---------|
| 正确性 | B | A++ | P0: setState 反模式, setTimeout 清理, CSS 类名, 数据丢失 |
| React 模式 | B+ | A+ | P1: memoization, 防抖, 死代码清理 |
| 类型安全 | B- | A+ | P2: 消除 any, 消除 ts-ignore |
| 无障碍 | C+ | A | P3: 语义化 HTML, ARIA, 焦点管理 |
| 架构 | B+ | A++ | P4: 组件提取, 逻辑复用, 常量提取 |
| 错误处理 | B | A+ | P5: 用户可见错误, 竞态修复 |
| **综合** | **B+** | **A++** | |

---

## 实施优先级建议

1. **立即修复（P0）**：6 项 — 总工作量 ~2S+4S = 6S
2. **本季度修复（P1）**：7 项 — 总工作量 ~5S+2M = 7S+2M
3. **下季度完成（P2-P3）**：10 项 — 总工作量 ~3S+2M+1L = 3S+2M+1L
4. **持续优化（P4-P5）**：9 项 — 总工作量 ~4S+2M = 4S+2M

**总工作量估算：** ~20S + 6M + 1L

---

*Generated by AI Agent (agent_core)*
*Model: gpt-4o | Timestamp: 2026-07-05T21:05:00+08:00*
