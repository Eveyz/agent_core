I'll read all the files in the chat and layout directories. Let me start by reading them in parallel batches.
Let me find any remaining files and the layout directory files.
I've now read all 32 files. Here is my comprehensive code quality report.

---

# A++ Code Quality Review Report

## Summary

**Files reviewed:** 24 chat files + 8 layout files = 32 files total
**Total issues found:** 43

| Severity | Count |
|----------|-------|
| P0 (critical) | 4 |
| P1 (high) | 12 |
| P2 (medium) | 16 |
| P3 (low) | 11 |

---

## P0 — Critical Issues

### [severity: P0] setState during render — React anti-pattern
**File:** app/src/components/chat/EditFileWidget.tsx:21-24
**Problem:** Calling `setPrevActive` and `setCollapsed` directly in the render body violates React's rules. This is the "derived state from props" anti-pattern. It causes an extra render cycle on every `active` prop change and can cause issues with concurrent rendering. React may warn or behave unexpectedly.
**Current code:**
```tsx
const [prevActive, setPrevActive] = useState(active);

if (active !== prevActive) {
  setPrevActive(active);
  setCollapsed(!active);
}
```
**Recommendation:** Use `useEffect` to sync state when the prop changes, or better yet, use a `key` prop to remount the component when `active` changes:
```tsx
useEffect(() => {
  setCollapsed(!active);
}, [active]);
```
Or use the `key` prop approach: `<EditFileWidget key={call_id} ... />` and initialize state from props once.

---

### [severity: P0] setState during render — React anti-pattern
**File:** app/src/components/chat/ToolBlockUI.tsx:122-127
**Problem:** Same anti-pattern as EditFileWidget. Calling `setPrevActive` and `setCollapsed` during render triggers an immediate re-render. This is explicitly called out in React docs as incorrect.
**Current code:**
```tsx
const [prevActive, setPrevActive] = useState(active);
// ...
if (active !== prevActive) {
  setPrevActive(active);
  if (!active && is_error) {
    setCollapsed(false);
  }
}
```
**Recommendation:** Move to `useEffect`:
```tsx
useEffect(() => {
  if (!active && is_error) {
    setCollapsed(false);
  }
}, [active, is_error]);
```

---

### [severity: P0] setTimeout without cleanup — memory leak risk
**File:** app/src/components/chat/BashWidget.tsx:56
**Problem:** `setTimeout(() => setCopied(false), 2000)` is not cleaned up. If the component unmounts before 2 seconds, this will attempt to set state on an unmounted component. While React 18 doesn't warn about this anymore, it's still a leak.
**Current code:**
```tsx
const handleCopy = async (e: React.MouseEvent) => {
  // ...
  setCopied(true);
  setTimeout(() => setCopied(false), 2000);
};
```
**Recommendation:** Store the timeout in a ref and clear it on unmount, or use a custom `useTimeout` hook.

---

### [severity: P0] setTimeout without cleanup — memory leak risk
**File:** app/src/components/chat/CodeBlock.tsx:200
**Problem:** Same issue as BashWidget. The copy reset timeout is not cleaned up on unmount.
**Current code:**
```tsx
const handleCopy = async () => {
  // ...
  setCopied(true);
  setTimeout(() => setCopied(false), 2000);
};
```
**Recommendation:** Use a ref to track the timeout and clear it in the effect cleanup or component unmount.

---

## P1 — High Priority Issues

### [severity: P1] Inline `<style>` tag rendered in every ToolBlockUI instance
**File:** app/src/components/chat/ToolBlockUI.tsx:217-238
**Problem:** A `<style>` block with global CSS rules is rendered inside every `ToolBlockUI` component instance. If there are 50 tool blocks in a session, this creates 50 identical `<style>` elements in the DOM. This is both a performance issue (DOM bloat) and a correctness issue (global CSS duplicated).
**Current code:**
```tsx
<div className="step-block">
  <style>{`
    .search-tool-result { font-size: 13px !important; }
    .search-tool-result h1, ... { ... }
    .scrollable-markdown { max-height: 400px; overflow-y: auto; }
    .tool-result-content { ... }
  `}</style>
```
**Recommendation:** Move these styles to a CSS file or extract to a single `<style>` tag at the app root.

---

