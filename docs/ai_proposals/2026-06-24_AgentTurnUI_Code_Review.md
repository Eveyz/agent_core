# 2026-06-24 AgentTurnUI 代码审查

## 审查概述

审查范围：`app/src/components/AgentTurn.tsx`，单文件约 **750 行**，包含 10+ 个组件/辅助函数（`ProcessingTimer`、`TurnIterationUI`、`ToolBlockUI`、`ApprovalBlockUI`、`EditFileWidget`、`SubagentSpawnWidget`、`SubagentCard`、`TurnFooter`、`AgentTurnUI`、`groupBlocksIntoItems`、`parseUnifiedDiff`、`parseEditSummary`、`getToolIcon`）。

技术栈：React 19 + TypeScript 5.8 + Redux Toolkit + Tauri IPC + lucide-react。

总体评价：**功能完整，diff 渲染和状态折叠交互设计合理；但存在多个 useEffect 覆盖用户意图、Redux 全量订阅导致 O(n²) 重渲染、无障碍缺失、文件过大等问题，需要优先修复。**

---

## P0 — Bug 与安全

### P0-1. useEffect 覆盖用户折叠意图（多处）

`TurnIterationUI`、`ToolBlockUI`、`EditFileWidget`、`AgentTurnUI` 均使用 `useEffect` 强制重置 collapse 状态：

```tsx
// TurnIterationUI 第 296-304 行
useEffect(() => {
  if (!isStreaming && !iteration.isLast) {
    setThoughtCollapsed(true);
    setToolsCollapsed(true);
  } else {
    setThoughtCollapsed(false);
    setToolsCollapsed(false);
  }
}, [isStreaming, iteration.isLast]);
```

**问题：** 用户手动展开/折叠后，如果 streaming 结束或 `isLast` 变化，组件强行重置状态，用户操作被覆盖。例如：用户想保留旧 iteration 的展开以对比内容，streaming 结束后却被自动折叠。

**修法**：引入初始化标志，仅在 iteration identity 变化时重置，或干脆移除自动折叠逻辑，由用户完全控制。

```tsx
const [initialized, setInitialized] = useState(false);
useEffect(() => {
  if (!initialized) {
    setThoughtCollapsed(!isStreaming && !iteration.isLast);
    setToolsCollapsed(!isStreaming && !iteration.isLast);
    setInitialized(true);
  }
}, [iteration.id]); // 只在 iteration 切换时触发
```

### P0-2. SubagentCard 的 elapsed time 是静态快照

```tsx
// SubagentCard 内
const statusText = useMemo(() => {
  if (subagent.status === 'working') {
    const elapsed = formatTime(Date.now() - subagent.startTime); // 只计算一次！
    return `Processing${' .'.repeat((dotCount % 3) + 1)} (${elapsed})`;
  }
  // ...
}, [subagent, toolCount]);
```

**问题：** `Date.now()` 被捕获在 `useMemo` 中，子 agent 运行中的时间不会更新，除非父组件因其他原因重新渲染。用户看到的 elapsed time 是一个静态快照。

**修法**：复用已有的 `ProcessingTimer` 组件，或内联 interval 逻辑。

```tsx
{subagent.status === 'working' && (
  <ProcessingTimer startTime={subagent.startTime} isComplete={false} />
)}
```

### P0-3. Redux 全量 `subagents` 订阅导致 O(n²) 重渲染

```tsx
// AgentTurnUI 内
const subagents = useSelector((state: RootState) => state.chat.subagents);
```

**问题：** 每个 `AgentTurnUI` 实例都订阅整个 `subagents` 对象。任何一个子 agent 的状态更新都会触发**所有已挂载轮次**的重渲染。如果聊天历史有 50 轮，每轮都 mount 一个 `AgentTurnUI`，则每次 subagent 更新触发 50 次重渲染。

**修法**：从父组件传入所需的 subagent 数据，或使用 `shallowEqual` 比较器，或只 select 当前 turn 需要的 subagent IDs。

```tsx
const subagentIds = useMemo(() =>
  iteration.subagentTools.map((t) => t.subagent_id),
  [iteration.subagentTools]
);
const subagents = useSelector(
  (state: RootState) =>
    subagentIds.map((id) => state.chat.subagents[id]),
  shallowEqual
);
```

### P0-4. 可交互元素无按钮语义（无障碍）

以下元素可点击但使用 `<div>`：

- `.turn-header`（折叠整轮）
- `.step-row`（折叠 thinking/tools）
- `.step-row`（折叠单个 tool）
- "Show More" 按钮

**问题：** 键盘用户无法 Tab 聚焦，屏幕阅读器无法识别为可交互元素。

**修法**：改用 `<button>` 或添加 `role="button" tabIndex={0} onKeyDown`。

