Now I have read all files. Let me compile the comprehensive code quality report.

---

# Frontend Code Quality Review Report

## settings/ Directory

---

### [severity: P1] Unhandled promise in dispatch — no error handling on config save
**File:** app/src/components/settings/PermissionsTab.tsx:78
**Problem:** `dispatch(saveConfig(newConfig))` is fire-and-forget. The returned promise is not awaited or caught. If the backend save fails, the user gets no feedback and the UI may show stale state.
**Current code:**
```ts
const handleSelectLevel = (mode: string, autoAllow?: string) => {
  const newConfig = { ... };
  dispatch(saveConfig(newConfig)); // ← unhandled promise
};
```
**Recommendation:** Wrap in async/try-catch and surface errors to the user:
```ts
const handleSelectLevel = async (mode: string, autoAllow?: string) => {
  try {
    await dispatch(saveConfig(newConfig)).unwrap();
  } catch (e) {
    setError(String(e));
  }
};
```

---

### [severity: P1] `useDispatch<any>()` — untyped dispatch
**File:** app/src/components/settings/PermissionsTab.tsx:56
**Problem:** `useDispatch<any>()` bypasses all type safety for dispatched thunks and actions.
**Current code:**
```ts
const dispatch = useDispatch<any>();
```
**Recommendation:** Use the typed dispatch hook already available in the codebase:
```ts
const dispatch = useAppDispatch();
```

---

### [severity: P2] Clickable `div` for permission level selection — not accessible
**File:** app/src/components/settings/PermissionsTab.tsx:99-101
**Problem:** Permission level cards are `<div onClick={...}>` elements. They lack `role`, `tabIndex`, and keyboard event handlers. Users cannot navigate or select via keyboard, and screen readers won't announce them as interactive.
**Current code:**
```tsx
<div
  key={level.id}
  onClick={() => handleSelectLevel(level.mode, level.auto_allow_up_to)}
  className={`permission-card ${isActive ? 'active' : ''}`}
>
```
**Recommendation:** Use a `<button>` element or add `role="radio"`, `tabIndex={0}`, `aria-checked={isActive}`, and an `onKeyDown` handler for Enter/Space.

---

### [severity: P2] Unsafe cast on appearance value
**File:** app/src/components/settings/GeneralTab.tsx:30
**Problem:** `e.target.value as 'system' | 'dark' | 'light'` is an unchecked cast. If the select options ever diverge from the type, this silently passes invalid data to Redux.
**Current code:**
```ts
onChange={(e) => dispatch(setAppearance(e.target.value as 'system' | 'dark' | 'light'))}
```
**Recommendation:** Validate the value at runtime or derive options from a typed constant array:
```ts
const APPEARANCE_OPTIONS = ['system', 'dark', 'light'] as const;
type Appearance = typeof APPEARANCE_OPTIONS[number];
// ...
const val = APPEARANCE_OPTIONS.includes(e.target.value as Appearance) ? (e.target.value as Appearance) : 'system';
```

---

### [severity: P2] Unsafe cast on memory mode
**File:** app/src/components/settings/MemoryTab.tsx:71
**Problem:** `(memory?.mode as MemoryMode) ?? 'standard'` is an unchecked cast. A backend-returned mode string that doesn't match the union will silently flow through.
**Current code:**
```ts
const currentMode = (memory?.mode as MemoryMode) ?? 'standard';
```
**Recommendation:** Validate against known modes:
```ts
const VALID_MODES: MemoryMode[] = ['stateless', 'standard', 'deep'];
const rawMode = memory?.mode;
const currentMode = rawMode && VALID_MODES.includes(rawMode as MemoryMode) ? (rawMode as MemoryMode) : 'standard';
```

---

### [severity: P2] `any` type in ReflectionModelSelector prop typing
**File:** app/src/components/settings/MemoryTab.tsx:336
**Problem:** `config: NonNullable<ReturnType<typeof useSelector<RootState, any>>>` uses `any` as the selector return type, defeating type safety for the entire config object.
**Current code:**
```ts
config: NonNullable<ReturnType<typeof useSelector<RootState, any>>>;
```
**Recommendation:** Import and use the actual config type:
```ts
import type { AppConfig } from '../../features/settings/settingsSlice';
// ...
config: AppConfig;
```

---