### [severity: P1] Non-memoized object breaks child memoization
**File:** app/src/components/chat/SubagentDetailPage.tsx:19-25
**Problem:** `syntheticEntry` is a new object literal created on every render. It's passed to `AgentTurnUI` which is wrapped in `memo()`. Since the object reference changes every render, the memoization is completely defeated — `AgentTurnUI` (and all its children including markdown parsing, code highlighting, etc.) re-renders every time the parent does.
**Current code:**
```tsx
const syntheticEntry = {
  id: `subagent-detail-${subagent.id}`,
  type: 'turn' as const,
  blocks: convertSubagentBlocks(subagent.blocks),
  startTime: subagent.startTime,
  endTime: subagent.endTime,
};
```
**Recommendation:** Wrap in `useMemo`:
```tsx
const syntheticEntry = useMemo(() => ({
  id: `subagent-detail-${subagent.id}`,
  type: 'turn' as const,
  blocks: convertSubagentBlocks(subagent.blocks),
  startTime: subagent.startTime,
  endTime: subagent.endTime,
}), [subagent.id, subagent.blocks, subagent.startTime, subagent.endTime]);
```

---

### [severity: P1] `any` types throughout file parsing
**File:** app/src/components/layout/FileTree.tsx:155,228,230,231
**Problem:** Multiple uses of `any` for directory listing results, defeating TypeScript's type safety. The invoke result shape is untyped.
**Current code:**
```tsx
const result: any[] = await invoke('list_directory', { path: node.path });
const visibleFiles = result.filter(item => !item.name.startsWith('.'));
```
And:
```tsx
.then((result: any) => {
  const visibleFiles = result.filter((item: any) => !item.name.startsWith('.'));
  setNodes(visibleFiles.map((item: any) => ({ ... })));
})
```
**Recommendation:** Define a proper interface for the Tauri command result and type the invoke call:
```tsx
interface DirEntry { name: string; type: 'file' | 'dir'; size: string; }
const result = await invoke<DirEntry[]>('list_directory', { path: node.path });
```

---

### [severity: P1] `any` type in status parsing
**File:** app/src/components/chat/ToolBlockUI.tsx:58
**Problem:** `match[3] as any` casts a regex capture group to `any`, bypassing type checking. The status could be any string, not just the valid union type.
**Current code:**
```tsx
items.push({
  id: match[2],
  status: match[3] as any,
  description: match[4],
});
```
**Recommendation:** Validate and narrow the type:
```tsx
const status = match[3] as string;
if (['pending', 'in_progress', 'completed', 'blocked'].includes(status)) {
  items.push({ id: match[2], status, description: match[4] });
}
```

---

### [severity: P1] Dead/non-functional button with no handler
**File:** app/src/components/chat/ChatInput.tsx:293
**Problem:** A button with a PlusIcon has no `onClick` handler and no `aria-label`. It appears to be dead code or an unfinished feature, and it's not accessible.
**Current code:**
```tsx
<button className="icon-btn"><PlusIcon size={16} /></button>
```
**Recommendation:** Either implement the intended functionality, remove it, or if it's a placeholder, add `disabled` and `aria-label` with a title.

---

### [severity: P1] Dead code — `isStreaming` prop declared but never used
**File:** app/src/components/chat/MarkdownContent.tsx:94
**Problem:** The `isStreaming` prop is declared in the component's type signature but is never destructured or used in the component body. Callers pass it (e.g., AgentTurn.tsx:278 `isStreaming={item.data.isStreaming}`) but it has no effect. This is misleading dead code.
**Current code:**
```tsx
export const MarkdownContent = memo(function MarkdownContent({
  content,
  className,
  plainText = false,
}: {
  content: string;
  className?: string;
  isStreaming?: boolean;  // ← declared but never destructured
  plainText?: boolean;
}) {
```
**Recommendation:** Either implement the streaming fast-path (the code at lines 190-195 appears to be intended for this but is unreachable since `segments` is always truthy) or remove the prop from the type signature and all call sites.

---

### [severity: P1] Unreachable dead code — streaming fast path
**File:** app/src/components/chat/MarkdownContent.tsx:169-195
**Problem:** The `if (segments)` check at line 169 is always true because `useMemo` at line 120 always returns an array. The fallback at lines 190-195 is unreachable dead code. This suggests the streaming optimization was intended but never properly implemented.
**Current code:**
```tsx
if (segments) {  // always true — segments is always an array
  return ( ... );
}
// This is never reached:
return (
  <div className={className} style={streamingStyle} onClick={handleClick}>
    {trimmedContent}
  </div>
);
```
**Recommendation:** Either implement the streaming path properly (check `isStreaming` before computing segments) or remove the dead code.