```tsx
<div
  className="turn-header"
  role="button"
  tabIndex={0}
  onClick={toggle}
  onKeyDown={(e) => e.key === 'Enter' && toggle()}
>
```

### P0-5. EditFileWidget 双类型断言无运行时验证

```tsx
const filePath = (args as Record<string, unknown> | undefined)?.file_path as string | undefined;
```

**问题：** `args` 是 `unknown`，两次 `as` 没有任何运行时验证。`file_path` 可能是 `number`、`null` 或任意类型，后续 `filePath?.split('/')` 会抛 TypeError。

**修法**：使用类型守卫：

```tsx
function getStringProp(obj: unknown, key: string): string | undefined {
  if (typeof obj !== 'object' || obj === null) return undefined;
  const val = (obj as Record<string, unknown>)[key];
  return typeof val === 'string' ? val : undefined;
}
```

---

## P1 — 性能

### P1-1. `renderRegularTools`/`renderSubagentTools` 是内联函数

```tsx
const renderRegularTools = () => (<>...</>);
```

每次 `TurnIterationUI` 渲染都重新创建函数，虽然立即调用，但会创建新的 JSX 闭包和局部变量捕获，不利于 V8 优化，也使子组件的 memo 效果打折扣。

**修法**：提取为独立组件：

```tsx
const RegularToolsList = memo(function RegularToolsList({
  tools, toolCount, isLast, iteration, isStreaming
}: RegularToolsListProps) {
  return <>...</>;
});
```

### P1-2. 工具图标 IIFE 模式

```tsx
{(() => { const ToolIcon = getToolIcon(name); return <ToolIcon ... />; })()}
```

每次都创建 IIFE，应提取为变量或独立 `ToolIcon` 组件。

**修法**：

```tsx
const ToolIconComponent = memo(function ToolIconComponent({ name }: { name: string }) {
  const Icon = getToolIcon(name);
  return <Icon size={12} />;
});
```

### P1-3. 内联 style 对象破坏 memo

`AgentTurn.tsx` 全文有 20+ 处内联 `style` 对象：

```tsx
<div style={{ marginLeft: '6px', paddingLeft: '12px', borderLeft: '1px solid var(--text-muted)', ... }}>
```

每次渲染创建新对象 → `React.memo` 的浅比较判定 props 变化 → 子组件无谓重渲染。

**修法**：提取为模块级 `const` 常量：

```tsx
const ITERATION_BODY_STYLE: CSSProperties = {
  marginLeft: '6px',
  paddingLeft: '12px',
  borderLeft: '1px solid var(--text-muted)',
  // ...
};
```

### P1-4. `hasIntermediateSteps` 计数不完整

```tsx
const hasIntermediateSteps = toolCount > 0 || thoughtCount > 0;
```

仅统计非 subagent 的 tool 和 thinking。如果一轮只有 subagent spawn 或 error，`turn-header` 不渲染但内容仍然出现，布局不一致，且导致 header 条件渲染与内容渲染脱节。

**修法**：

```tsx
const hasIntermediateSteps = toolCount > 0 || thoughtCount > 0 ||
  iteration.subagentTools.length > 0 || iteration.approvalBlocks.length > 0;
```

---

## P2 — 架构与代码质量

### P2-1. 文件过大（750 行）需拆分

| 组件/函数 | 行数 | 职责 |
|-----------|------|------|
| `TurnIterationUI` | ~180 | 单轮迭代渲染 |
| `ToolBlockUI` | ~120 | 工具调用详情 |
| `EditFileWidget` + diff utils | ~140 | diff 解析与渲染 |
| `ApprovalBlockUI` | ~100 | 权限审批 UI |
| `SubagentSpawnWidget` + `SubagentCard` | ~140 | 子 agent 渲染 |
| `AgentTurnUI` + `groupBlocksIntoItems` | ~120 | 入口与 block 分组 |

**修法**：

```
AgentTurn/
  index.tsx              # AgentTurnUI + groupBlocksIntoItems
  ProcessingTimer.tsx
  TurnIterationUI.tsx
  ToolBlockUI.tsx
  ApprovalBlockUI.tsx
  EditFileWidget.tsx
  SubagentWidgets.tsx    # SubagentSpawnWidget + SubagentCard
  TurnFooter.tsx
  utils.ts               # parseUnifiedDiff, parseEditSummary, getToolIcon
```

### P2-2. `parseUnifiedDiff` 正则重复匹配

```ts
oldLine = parseInt(m[1] ? line.match(/@@ -(\d+)/)![1] : '0', 10);
```

`m` 已经通过 `/^@@ -(\d+)(?:,\d+)? \+(\d+)/` 捕获了数据，又跑一次 `match`。

**修法**：直接取 `m[1]` 和 `m[2]`，无需二次 match。

### P2-3. 不必要的 `as` 断言（3+ 处）