### [severity: P2] `any` types in provider/model destructuring
**File:** app/src/components/settings/MemoryTab.tsx:366,368
**Problem:** `Object.entries(config.providers).forEach(([providerKey, provider]: [string, any])` and `Object.entries(provider.models).forEach(([modelKey]: [string, any])` use `any`, losing all type information.
**Recommendation:** Use the actual provider/model types from the settings slice.

---

### [severity: P2] Inline async onClick handler — hard to test, not memoized
**File:** app/src/components/settings/MemoryTab.tsx:261-275
**Problem:** A complex multi-line async function is inlined in the `onClick` of a `<span>`. This is hard to read, test, and causes a new function allocation every render. Also, using a `<span>` with `onClick` and `cursor: pointer` is not semantically a button.
**Current code:**
```tsx
<span
  className={`badge badge-${...}`}
  style={{ cursor: 'pointer' }}
  onClick={async () => {
    if (!config || !memory) return;
    const newConfig = { ... };
    try {
      await dispatch(saveConfig(newConfig));
    } catch (e) {
      console.error('Failed to toggle embedding', e);
    }
  }}
>
```
**Recommendation:** Extract to a named `handleToggleEmbedding` function wrapped in `useCallback`. Use a `<button>` element instead of `<span>`.

---

### [severity: P3] Magic numbers and strings in default memory config
**File:** app/src/components/settings/MemoryTab.tsx:78-88
**Problem:** Hardcoded values like `'~/.agverse/memory.db'`, `'BAAI/bge-small-en-v1.5'`, `5`, `2000`, `20` are magic values with no named constants or documentation.
**Recommendation:** Extract to named constants:
```ts
const DEFAULT_MEMORY_CONFIG = {
  db_path: '~/.agverse/memory.db',
  embedding_model: 'BAAI/bge-small-en-v1.5',
  max_core_blocks: 5,
  default_block_max_chars: 2000,
  // ...
};
```

---

### [severity: P2] Native `confirm()` blocks UI thread
**File:** app/src/components/settings/ProviderTab.tsx:351
**Problem:** `confirm()` is a blocking native dialog that behaves inconsistently in Tauri WebView. The codebase already has a `DialogManager` / `useConfirmDialog` hook designed to replace this.
**Current code:**
```ts
if (!confirm(`Delete provider "${key}" and all its models (${names})?`)) return;
```
**Recommendation:** Use the existing `useConfirmDialog` hook:
```ts
const { confirm, dialogElement } = useConfirmDialog();
// ...
const ok = await confirm({ title: 'Delete Provider', message: `...`, danger: true });
if (!ok) return;
```

---

### [severity: P2] Magic numbers in provider save config
**File:** app/src/components/settings/ProviderTab.tsx:173-179
**Problem:** `32768`, `100`, `1800` are magic numbers with no explanation.
**Current code:**
```ts
max_context_tokens: 32768,
max_iterations: 100,
request_timeout_secs: 1800,
```
**Recommendation:** Extract to named constants with comments explaining defaults.

---

### [severity: P3] Using array index as React key
**File:** app/src/components/settings/ProviderTab.tsx:260
**Problem:** `key={index}` in the models table map. If models are reordered or deleted, React may incorrectly reuse DOM nodes, causing input state to leak between rows.
**Current code:**
```tsx
{form.models.map((row, index) => (
  <div key={index} className="models-table-row">
```
**Recommendation:** Use a stable unique ID per row (e.g., a generated UUID when adding rows).

---

### [severity: P2] Missing try/catch in handleRefresh — unhandled promise rejection
**File:** app/src/components/settings/SkillsTab.tsx:13-18
**Problem:** `await invalidate()` and `await refresh()` can throw, but there's no try/catch. A failure will result in an unhandled promise rejection and the `setRefreshing(false)` will never run if `invalidate` throws, leaving the button permanently disabled.
**Current code:**
```ts
const handleRefresh = async () => {
  setRefreshing(true);
  await invalidate();
  await refresh();
  setRefreshing(false);
};
```
**Recommendation:**
```ts
const handleRefresh = async () => {
  setRefreshing(true);
  try {
    await invalidate();
    await refresh();
  } catch (e) {
    console.error('Failed to refresh skills', e);
  } finally {
    setRefreshing(false);
  }
};
```