---

### [severity: P1] Stale timer display — `Date.now()` in render without timer
**File:** app/src/components/chat/SubagentWidgets.tsx:66-68
**Problem:** For working subagents, `Date.now() - subagent.startTime` is computed in the render body (inside `useMemo` with deps `[subagent, toolCount]`). Since there's no interval/timer updating the component, this value will be stale — it only updates when `subagent` or `toolCount` changes, not as time passes.
**Current code:**
```tsx
const statusText = useMemo(() => {
  if (!subagent) return '';
  if (subagent.status === 'working') {
    const elapsed = subagent.endTime
      ? formatTime(subagent.endTime - subagent.startTime)
      : formatTime(Date.now() - subagent.startTime);  // stale!
    return `Working · ${toolCount} tools · ${elapsed}`;
  }
  // ...
}, [subagent, toolCount]);
```
**Recommendation:** Add a timer interval (like `ProcessingTimer.tsx` does) that forces re-renders every second while the subagent is working.

---

### [severity: P1] Redundant identical ternary branches — dead code
**File:** app/src/components/chat/EditFileWidget.tsx:51
**Problem:** The color ternary has identical true and false branches: `active ? 'var(--text-muted)' : 'var(--text-muted)'`. This is dead logic.
**Current code:**
```tsx
color={is_error ? 'var(--danger)' : (active ? 'var(--text-muted)' : 'var(--text-muted)')}
```
**Recommendation:** Simplify to `color={is_error ? 'var(--danger)' : 'var(--text-muted)'}`.

---

### [severity: P1] Redundant ternary — both branches return 'Edited'
**File:** app/src/components/chat/EditFileWidget.tsx:43
**Problem:** `is_error ? 'Edit failed:' : summary ? 'Edited' : 'Edited'` — both branches of the inner ternary return the same string.
**Current code:**
```tsx
const labelPrefix = active ? 'Editing' : is_error ? 'Edit failed:' : summary ? 'Edited' : 'Edited';
```
**Recommendation:** Simplify to `const labelPrefix = active ? 'Editing' : is_error ? 'Edit failed:' : 'Edited';`

---

### [severity: P1] `@ts-ignore` for CSS properties instead of proper typing
**File:** app/src/components/layout/CustomTitleBar.tsx:25-28, 52-55
**Problem:** Four `@ts-ignore` comments suppress type errors for `WebkitAppRegion` and `appRegion` CSS properties. This bypasses TypeScript safety and the comments themselves are noisy.
**Current code:**
```tsx
// @ts-ignore - webkit-app-region is not in React's CSSProperties type
WebkitAppRegion: "drag",
// @ts-ignore
appRegion: "drag",
```
**Recommendation:** Create a type augmentation or use a typed cast:
```tsx
const dragStyle = {
  WebkitAppRegion: 'drag',
  appRegion: 'drag',
} as React.CSSProperties;
```
Or add a module augmentation for `React.CSSProperties`.

---

### [severity: P1] Direct DOM style manipulation in event handlers
**File:** app/src/components/layout/CustomTitleBar.tsx:57-61
**Problem:** `onMouseEnter` and `onMouseLeave` directly mutate `e.currentTarget.style.background`. This is an anti-pattern in React — it bypasses the virtual DOM and can cause style conflicts. It's also not accessible (no keyboard equivalent for hover state).
**Current code:**
```tsx
onMouseEnter={(e) => (e.currentTarget.style.background = "var(--overlay-0_1)")}
onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
```
**Recommendation:** Use React state (`useState` for `isHovered`) and apply styles conditionally, or better, use CSS `:hover` pseudo-class.

---

## P2 — Medium Priority Issues

### [severity: P2] Non-memoized `projects.find()` runs on every render
**File:** app/src/components/chat/ChatInput.tsx:69
**Problem:** `projects.find((p) => p.id === activeProjectId)` runs on every render. The `projects` array could be large and this is a linear scan.
**Current code:**
```tsx
const activeProject = projects.find((p) => p.id === activeProjectId);
```
**Recommendation:** Wrap in `useMemo`:
```tsx
const activeProject = useMemo(() => projects.find((p) => p.id === activeProjectId), [projects, activeProjectId]);
```

---