| 位置 | 问题 |
|------|------|
| `groupBlocksIntoItems` 中 `b as AssistantBlock` | 已被 `b.type === 'assistant'` 窄化，无需 `as` |
| `b as ThinkingBlock` | 同上 |
| `approvalStatus` 的 `as 'approved' \| 'denied' \| undefined` | 依赖一个"说谎"的 type predicate，应显式过滤 |

**修法**：使用 TypeScript 类型守卫，移除所有不必要的 `as`。

### P2-4. CSS hover 用 JS 实现

```tsx
onMouseEnter={(e) => e.currentTarget.style.background = 'var(--overlay-0_08)'}
onMouseLeave={(e) => e.currentTarget.style.background = 'var(--overlay-0_04)'}
```

**问题：** 每次事件都触发 React 合成事件 → 状态更新 → 重渲染。且鼠标快速划过时可能有闪烁。

**修法**：改用 CSS `:hover`。

```css
.step-row:hover {
  background: var(--overlay-0_08);
}
```

### P2-5. 保存逻辑与流式状态耦合

`TurnFooter` 的 `copyOutput` 和 `rerunFromHere` 直接访问 DOM 和 dispatch，但 footer 本身没有接收 `entry` 以外的上下文。这种设计使得 footer 的测试和复用困难。

**修法**：通过回调 props 解耦，让父组件 `AgentTurnUI` 提供 `onCopy` 和 `onRerun` handler。

---

## P3 — 风格与可维护性

### P3-1. DRY 违规

- Chevron toggle 重复 5+ 次 → 提取 `<CollapsibleChevron />`
- `className={\`step-row ${active ? 'step-row-active' : ''} ${is_error ? 'step-row-error' : ''}\`}` 重复 → 使用 `clsx` 或辅助函数
- `marginTop: hasThinkingContent ? '4px' : '0'` 重复 3 次

**修法**：引入 `clsx` 库，提取共享组件。

```tsx
import clsx from 'clsx';

<div className={clsx('step-row', active && 'step-row-active', is_error && 'step-row-error')}>
```

### P3-2. 硬编码魔法值

- `500`（截断阈值）
- `250`（计时器 interval）
- `1500`（复制成功反馈时长）
- `'#ef4444'`、`'#f87171'`、`'#888'`（应统一使用 CSS 变量）

**修法**：提取为命名常量或 CSS 变量。

### P3-3. `||` vs `??` 用于 key fallback

```tsx
key={b.call_id || idx}
```

`call_id = ""` 时会 fallback 到 `idx`，可能导致 key 不稳定。

**修法**：`key={b.call_id ?? idx}`

### P3-4. snake_case vs camelCase 混用

props 使用 snake_case（`is_error`、`is_last`、`call_id` —— 后端 Rust 命名），但 React 组件中应使用 camelCase。

**修法**：在数据边界（Redux slice 或 selector）做规范化转换，组件内统一 camelCase。

### P3-5. 空 error 文本无兜底

```tsx
<span>{item.data.text}</span>
```

如果 `text` 为空字符串，用户只看到带图标的空框。

**修法**：`{item.data.text || 'Unknown error'}`

### P3-6. `rawOutput` 复制可能含 `undefined`

```tsx
.map((b) => b.text)
.join('\n')
```

如果 `b.text` 为 `undefined`，会复制出 `"undefined"` 字符串。

**修法**：`.map((b) => b.text ?? '').join('\n')`

---

## 评分

| 维度 | 评价 |
|---|---|
| 组件设计 | 良好，diff 渲染和状态折叠交互设计合理 |
| 状态管理 | 中等，Redux 全量订阅 + useEffect 覆盖用户意图 |
| 性能优化 | 中等偏下，多处内联 style 破坏 memo + IIFE 模式 |
| 类型安全 | 中等，多处 `as` 断言 + `unknown` 未做类型守卫 |
| 无障碍 | **差**，可点击 div 无按钮语义 |
| 代码组织 | 中等偏下，750 行单文件需拆分 |
| 可维护性 | 中等，DRY 违规和硬编码值较多 |

---

> [!NOTE]
> **优先修复顺序建议**
> 1. P0-1: 移除 useEffect 覆盖用户 collapse 状态的行为
> 2. P0-2: SubagentCard 使用 live timer 更新 elapsed time
> 3. P0-3: 窄化 Redux selector，避免全量 subagents 订阅
> 4. P0-4: 可点击 div 改为 button 或添加 a11y 属性
> 5. P0-5: 移除不必要的 `as` 断言，添加类型守卫
> 6. P2-1: 拆分 AgentTurn.tsx（750 行 → 多文件）
> 7. P1-1 + P1-2: 提取内联函数和 IIFE 为独立组件
> 8. P3-1: 引入 clsx，提取共享组件（CollapsibleChevron、ToolIcon 等）