---

### [severity: P3] Using array index as key for skills
**File:** app/src/components/settings/SkillsTab.tsx:50
**Problem:** `key={i}` — if skills reorder, React may misidentify items.
**Current code:**
```tsx
{skills.map((skill, i) => (
  <div key={i} ...>
```
**Recommendation:** Use `skill.name` or `skill.id` as key.

---

### [severity: P3] Inline onClick not memoized in tab buttons
**File:** app/src/components/settings/SettingsModal.tsx:87
**Problem:** `onClick={() => dispatch(setActiveTab(tab.key))}` creates a new function each render for each tab button.
**Recommendation:** Extract a `handleTabClick` callback:
```ts
const handleTabClick = useCallback((key: typeof activeTab) => {
  dispatch(setActiveTab(key));
}, [dispatch]);
// ...
onClick={() => handleTabClick(tab.key)}
```

---

## ui/ Directory

---

### [severity: P1] useEffect missing dependencies — stale closure risk
**File:** app/src/components/ui/CronjobModal.tsx:49-54
**Problem:** `loadJobs` and `loadSkills` are called in the effect but not listed in dependencies, and they are not memoized with `useCallback`. This violates the rules of hooks and can cause stale closures.
**Current code:**
```ts
useEffect(() => {
  if (isOpen) {
    loadJobs();
    loadSkills();
  }
}, [isOpen]); // ← loadJobs, loadSkills missing
```
**Recommendation:** Either wrap `loadJobs`/`loadSkills` in `useCallback` and add to deps, or inline the logic.

---

### [severity: P1] `invoke<any[]>("get_skills")` — untyped backend response
**File:** app/src/components/ui/CronjobModal.tsx:67
**Problem:** `any[]` provides no type safety. The code accesses `s.name` without any compile-time guarantee it exists.
**Current code:**
```ts
const data = await invoke<any[]>("get_skills");
setSkillsList(data.map((s) => ({ id: s.name, name: s.name })));
```
**Recommendation:** Define and use a proper interface:
```ts
interface SkillInfo { name: string; description: string; version: string; triggers: string[]; }
const data = await invoke<SkillInfo[]>("get_skills");
```

---

### [severity: P2] `alert()` for error — blocking, inconsistent in Tauri
**File:** app/src/components/ui/CronjobModal.tsx:92
**Problem:** `alert()` blocks the UI and is unreliable in Tauri WebView. The codebase has a `DialogManager` for this purpose.
**Current code:**
```ts
} catch (e) {
  alert(`Error creating job: ${e}`);
}
```
**Recommendation:** Use an error state variable and render an inline error message, or use the `useConfirmDialog`/dialog system.

---

### [severity: P2] No confirmation for destructive delete action
**File:** app/src/components/ui/CronjobModal.tsx:96-103
**Problem:** `handleDelete` immediately deletes a cronjob with no confirmation dialog. Accidental clicks will permanently delete tasks.
**Current code:**
```ts
const handleDelete = async (id: string) => {
  try {
    await invoke("delete_cronjob", { id });
    loadJobs();
  } catch (e) {
    console.error(e);
  }
};
```
**Recommendation:** Add a confirmation dialog before deletion.

---

### [severity: P2] `any` type in provider destructuring
**File:** app/src/components/ui/CronjobModal.tsx:291
**Problem:** `Object.entries(config.providers).map(([providerKey, provider]: [string, any])` uses `any`.
**Recommendation:** Use the proper provider type.

---

### [severity: P2] Form validation incomplete — cadenceValue not validated
**File:** app/src/components/ui/CronjobModal.tsx:381
**Problem:** The save button is only disabled when `!name || !prompt`, but `cadenceValue` could be empty or invalid (especially for "Custom" cron expressions). No validation is performed on the cron string.
**Current code:**
```tsx
<button className="btn-primary" onClick={handleCreate} disabled={!name || !prompt}>
```
**Recommendation:** Validate `cadenceValue` based on `cadenceType` (e.g., non-empty for Custom, valid time for Daily).

---