### [severity: P2] `handleClick` not memoized — recreated every render
**File:** app/src/components/chat/MarkdownContent.tsx:99-108
**Problem:** `handleClick` is defined as a plain function in the component body, recreated on every render. Since `MarkdownContent` is `memo`'d, this is less impactful, but the function is still recreated on every actual render.
**Current code:**
```tsx
const handleClick = (e: React.MouseEvent) => {
  const target = e.target as HTMLElement;
  // ...
};
```
**Recommendation:** Wrap in `useCallback` (with empty deps since it only uses `e`).

---

### [severity: P2] `handleCopy` not memoized
**File:** app/src/components/chat/ToolBlockUI.tsx:129-138
**Problem:** `handleCopy` is defined as a plain async function in the render body, recreated every render.
**Recommendation:** Wrap in `useCallback` with proper deps (`[result]`).

---

### [severity: P2] Index as key in list rendering
**File:** app/src/components/chat/SubagentWidgets.tsx:34
**Problem:** `key={idx}` uses array index as key. If subagent refs are reordered or inserted, React may incorrectly reuse DOM nodes.
**Current code:**
```tsx
{subagentRefs.map((refBlock, idx) => {
  return <SubagentCard key={idx} subagentId={refBlock.subagent_id} />;
})}
```
**Recommendation:** Use `refBlock.subagent_id` as the key: `key={refBlock.subagent_id}`.

---

### [severity: P2] `renderRegularTools` and `renderSubagentTools` not memoized
**File:** app/src/components/chat/TurnIterationUI.tsx:67,130
**Problem:** These render functions are defined as plain functions in the component body. They're called during render so this is acceptable, but they recreate closures on every render. Given the component is `memo`'d with a custom comparator, this is a minor concern.
**Recommendation:** Consider extracting these as separate memoized components for clarity and performance.

---

### [severity: P2] Unnecessary type casts after type guards
**File:** app/src/components/chat/turnHelpers.ts:44,56
**Problem:** `b as AssistantBlock` and `b as ThinkingBlock` are unnecessary because the preceding `if (b.type === 'assistant')` and `if (b.type === 'thinking')` checks already narrow the type via TypeScript's discriminated unions.
**Current code:**
```tsx
if (b.type === 'assistant') {
  pushCurrentIter();
  items.push({ type: 'assistant', data: b as AssistantBlock });
  return;
}
```
**Recommendation:** Remove the `as` casts — `b` is already narrowed to `AssistantBlock` by the type guard.

---

### [severity: P2] `console.error` as only error handling
**File:** app/src/components/chat/ApprovalBlockUI.tsx:32
**Problem:** When `invoke('approve_tool')` fails, the error is only logged to console. The user sees no feedback, and the Redux state has already been optimistically updated (line 28) without rollback.
**Current code:**
```tsx
} catch (e) {
  console.error('Failed to approve tool', e);
}
```
**Recommendation:** Show a user-facing error (toast/banner) and consider rolling back the Redux state update on failure.

---

### [severity: P2] Silent error swallowing
**File:** app/src/components/chat/ModeSelector.tsx:47
**Problem:** `.catch(() => setMode('build'))` silently swallows the error and falls back to 'build' mode. The user has no indication that the mode fetch failed.
**Current code:**
```tsx
invoke<string>('get_mode')
  .then((m) => setMode(m as AgentMode))
  .catch(() => setMode('build'));
```
**Recommendation:** Log the error at minimum: `.catch((e) => { console.error('Failed to get mode', e); setMode('build'); })`.

---

### [severity: P2] Silent error swallowing
**File:** app/src/components/layout/Sidebar.tsx:83
**Problem:** `catch {}` with empty body silently swallows the error from `open_in_explorer`.
**Current code:**
```tsx
try {
  await invoke("open_in_explorer", { path: projectPath });
} catch {}
```
**Recommendation:** At minimum log the error, or show user feedback.

---

### [severity: P2] Massive code duplication between projects and chat sections
**File:** app/src/components/layout/Sidebar.tsx:514-591 and 627-718
**Problem:** The session rendering logic for the "Projects" section (lines 514-591) and the "Chat" section (lines 627-718) is nearly identical — same maxInitial logic, same session row rendering, same expand/collapse toggle. ~80 lines duplicated.
**Recommendation:** Extract a `SessionList` component that `SessionList` component that takes `sessions`, takes `sessions`, `projectId`, `projectId`, and related and related handlers handlers as props.

---

### [severity: P2] Magic numbers for session limits
**File:** app/src/components/layout/Sidebar.tsx:515,639
**Problem:** `const maxInitial = 6;` appears twice as a magic number.
**Recommendation:** Extract to a module-level constant: `const MAX_INITIAL_SESSIONS = 6;`

