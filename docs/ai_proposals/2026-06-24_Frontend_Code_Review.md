# Frontend Code Review (app)

**Date:** 2026-06-24
**Reviewer:** Antigravity (AI Agent)
**Target:** `app/` Directory (Vite + React + Redux + Tauri)

This document provides a critical, independent code review of the `agent_core/app` frontend architecture. It covers code organization, state management, component complexity, performance bottlenecks, and potential architectural risks.

---

## 1. Architectural & Structural Concerns

### 1.1 Direct Tauri IPC Coupling in Views
Components like `App.tsx` directly invoke Tauri commands (e.g., `invoke('send_message', ...)`, `invoke('abort_agent', ...)`). 
- **Risk:** Tight coupling between the UI layer and the Rust backend. Makes unit testing the UI without Tauri practically impossible. 
- **Recommendation:** Abstract all IPC calls into a dedicated service layer (e.g., `services/ipc.ts` or `services/agentService.ts`), or integrate them entirely into Redux Async Thunks (like `projectSlice.ts` does).

### 1.2 Monolithic Redux Slices
`features/chat/chatSlice.ts` is exceptionally large (~1300 lines) and acts as a catch-all for state manipulation, stream parsing, and event routing.
- **Risk:** High maintenance burden, merge conflicts, and difficult testing. The logic for parsing DeepSeek `<think>` tags and delta string operations is deeply embedded in reducers.
- **Recommendation:** 
  - Separate pure stream-parsing/string manipulation logic (e.g., `appendDeltaToBlocks`, `<think>` parsing) into utility functions (e.g., `utils/streamParser.ts`).
  - Move complex `RunEventPayload` handling and gap detection to a Redux middleware or listener rather than bloating the slice.

### 1.3 Missing Dependency Injection / API Abstraction
Hardcoded string endpoints and IPC names (`'get_project_sessions'`, `'replay_since'`) are scattered across slices.
- **Recommendation:** Centralize these into a typed API client/constants file.

---

## 2. State Management (Redux Toolkit)

### 2.1 Suboptimal Normalization & Performance Patches
The `chatSlice.ts` relies on arrays for `entries` and performs `O(N)` loops frequently to find and update active blocks. A custom `WeakMap` (`entryMapCache`) was added to memoize `selectEntryById` to avoid `O(N^2)` lookups.
- **Critique:** While the `WeakMap` patch works, it fights Redux Toolkit's design. 
- **Recommendation:** Use Redux Toolkit's `createEntityAdapter`. It natively handles normalized state (`{ ids: [], entities: {} }`), granting `O(1)` lookups by ID and eliminating the need for manual `WeakMap` caching and backwards array traversal.

### 2.2 Inefficient React-Redux Selectors
In `App.tsx`:
```typescript
const entryIds = useSelector((state: RootState) => state.chat.entries.map((e) => e.id), shallowEqual);
```
- **Critique:** `map` creates a new array reference every render. Even with `shallowEqual`, this forces React-Redux to run the equality check on the entire array whenever *any* part of `state.chat` updates.
- **Recommendation:** Use `createSelector` from `@reduxjs/toolkit` to memoize the array mapping:
```typescript
const selectEntryIds = createSelector(
  (state: RootState) => state.chat.entries,
  (entries) => entries.map(e => e.id)
);
const entryIds = useSelector(selectEntryIds);
```

---

## 3. Component Complexity and Maintenance

### 3.1 "God Components"
- **`App.tsx` (433 lines):** Handles routing, keyboard shortcuts (`Escape` to abort), auto-scrolling, session tracking, theme switching, and layout. 
  - **Recommendation:** Extract keyboard shortcut logic to a custom hook (e.g., `useKeyboardShortcuts`). Extract theme logic to `useTheme`. Move routing/session logic to an intermediate container component.
- **`AgentTurn.tsx` (950+ lines):** Responsible for rendering every possible block type (Thinking, Tool, Approval, Error, Iteration UI).
  - **Recommendation:** This file desperately needs decomposition. Split into smaller files inside a `features/chat/components/` folder:
    - `ThinkingBlock.tsx`
    - `ToolExecutionBlock.tsx` (or mapping distinct tools to distinct small components)
    - `ApprovalPrompt.tsx`
    - `TurnIteration.tsx`

### 3.2 Magic Strings and Type Safety
- **Type Assertions:** Heavy reliance on type casting like `as unknown as RunEventPayload` and generic `Record<string, unknown>`.
- **Recommendation:** Leverage TypeScript fully. If the Rust backend uses serde to emit JSON, you can use tools like `ts-rs` to automatically generate TypeScript interfaces from Rust structs, ensuring strict contract enforcement without manual casting.

---

## 4. UI/UX and Best Practices

### 4.1 Missing Error Boundaries for Individual Blocks
If one specific tool's output rendering crashes (e.g., due to malformed JSON inside `AgentTurn.tsx`), the entire chat view might unmount.
- **Recommendation:** Wrap critical sub-components (like specific tool renderers) in `ErrorBoundary` components to prevent a single bad message from bringing down the whole UI.

### 4.2 Inline Styles
Several components mix class-based CSS with heavy inline styling (e.g., `style={{ marginLeft: '6px', paddingLeft: '12px', borderLeft: '1px solid var(--text-muted)' }}`).
- **Recommendation:** Migrate these to the existing `App.css` (or CSS Modules / Tailwind if adopted later) for better readability and reusability.

---

## 5. Summary & Action Items

**Overall Assessment:** The frontend is highly functional and handles complex state synchronization (streaming, gaps, subagents) impressively well. However, it is suffering from architectural strain as features have accumulated. The massive size of `chatSlice.ts` and `AgentTurn.tsx` pose immediate technical debt risks.

### Immediate Action Items (High Priority)
1. **Refactor `AgentTurn.tsx`:** Break it apart into smaller, modular components.
2. **Selector Optimization:** Fix unmemoized array mappings in `useSelector` hooks to prevent unnecessary re-renders during high-frequency streaming.
3. **Isolate Parsing Logic:** Move the DeepSeek `<think>` parsing and delta application logic out of `chatSlice.ts` into a dedicated testing-friendly pure utility file. 

### Medium-Term Architectural Items
1. **Redux Normalization:** Migrate the chat state to use `createEntityAdapter`.
2. **IPC Abstraction:** Decouple Tauri `invoke` calls from UI components into a service layer or thunks.
3. **Type Generation:** Sync Rust structs directly to TypeScript types to eliminate `Record<string, unknown>` and `as unknown`.