### [severity: P2] setTimeout not cleaned up — potential memory leak
**File:** app/src/components/ui/DialogManager.tsx:44
**Problem:** `setTimeout(() => inputRef.current?.focus(), 50)` is not cleared on unmount. If the component unmounts within 50ms, the callback will still fire and attempt to access a stale ref.
**Current code:**
```ts
useEffect(() => {
  if (state && 'defaultValue' in state) {
    setInputValue(state.defaultValue ?? '');
    setTimeout(() => inputRef.current?.focus(), 50);
  }
}, [state]);
```
**Recommendation:**
```ts
useEffect(() => {
  if (state && 'defaultValue' in state) {
    setInputValue(state.defaultValue ?? '');
    const timer = setTimeout(() => inputRef.current?.focus(), 50);
    return () => clearTimeout(timer);
  }
}, [state]);
```

---

### [severity: P3] Overly complex nested conditional in handleConfirm
**File:** app/src/components/ui/DialogManager.tsx:48-58
**Problem:** Triple-nested `if` with redundant `'resolve' in state` check (all DialogState variants have `resolve`).
**Current code:**
```ts
const handleConfirm = useCallback(() => {
  if (state) {
    if ('resolve' in state) {
      if ('defaultValue' in state) {
        (state as PromptDialogState).resolve(inputValue);
      } else {
        (state as ConfirmDialogState).resolve(true);
      }
    }
  }
  onClose();
}, [state, inputValue, onClose]);
```
**Recommendation:** Simplify:
```ts
const handleConfirm = useCallback(() => {
  if (state) {
    if ('defaultValue' in state) {
      (state as PromptDialogState).resolve(inputValue);
    } else {
      (state as ConfirmDialogState).resolve(true);
    }
  }
  onClose();
}, [state, inputValue, onClose]);
```

---

### [severity: P2] eslint-disable for exhaustive-deps — missing `config` dependency
**File:** app/src/components/ui/NewAgentModal.tsx:46-76
**Problem:** The effect references `config?.default_model` but `config` is not in the dependency array. The eslint-disable suppresses the warning. If `config` loads after the modal opens, the model default won't update.
**Current code:**
```ts
useEffect(() => {
  if (isOpen) {
    // ... uses config?.default_model
  }
  // eslint-disable-next-line react-hooks/exhaustive-deps
}, [isOpen, editingAgent]);
```
**Recommendation:** Add `config?.default_model` to the dependency array and remove the disable.

---

### [severity: P2] `invoke<any[]>("get_skills")` — untyped
**File:** app/src/components/ui/NewAgentModal.tsx:80
**Problem:** Same `any[]` pattern as CronjobModal — no type safety on skill data.
**Recommendation:** Define and use a `SkillInfo` interface.

---

### [severity: P2] Massive code duplication — model/skill/tool dropdowns
**File:** app/src/components/ui/NewAgentModal.tsx:223-317, app/src/components/agents/tabs/AgentConfigTab.tsx:178-269
**Problem:** The model dropdown, skill dropdown (with search, checkbox styling), and tool dropdown are nearly identical copy-pasted blocks across `NewAgentModal.tsx` and `AgentConfigTab.tsx` (~200 lines duplicated). This is a maintenance burden — any fix must be applied in multiple places.
**Recommendation:** Extract reusable components: `<ModelSelector>`, `<SkillSelector>`, `<ToolSelector>`.

---

### [severity: P3] Magic numbers for default agent config
**File:** app/src/components/ui/NewAgentModal.tsx:39-40,59-60
**Problem:** `50` (maxIterations) and `32000` (maxContextTokens) are magic numbers repeated in multiple places.
**Recommendation:** Extract to shared constants:
```ts
const DEFAULT_MAX_ITERATIONS = 50;
const DEFAULT_MAX_CONTEXT_TOKENS = 32000;
```

---

## agents/ Directory

---

### [severity: P1] `confirm()` blocking call + unhandled dispatch promise
**File:** app/src/components/agents/AgentDashboard.tsx:19-22
**Problem:** Uses native `confirm()` (blocks UI, inconsistent in Tauri) and `await dispatch(deleteAgent(agent.id))` has no try/catch. If deletion fails, `setSelectedAgent(null)` still runs, and the error is swallowed.
**Current code:**
```ts
const handleDelete = async () => {
  if (confirm(`Are you sure you want to delete ${agent.name}?`)) {
    await dispatch(deleteAgent(agent.id));
    dispatch(setSelectedAgent(null));
  }
};
```
**Recommendation:** Use `useConfirmDialog` and wrap dispatch in try/catch:
```ts
const handleDelete = async () => {
  const ok = await confirm({ title: 'Delete Agent', message: `...`, danger: true });
  if (!ok) return;
  try {
    await dispatch(deleteAgent(agent.id)).unwrap();
    dispatch(setSelectedAgent(null));
  } catch (e) {
    setError(String(e));
  }
};
```