---

### [severity: P2] Non-functional "Settings" link — dead TODO
**File:** app/src/components/chat/SkillSelector.tsx:216
**Problem:** A link with `onClick={(e) => { e.preventDefault(); /* TODO: open settings */ }}` does nothing. This is misleading to users who click it.
**Current code:**
```tsx
<a href="#settings" onClick={(e) => { e.preventDefault(); /* TODO: open settings */ }}>
  Install skills in Settings
</a>
```
**Recommendation:** Either wire up the settings navigation or remove the link and show plain text instead.

---

### [severity: P2] Complex nested ternary in JSX
**File:** app/src/components/chat/AgentTurn.tsx:281-316
**Problem:** The error rendering block is a deeply nested ternary (4 levels) making it very hard to read. The condition at line 282-285 does case-insensitive matching with three separate `includes()` calls with different casings.
**Current code:**
```tsx
item.data.text.includes("maximum number of steps") ||
item.data.text.includes("Reached the maximum number of steps") ||
item.data.text.includes("reached the maximum number of steps") ? ( ... )
```
**Recommendation:** Extract to a helper function `getErrorVariant(text)` that returns a type, and use a switch or lookup. Use `text.toLowerCase().includes(...)` for case-insensitive matching.

---

### [severity: P2] Typo in user-facing text
**File:** app/src/components/chat/AgentTurn.tsx:289
**Problem:** "You have been working in this project in a while." should be "You have been working in this project for a while."
**Recommendation:** Fix the typo.

---

### [severity: P2] Unsafe `any` in memory search results
**File:** app/src/components/chat/ToolBlockUI.tsx:301
**Problem:** `results.map((r: any, idx: number) =>` uses `any` for memory search results, accessing `.id`, `.role`, `.importance`, `.created_at`, `.content`, `.text`, `.message`, `.metadata` without any type safety.
**Recommendation:** Define an interface for memory search results.

---

### [severity: P2] IIFE pattern in JSX — repeated anti-pattern
**File:** app/src/components/chat/BashWidget.tsx:68, app/src/components/chat/EditFileWidget.tsx:51, app/src/components/chat/ReadFileWidget.tsx:37, app/src/components/chat/ToolBlockUI.tsx:245,294
**Problem:** Multiple files use `{(() => { ... })()}` IIFE pattern in JSX to compute icon components. This creates new closures on every render and hurts readability.
**Current code (BashWidget.tsx:68):**
```tsx
{(() => { const ToolIcon = getToolIcon(toolName); return <ToolIcon size={13} ... />; })()}
```
**Recommendation:** Extract to a variable before the return statement:
```tsx
const ToolIcon = getToolIcon(toolName);
// in JSX:
<ToolIcon size={13} className="step-icon" color={...} />
```

---

## P3 — Low Priority Issues

### [severity: P3] Accessibility: div with onClick, no keyboard support
**File:** Multiple files: AgentTurn.tsx:57,247; BashWidget.tsx:64; EditFileWidget.tsx:47; TodoPanel.tsx:37; TurnIterationUI.tsx:172,198; FileTree.tsx:175; Sidebar.tsx:406,412,418,424,727,730
**Problem:** Throughout the codebase, `<div>` elements with `onClick` handlers are used for interactive controls (collapse/expand, navigation, toggles). These are not keyboard accessible — users can't focus or activate them with Tab/Enter. No `role="button"`, no `tabIndex`, no `onKeyDown`.
**Recommendation:** Use `<button>` for interactive elements, or add `role="button"`, `tabIndex={0}`, and `onKeyDown` handlers for keyboard support.

---

### [severity: P3] Accessibility: spans used as buttons
**File:** app/src/components/chat/UserRow.tsx:62,65,79,84,88
**Problem:** `<span style={cursorPointer} onClick={confirmEdit}>` and similar — clickable spans used where buttons should be. Not keyboard accessible, no focus styles, not announced as interactive by screen readers.
**Recommendation:** Replace with `<button>` elements.

---

### [severity: P3] Accessibility: autocomplete dropdown lacks ARIA
**File:** app/src/components/chat/ChatInput.tsx:235-266
**Problem:** The autocomplete dropdown has no `role="listbox"`, items have no `role="option"`, no `aria-selected`, no `aria-activedescendant` for the selected item. Screen readers won't announce the dropdown.
**Recommendation:** Add appropriate ARIA roles and attributes, following the combobox pattern.

