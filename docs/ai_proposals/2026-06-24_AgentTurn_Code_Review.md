# 2026-06-24 Code Review: AgentTurn.tsx

## Overview
This document provides a comprehensive code review of `app/src/components/chat/AgentTurn.tsx`. The file is responsible for rendering an AI agent's turn, including thinking blocks, tool calls, edit file diffs, subagent spawns, and markdown outputs.

## 1. Strengths
- **Aggressive Memoization**: There is excellent use of `React.memo`, `useMemo`, and `useCallback` throughout the file. Custom comparison functions (like the one in `TurnIterationUI`) effectively prevent unnecessary re-renders of large chat blocks.
- **Complex State Mapping**: The `groupBlocksIntoItems` function intelligently collapses a linear array of blocks (`TurnBlock[]`) into nested semantic structures (`TurnIteration`), drastically simplifying the rendering logic.
- **Diff Rendering**: The custom unified diff parser (`parseUnifiedDiff`) and `EditFileWidget` component provide a very clean side-by-side or inline view of file changes directly in the chat UI.
- **Performance Awareness**: Using `MarkdownContent` with the plaintext streaming fast-path correctly avoids O(N²) parsing slowdowns during text streaming.

## 2. Critical Risks & Performance Issues

### A. Global State Subscription Anti-Pattern
```typescript
export const AgentTurnUI = memo(function AgentTurnUI({ entry }: { entry: ChatEntry }) {
  const subagents = useSelector((state: RootState) => state.chat.subagents);
  // ...
```
**Issue**: `AgentTurnUI` subscribes to the entire `state.chat.subagents` dictionary. This means that **every time ANY subagent's state updates**, ALL `AgentTurnUI` instances in the entire chat history will re-render, completely defeating the `React.memo` optimization.
**Recommendation**: Remove the `subagents` subscription from `AgentTurnUI` and `TurnIterationUI`. Instead, only components that actually render a specific subagent (e.g., `SubagentCard`) should subscribe to that specific subagent's ID:
```typescript
const subagent = useSelector((state: RootState) => state.chat.subagents[subagentId]);
```

### B. Inefficient `useEffect` State Syncing
```typescript
  const [collapsed, setCollapsed] = useState(!active);
  useEffect(() => {
    if (active === false) setCollapsed(true);
    else if (active === true) setCollapsed(false);
  }, [active]);
```
**Issue**: Syncing props to state using `useEffect` causes a duplicate render pass. When `active` changes, React renders the component once, fires the effect, updates the state, and forces a second render.
**Recommendation**: Use a derived state pattern or an explicit `key` if the component should completely reset when `active` changes. Alternatively, since the user can manually toggle `collapsed`, keep track of user overrides instead of unconditionally syncing.

## 3. Architecture & Code Quality

### A. Unnecessary Re-exports
```typescript
import { MarkdownContent, formatTime, parseMarkdown } from './MarkdownContent';
export { formatTime, parseMarkdown };
```
**Issue**: Re-exporting these utility functions from a UI component file creates tight coupling and potential circular dependencies. 
**Recommendation**: Remove these exports. Any other component needing `formatTime` or `parseMarkdown` should import them directly from `./MarkdownContent` or a shared `utils.ts` file.

### B. Hardcoded Inline Styles
Despite the comment `// ── Shared style constants (P1-5: avoid inline objects that break memo) ──`, there are still numerous inline styles scattered throughout the code:
- `style={{ display: 'flex', alignItems: 'center', gap: '8px' }}`
- `style={{ marginTop: hasThinkingContent ? '4px' : '0' }}`
**Issue**: While they don't break memoization if they are on host DOM nodes, they clutter the JSX and make theming difficult.
**Recommendation**: Move these styles to CSS classes in the associated `.css` file.

### C. Diff Parser Edge Cases
The `parseUnifiedDiff` function handles standard `@@` hunks but uses a very basic string-matching approach. 
**Issue**: It may fail or misalign lines if the unified diff contains context lines that coincidentally start with `+`, `-`, or `@@` within the code content, or if the diff header is malformed.
**Recommendation**: Consider using a robust external library like `diff` or `react-diff-viewer` if file diffs become more complex, or at least harden the regex parsing.

## Summary of Action Items
1. **[High Priority]** Refactor `AgentTurnUI` to avoid selecting the global `state.chat.subagents` object. Pass only IDs down the tree and use targeted `useSelector` hooks in `SubagentCard`.
2. **[Medium Priority]** Remove `useEffect` state syncing for the `collapsed` state in `ToolBlockUI` and `EditFileWidget`.
3. **[Low Priority]** Cleanup the `export { formatTime, parseMarkdown }` re-exports.
4. **[Low Priority]** Migrate the remaining inline styles to the stylesheet.