---

### [severity: P2] `filteredAgents` not memoized — recomputed every render
**File:** app/src/components/agents/AgentList.tsx:21-24
**Problem:** The filter operation runs on every render even when `agents` and `searchQuery` haven't changed.
**Current code:**
```ts
const filteredAgents = agents.filter((a) =>
  a.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
  (a.description && a.description.toLowerCase().includes(searchQuery.toLowerCase()))
);
```
**Recommendation:** Wrap in `useMemo`:
```ts
const filteredAgents = useMemo(() => agents.filter(...), [agents, searchQuery]);
```

---

### [severity: P2] Clickable `div` for agent list items — not accessible
**File:** app/src/components/agents/AgentList.tsx:53-56
**Problem:** Agent list items are `<div onClick={...}>` without `56
**Problem:** Agent list items are `<div onClickrole`, `tabIndex`, or={...}>` without ` keyboard handlersrole`, `tabIndex`, or. Users cannot select agents via keyboard.
** keyboard handlersCurrent code:**
```tsx
<div
  key={. Users cannot select agents via keyboard.
**Current code:**
```tsx
<div
  key={agent.id}
  className={`agent-list-item ${selectedAgentId === agent.id ? "item-active" : ""}`}
  onClick={() => dispatch(setSelectedAgent(agent.id))}