---

### [severity: P3] Inline style objects create new references each render
**File:** app/src/components/layout/ChatArea.tsx:40-46,63-68,80-86
**Problem:** Multiple inline style objects are created in the render body. While not a performance bottleneck for small lists, these create new object references each render.
**Recommendation:** For repeated styles (like the learn/btw entry styling), extract to constants or CSS classes.

---

### [severity: P3] Redundant variable alias
**File:** app/src/components/chat/ModelSelector.tsx:91
**Problem:** `const config = configFromStore;` creates a redundant alias that's only used once (line 93). This adds confusion without value.
**Recommendation:** Use `configFromStore` directly.

---

### [severity: P3] `handleKeyDown` references `handleSelect` before definition
**File:** app/src/components/chat/SkillSelector.tsx:131,154
**Problem:** `handleKeyDown` (line 120) references `handleSelect` (line 154) which is defined later. This works due to closure hoisting of `const` in the same scope, but is a code smell that hurts readability.
**Recommendation:** Reorder so `handleSelect` is defined before `handleKeyDown`, or move `handleSelect` into the `useCallback` deps.

---

### [severity: P3] Non-null assertion operator
**File:** app/src/components/chat/ToolBlockUI.tsx:324
**Problem:** `parseTodoResult(result)!` uses a non-null assertion. The function can return `null`, and the `!` suppresses the check. The preceding conditional (`name.startsWith('todo_') && parseTodoResult(result) ?`) guarantees non-null here, but the assertion is fragile if refactored.
**Recommendation:** Assign to a variable: `const parsed = parseTodoResult(result); if (parsed) { ... }`

---

### [severity: P3] Exported functions appear unused — dead code
**File:** app/src/components/chat/MarkdownContent.tsx:41,59
**Problem:** `extractCodeBlocks` and `replaceCodeBlocksWithPlaceholders` are exported but appear to have no consumers in the codebase. They're separate from the actual rendering logic which uses inline regex.
**Recommendation:** Verify usage and remove if dead, or consolidate with the inline parsing logic.

---

### [severity: P3] `slice().reverse().find()` — inefficient copy
**File:** app/src/components/chat/turnHelpers.ts:65
**Problem:** `items.slice().reverse().find(...)` creates a copy of the array and reverses it just to find the last matching element.
**Current code:**
```tsx
const lastIter = items.slice().reverse().find(i => i.type === 'iteration');
```
**Recommendation:** Use a backward loop or `findLast` (ES2023):
```tsx
let lastIter: TurnRenderItem | undefined;
for (let i = items.length - 1; i >= 0; i--) {
  if (items[i].type === 'iteration') { lastIter = items[i]; break; }
}
```

---

### [severity: P3] `handleItemClick` and `handleToggle` not memoized in FileTree
**File:** app/src/components/layout/FileTree.tsx:134,217
**Problem:** `handleToggle` (line 134) and `handleItemClick` (line 217) are not wrapped in `useCallback`. `FileTreeItem` is not memoized, so this causes all items to re-render when the parent re-renders.
**Recommendation:** Memoize the handlers and wrap `FileTreeItem` in `memo`.

---

### [severity: P3] Long line with many OR conditions
**File:** app/src/components/chat/TurnIterationUI.tsx:64
**Problem:** A single line with 13 `||` conditions checking tool names is hard to read and maintain.
**Current code:**
```tsx
return name === 'edit' || name === 'read_file' || name === 'bash' || name === 'grep_search' || name === 'glob_search' || name === 'grep' || name === 'glob' || name.startsWith('todo_') || name === 'write_file' || name === 'write_to_file' || name.startsWith('skill_') || name === 'archival_memory_search' || name === 'conversation_search';
```
**Recommendation:** Use a Set for exact matches combined with prefix checks:
```tsx
const SINGLE_TOOL_NAMES = new Set(['edit', 'read_file', 'bash', 'grep_search', 'glob_search', 'grep', 'glob', 'write_file', 'write_to_file', 'archival_memory_search', 'conversation_search']);
return SINGLE_TOOL_NAMES.has(name) || name.startsWith('todo_') || name.startsWith('skill_');
```

---

This concludes the review. The most impactful fixes would be the P0 issues (setState during render, setTimeout cleanup), followed by the P1 issues (inline `<style>` duplication, broken memoization in SubagentDetailPage, dead `isStreaming` prop, and the `any` types in FileTree).