>
```
**Recommendation:** Use `<button>` or add `role="button"`, `tabIndex={0}`, `onKeyDown` for Enter/Space, and `aria-selected`.

---

### [severity: P3] `selectedAgent` not memoized
**File:** app/src/components/agents/AgentsPage.tsx:17
**Problem:** `agents.find(...)` runs on every render. Minor, but for large agent lists this is unnecessary work.
**Current code:**
```ts
const selectedAgent = agents.find((a) => a.id === selectedAgentId);
```
**Recommendation:** Wrap in `useMemo`:
```ts
const selectedAgent = useMemo(() => agents.find((a) => a.id === selectedAgentId), [agents, selectedAgentId]);
```

---

### [severity: P2] useEffect missing dependencies — `loadDrafts` not in deps
**File:** app/src/components/agents/SkillDrafts.tsx:57-59
**Problem:** `loadDrafts` is called in the effect but not in the dependency array, and it's not memoized. This is a rules-of-hooks violation.
**Current code:**
```ts
useEffect(() => {
  loadDrafts();
}, []);
```
**Recommendation:** Wrap `loadDrafts` in `useCallback` and add to deps, or inline the fetch logic.

---

### [severity: P2] No user feedback for `loadDrafts` failure
**File:** app/src/components/agents/SkillDrafts.tsx:48-55
**Problem:** `loadDrafts` only does `console.error(e)` on failure. The user sees no error state, just an empty list with no explanation.
**Current code:**
```ts
const loadDrafts = async () => {
  try {
    const data = await invoke<SkillDraft[]>("list_skill_drafts");
    setDrafts(data);
  } catch (e) {
    console.error(e);
  }
};
```
**Recommendation:** Set an error state and display it to the user.

---

### [severity: P3] Magic number for generation limit
**File:** app/src/components/agents/SkillDrafts.tsx:67
**Problem:** `limit: 100` is a magic number.
**Recommendation:** Extract to a named constant `const DRAFT_GENERATION_LIMIT = 100;`.

---

### [severity: P1] setTimeout not cleaned up — memory leak on unmount
**File:** app/src/components/agents/tabs/AgentConfigTab.tsx:104
**Problem:** `setTimeout(() => setSuccessMsg(""), 3000)` is not cleared. If the component unmounts within 3 seconds (e.g., user switches tabs), React will attempt to set state on an unmounted component.
**Current code:**
```ts
setSuccessMsg("Configuration saved successfully.");
setTimeout(() => setSuccessMsg(""), 3000);
```
**Recommendation:**
```ts
setSuccessMsg("Configuration saved successfully.");
const timer = setTimeout(() => setSuccessMsg(""), 3000);
return () => clearTimeout(timer); // or use a ref to track the timer
```

---

### [severity: P2] `invoke<any[]>("get_skills")` — untyped
**File:** app/src/components/agents/tabs/AgentConfigTab.tsx:64
**Problem:** Same `any[]` pattern — no type safety.
**Recommendation:** Use a proper `SkillInfo` interface.

---

### [severity: P2] `any` type in provider destructuring
**File:** app/src/components/agents/tabs/AgentConfigTab.tsx:188
**Problem:** `Object.entries(config.providers).map(([providerKey, provider]: [string, any])` uses `any`.

---

### [severity: P2] Inline filter computed every render
**File:** app/src/components/agents/tabs/AgentConfigTab.tsx:236
**Problem:** `skillsList.filter(s => s.name.toLowerCase().includes(skillSearch.toLowerCase()))` is computed inline in JSX on every render, even when the dropdown is closed.
**Recommendation:** Memoize with `useMemo`:
```ts
const filteredSkills = useMemo(() =>
  skillsList.filter(s => s.name.toLowerCase().includes(skillSearch.toLowerCase())),
  [skillsList, skillSearch]
);
```

---

### [severity: P2] Dead code / placeholder in AgentMemoryTab
**File:** app/src/components/agents/tabs/AgentMemoryTab.tsx:47-48
**Problem:** The "Core Memory" panel shows a hardcoded placeholder string instead of actual data. The comment even says "In a real app, this would be fetched from the backend" and "Awaiting deeper integration." This is incomplete/dead code shipped to users.
**Current code:**
```tsx
{/* In a real app, this would be fetched from the backend */}
{`# ${agent.name} Core Memory\n\nAgent definition and core directives are initialized.\n...`}
```
**Recommendation:** Either fetch and display real agverse.md content (as `MemoryTab.tsx` already does via `invoke('get_agverse_md')`) or clearly label this as a coming-soon placeholder.

---

### [severity: P2] `handleSearch` not memoized
**File:** app/src/components/agents/tabs/AgentMemoryTab.tsx:22-25
**Problem:** `handleSearch` is recreated every render. While it's passed to a `<form onSubmit>`, memoizing is best practice.
**Recommendation:** Wrap in `useCallback`.

---

### [severity: P3] No loading state for memory search
**File:** app/src/components/agents/tabs/AgentMemoryTab.tsx:17-20
**Problem:** When `searchAgentMemory` is dispatched, there's no loading indicator. The user doesn't know if results are loading or if there are genuinely no results.
**Recommendation:** Track a loading state from the Redux store and display a spinner.

---

### [severity: P2] Generic placeholder text for all skills
**File:** app/src/components/agents/tabs/AgentSkillsTab.tsx:66-68
**Problem:** Every skill card displays the same hardcoded text: "This skill provides specialized context and tool sets to enhance the agent's performance in related tasks." This is not the actual skill description — it's a generic filler. The `SkillsTab.tsx` in settings already fetches real skill metadata.
**Current code:**
```tsx
<div style={{ fontSize: "13px", color: "var(--text-muted)", lineHeight: "1.5" }}>
  This skill provides specialized context and tool sets to enhance the agent's performance in related tasks.
</div>
```
**Recommendation:** Fetch skill metadata (description, triggers) via `useSkills` hook and display real descriptions.

---

### [severity: P3] Magic number for history limit
**File:** app/src/components/agents/tabs/AgentHistoryTab.tsx:18
**Problem:** `limit: 50` is a magic number.
**Recommendation:** Extract to `const HISTORY_PAGE_SIZE = 50;`.

---

### [severity: P3] No loading or error state in AgentHistoryTab
**File:** app/src/components/agents/tabs/AgentHistoryTab.tsx:17-19
**Problem:** No loading indicator while `fetchAgentHistory` is in flight, and no error display if it fails.
**Recommendation:** Add loading/error states from the Redux store.

---

## review/ Directory

---

### [severity: P1] setTimeout not cleaned up — memory leak
**File:** app/src/components/review/ReviewTab.tsx:313-319
**Problem:** `setTimeout` inside the `useEffect` is not cleared on cleanup. If the component unmounts or `fileContents` changes before 150ms, the callback fires on a stale state.
**Current code:**
```ts
setTimeout(() => {
  const id = `review-file-${filePath}`;
  const element = document.getElementById(id);
  if (element) {
    element.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }
}, 150);
```
**Recommendation:** Store the timer and clear it in the effect's cleanup function.

---

### [severity: P1] useEffect dependency on `fileContents` object — causes effect re-registration
**File:** app/src/components/review/ReviewTab.tsx:300-324
**Problem:** The effect depends on `fileContents` (a state object that changes every time a file is fetched). Each time a file content is loaded, the entire event listener is removed and re-added. This is inefficient and can cause missed events.
**Current code:**
```ts
useEffect(() => {
  const handleOpen = (e: Event) => {
    // ... references fileContents
  };
  window.addEventListener('open-right-sidebar', handleOpen);
  return () => window.removeEventListener('open-right-sidebar', handleOpen);
}, [fileContents]);
```
**Recommendation:** Use a ref to access the latest `fileContents` without adding it to the dependency array:
```ts
const fileContentsRef = useRef(fileContents);
fileContentsRef.current = fileContents;
useEffect(() => {
  const handleOpen = (e: Event) => {
    // use fileContentsRef.current instead of fileContents
  };
  window.addEventListener('open-right-sidebar', handleOpen);
  return () => window.removeEventListener('open-right-sidebar', handleOpen);
}, []);
```

---

### [severity: P2] Pervasive `any` types in FileDiffViewer
**File:** app/src/components/review/ReviewTab.tsx:14,20,23,26,365
**Problem:** Multiple `any[]` and `any` types: `diffRows: any[]`, `useState<any[]>`, `getDiffItems = useCallback((rows: any[], content?: string)`, `res: any[]`, `diffRows: any[]`. This eliminates all type safety for the diff rendering logic.
**Current code:**
```ts
interface FileDiffViewerProps {
  diffRows: any[];
  // ...
}
const [items, setItems] = useState<any[]>([]);
const getDiffItems = useCallback((rows: any[], content?: string) => {
  const res: any[] = [];
```
**Recommendation:** Define proper interfaces:
```ts
interface DiffRow {
  type: 'add' | 'del' | 'context' | 'gap';
  oldLineNo: number | null;
  newLineNo: number | null;
  oldText: string;
  newText: string;
  gapSize?: number;
  gapStartOld?: number;
  gapStartNew?: number;
}
```

---

### [severity: P2] `fetchFileContent` and `toggleFileExpanded` not memoized
**File:** app/src/components/review/ReviewTab.tsx:276-298
**Problem:** Both functions are recreated every render. `fetchFileContent` is passed as a prop to `FileDiffViewer` via `onFetchContent`, and since it's a new reference each time, it can trigger unnecessary re-renders of the memoized child (though `FileDiffViewer` is not memoized, so the impact is limited — but it's still an anti-pattern).
**Recommendation:** Wrap both in `useCallback`.

---

### [severity: P2] Code duplication — `getParsedArgs` and `modifiedFiles` logic
**File:** app/src/components/review/ReviewTab.tsx:327-399, app/src/components/review/OverviewTab.tsx:47-83
**Problem:** The `getParsedArgs` helper and the `modifiedFiles` extraction logic (iterating entries, checking tool names, parsing args) are duplicated nearly verbatim between `ReviewTab.tsx` and `OverviewTab.tsx`.
**Recommendation:** Extract to a shared utility:
```ts
// shared/modifiedFilesExtractor.ts
export function extractModifiedFiles(entries: ChatEntry[]): ModifiedFile[] { ... }
export function getParsedArgs(rawArgs: unknown): Record<string, unknown> { ... }
```

---

### [severity: P2] `any` types in OverviewTab
**File:** app/src/components/review/OverviewTab.tsx:49,90,230
**Problem:** `getParsedArgs = (rawArgs: any)`, `(sa: any)` for subagents — no type safety.
**Recommendation:** Use proper types from the chat slice.

---

### [severity: P2] `getArtifactDetails`, `sectionHeader`, `statusDot` not memoized
**File:** app/src/components/review/OverviewTab.tsx:129,178,192
**Problem:** Three functions are defined inside the component body without `useCallback`. They're called during render, creating new closures each time. While this won't cause infinite loops, it's unnecessary work.
**Recommendation:** Wrap in `useCallback` or move outside the component (for pure functions that don't depend on component state).

---

### [severity: P2] Direct DOM manipulation via inline style in event handlers
**File:** app/src/components/review/OverviewTab.tsx:274-275, app/src/components/review/ReviewTab.tsx:458-463
**Problem:** `onMouseEnter`/`onMouseLeave` directly mutate `e.currentTarget.style.color` / `backgroundColor`. This is an anti-pattern in React — it bypasses the declarative rendering model and can cause style conflicts.
**Current code:**
```tsx
onMouseEnter={(e) => (e.currentTarget.style.color = 'var(--text-main)')}
onMouseLeave={(e) => (e.currentTarget.style.color = 'var(--text-dim)')}
```
**Recommendation:** Use CSS hover states or React state to manage hover styling.

---

### [severity: P3] Magic number for file display limit
**File:** app/src/components/review/OverviewTab.tsx:213,262
**Problem:** `6` is a magic number for the initial file display limit.
**Recommendation:** Extract to `const INITIAL_FILE_LIMIT = 6;`.

---

### [severity: P3] Magic number for gap expansion
**File:** app/src/components/review/ReviewTab.tsx:137,140
**Problem:** `10` is a magic number for how many lines to expand.
**Recommendation:** Extract to `const GAP_EXPAND_LINES = 10;`.

---

### [severity: P2] Recursive `tryReadPaths` without await — race condition
**File:** app/src/components/review/DocumentTab.tsx:29-51
**Problem:** `tryReadPaths(index + 1)` is called without `await` in the catch block. This creates a chain of unawaited promises. While the `isMounted` check prevents state updates after unmount, multiple parallel reads could race, and the order of `setContent` calls is not guaranteed to match the path order.
**Current code:**
```ts
} catch (err) {
  // If it fails, try the next path
  tryReadPaths(index + 1); // ← not awaited
}
```
**Recommendation:** Use `await tryReadPaths(index + 1)` or refactor to a sequential loop with `for...of` and `await`.

---

### [severity: P3] `relativePaths` array in useEffect deps — reference instability
**File:** app/src/components/review/DocumentTab.tsx:58
**Problem:** `relativePaths` is an array prop. If the parent creates a new array literal each render (e.g., `relativePaths={['a', 'b']}`), this effect will re-run every render, re-fetching files unnecessarily.
**Recommendation:** Memoize the array in the parent, or use a JSON-serialized key for comparison.

---

### [severity: P3] No accessibility on overview section headers
**File:** app/src/components/review/OverviewTab.tsx:178-190
**Problem:** Section headers are `<div onClick={...}>` without `role`, `tabIndex`, or `aria-expanded`. Users cannot toggle sections via keyboard.
**Recommendation:** Use `<button>` with `aria-expanded={expanded.has(key)}` and `aria-controls`.

---

### [severity: P3] No accessibility on review file headers
**File:** app/src/components/review/ReviewTab.tsx:446-464
**Problem:** File headers are `<div onClick={...}>` without keyboard support or ARIA attributes for expand/collapse.
**Recommendation:** Use `<button>` with `aria-expanded` and `aria-controls`.

---

## Summary

| Severity | Count | Key Themes |
|----------|-------|------------|
| **P0** | 0 | — |
| **P1** | 5 | Unhandled promises, memory leaks (uncleaned setTimeout), missing effect deps, blocking `confirm()` |
| **P2** | 28 | `any` types, code duplication, missing memoization, inaccessible clickable divs, `alert()`/`confirm()` usage |
| **P3** | 12 | Magic numbers, index keys, minor missing memoization, dead/placeholder code |

**Top cross-cutting issues:**
1. **Code duplication**: Model/skill/tool selector dropdowns are copy-pasted across 3+ files (~600 lines total). Extract reusable components.
2. **`any` types pervasive**: Especially in `ReviewTab.tsx` (diff rows) and all `invoke<any[]>("get_skills")` calls.
3. **Blocking native dialogs**: `confirm()` and `alert()` used despite an existing `DialogManager` replacement.
4. **Missing `useCallback`/`useMemo`**: Most handler functions and derived data are not memoized.
5. **Accessibility**: Clickable `div` elements used throughout instead of semantic buttons, lacking keyboard navigation and ARIA attributes.
