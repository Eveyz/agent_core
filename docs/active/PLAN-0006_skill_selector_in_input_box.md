# PLAN-0006: Skill Selector in Input Box

```yaml
---
id: PLAN-0006
type: PLAN
title: Skill Selector in Input Box
status: Draft
author: agent_core
created: 2026-06-28
updated: 2026-06-28
reviewers: [zniverse]
related: [PLAN-0005]
supersedes: ~
superseded_by: ~
tags: [skills, ui, input, caching, frontend]
---
```

## Objective

Add a skills icon button to the right of the "+" button in the chat input box. Clicking it opens a searchable dropdown listing all available skills, and selecting a skill inserts it as an `@skill:name` mention into the input (similar to the existing `@file` mention pattern). The entire flow must be backed by a multi-layer cache to avoid redundant filesystem scanning and network calls.

## Background

### Current State

The project already has a full skill infrastructure:

- **Rust backend** (`core/src/skills/`): `SkillManager` discovers and parses SKILL.md files from standard directories (`~/.agverse/skills/`, project-local dirs, built-in dirs). Each skill has a `SkillManifest` with name, description, version, triggers, tags, etc.
- **Tauri command** (`app/src-tauri/src/lib.rs:666`): `get_skills()` already exists — but creates a new `SkillManager` and re-scans filesystem on every call. **No caching.**
- **Frontend** (`app/src/components/settings/SkillsTab.tsx`): Displays skills in Settings modal. Fetches via `invoke('get_skills')` on every mount. **No caching.**
- **Chat input** (`app/src/components/chat/ChatInput.tsx`): Has `.input-actions-left` with a placeholder Plus button, and `.input-actions-right` with ModelSelector and send/stop buttons.
- **Autocomplete** (`app/src/hooks/useAutocomplete.ts`): Handles `@` for file mentions and `/` for commands with a dropdown, keyboard navigation, and highlight overlay.
- **ModelSelector** (`app/src/components/chat/ModelSelector.tsx`): Reference pattern for a searchable dropdown component with outside-click-dismiss.

### Why Now

The user frequently installs/manages skills (from Agverse marketplace, manual installs, etc.) but there is no quick way to browse or invoke skills from the chat input. Currently the only way to see installed skills is to open Settings → Skills tab. Adding a toolbar button + dropdown gives instant access.

### Cache Problem

Every `get_skills` call:
1. Instantiates a new `SkillManager` (allocates search dirs)
2. Glob-walks the entire skill search tree (scanning `**/SKILL.md` files)
3. Parses YAML frontmatter for each found file
4. Deduplicates and sorts by priority

This is wasteful when called repeatedly (switching tabs, remounting components). The PLAN-0005 project established a caching pattern (OnceLock with TTL) that we apply here.

## Scope

### In Scope

- **Backend cache**: Add `OnceLock`-based TTL cache to the `get_skills` Tauri command (30s TTL, invalidated on skill changes)
- **Redux cache**: Add `skillsCache` to `chatSlice` with timestamp, so components can read without re-fetching
- **`useSkills` hook**: A reusable hook that checks Redux cache first, dispatches fetch only when stale
- **`SkillSelector` component**: Searchable dropdown triggered by a tooltip icon in the input toolbar
  - Manual refresh button in dropdown header
  - Recent used skills section (top 3-5)
  - Skill count badge on wand icon
  - Full keyboard navigation (arrow keys, enter, escape)
  - ARIA labels for accessibility
- **ChatInput integration**: Icon button in `.input-actions-left` beside the Plus button, `handleSkillSelect` inserts `@skill:name ` text into textarea
- **Keyboard shortcut**: Cmd/Ctrl+K to open SkillSelector dropdown
- **Cache invalidation**: Manual cache invalidation command for Settings skill installation
- **CSS**: Skill dropdown styles (reuse `.model-dropdown` base, add skill-specific item styles)
- **AI-NOTE**: Create a note documenting the caching strategy and component architecture for future reference

### Out of Scope

- Actually invoking/loading the skill — this is handled by the LLM agent's `skill_load` tool at send time
- Real-time skill file watching (polling or fsnotify) — cache TTL refresh is sufficient
- Replacing the existing ModelSelector or Plus button UI
- Server-side skill management (upload, install from marketplace in this component)

## Design

### 1. Backend Cache — `app/src-tauri/src/lib.rs`

```rust
use std::sync::OnceLock;
use std::time::Instant;

static SKILL_CACHE: OnceLock<(Instant, Vec<SkillManifest>)> = OnceLock::new();
const SKILL_CACHE_TTL: u64 = 30; // seconds

#[tauri::command]
async fn get_skills() -> Result<Vec<SkillManifest>, String> {
    // Check cache hit
    if let Some((cached_at, cached)) = SKILL_CACHE.get() {
        if cached_at.elapsed().as_secs() < SKILL_CACHE_TTL {
            return Ok(cached.clone());
        }
    }
    // Cache miss — scan from disk
    let mut manager = SkillManager::with_defaults();
    manager.scan().map_err(|e| e.to_string())?;
    let skills: Vec<SkillManifest> = manager.list().into_iter().cloned().collect();
    // Update cache (silently drop if already set — racy but fine)
    let _ = SKILL_CACHE.set((Instant::now(), skills.clone()));
    Ok(skills)
}
```

TTL of 30 seconds means:
- Fast re-opens within 30s get instant cache hit
- Long-lived sessions refresh every 30s, catching skills added via other tabs
- No stale data for more than 30s

### 2. Redux Cache — `app/src/features/chat/chatSlice.ts`

Extend `ChatState`:

```typescript
interface ChatState {
  // ... existing fields
  skillsCache: {
    skills: SkillManifest[];
    loadedAt: number; // Date.now() timestamp
  } | null;
}
```

Add:

```typescript
// Action
cacheSkills(state, action: PayloadAction<SkillManifest[]>)

// Async thunk — with cache-before-fetch
export const fetchSkills = createAsyncThunk(
  'chat/fetchSkills',
  async (_, { getState, dispatch }) => {
    const state = getState() as { chat: ChatState };
    const cached = state.chat.skillsCache;
    if (cached && Date.now() - cached.loadedAt < 25000) {
      return cached.skills; // skip fetch if cache fresh
    }
    const skills = await invoke<SkillManifest[]>('get_skills');
    dispatch(cacheSkills(skills));
    return skills;
  }
);
```

25s client timeout (< 30s backend TTL) ensures we don't stack two fetches.

### 3. `useSkills` Hook — `app/src/hooks/useSkills.ts`

```typescript
export function useSkills() {
  const dispatch = useAppDispatch();
  const skillsCache = useSelector((state: RootState) => state.chat.skillsCache);

  useEffect(() => {
    if (!skillsCache || Date.now() - skillsCache.loadedAt > 25000) {
      dispatch(fetchSkills());
    }
  }, []);

  return {
    skills: skillsCache?.skills ?? [],
    loadedAt: skillsCache?.loadedAt ?? null,
    loading: !skillsCache,
    refresh: useCallback(() => dispatch(fetchSkills()), [dispatch]),
  };
}
```

### 4. `SkillSelector` Component — `app/src/components/chat/SkillSelector.tsx`

**Structure** (follows ModelSelector pattern):

```
SkillSelector (div, ref-based, outside-click dismiss)
├── Trigger: WandIcon button (icon-btn class, size=16)
│   └── Badge: skill count (if > 0)
└── Dropdown (.model-dropdown, bottom:100%)
    ├── Header (.model-dropdown-header)
    │   ├── Refresh button (RefreshIcon, size=14)
    │   └── Search row (.model-dropdown-search)
    │       ├── SearchIcon (size=14)
    │       └── input (auto-focus on open)
    └── List (.model-dropdown-list)
        ├── Recent skills section (if any)
        │   └── per recent skill:
        │       ├── ClockIcon (size=12)
        │       ├── skill.name (semibold)
        │       └── skill.description (text-dim, 11px, ellipsis)
        ├── All skills section
        │   └── per skill:
        │       ├── ZapIcon (or WandIcon, size=14)
        │       ├── skill.name (semibold)
        │       └── skill.description (text-dim, 11px, ellipsis)
        ├── Loading state: "Loading skills..."
        ├── Empty state: "No skills installed" + link to settings
        └── Error state: error text + "Retry" button
```

**Behavior**:
- Click wand icon → toggle dropdown
- Cmd/Ctrl+K → open dropdown (global shortcut)
- Search filters by name, description, triggers (case-insensitive)
- Arrow keys navigate items (up/down/enter/escape — standard listbox pattern)
- Click item or Enter → close dropdown, call `onSelect(skill)`, add to recent skills
- Outside click → close dropdown
- Re-opening after close re-uses cached skills (no new Tauri call)
- Refresh button → force cache invalidation and re-fetch
- Recent skills stored in localStorage (max 5)

**Props**:
```typescript
interface SkillSelectorProps {
  onSelect: (skill: SkillManifest) => void;
}
```

### 5. ChatInput Integration — `app/src/components/chat/ChatInput.tsx`

```tsx
<div className="input-actions-left">
  <button className="icon-btn" onClick={handlePlusClick}>
    <PlusIcon size={16} />
  </button>
  <SkillSelector onSelect={handleSkillSelect} />
</div>
```

`handleSkillSelect` inserts `@skill:name ` at cursor:

```typescript
const handleSkillSelect = useCallback((skill: SkillManifest) => {
  setInput((prev) => {
    const el = textareaRef.current;
    const cursorPos = el?.selectionStart ?? prev.length;
    const before = prev.slice(0, cursorPos);
    const after = prev.slice(cursorPos);
    return `${before}@skill:${skill.name} ${after}`;
  });
  // Restore cursor position after state update
  requestAnimationFrame(() => {
    const el = textareaRef.current;
    if (el) {
      const insertLen = `@skill:${skill.name} `.length;
      const cursorPos = el.selectionStart;
      const newPos = cursorPos + insertLen;
      el.setSelectionRange(newPos, newPos);
      el.focus();
    }
  });
}, [setInput, textareaRef]);
```

### 6. CSS — `app/src/styles/skill-selector.css`

```css
.skill-selector-wrapper {
  position: relative;
}

.skill-item-name {
  font-weight: 600;
  font-size: 13px;
}

.skill-item-description {
  font-size: 11px;
  color: var(--text-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 260px;
}
```

Reuses `.model-dropdown`, `.model-dropdown-search`, `.model-dropdown-list`, `.model-dropdown-item` from `model-dropdown.css` (same as ModelSelector).

### 7. Cache Hit Optimization — Three-Layer Architecture

```
User clicks wand icon
    │
    ▼
SkillSelector opens
    │
    ├── Redux cache hit? (loadedAt < 25s ago)
    │      └── YES → show cached skills instantly
    │
    ├── Redux cache miss → dispatch(fetchSkills())
    │      │
    │      ├── Backend cache hit? (OnceLock < 30s old)
    │      │      └── YES → return cached Vec<SkillManifest> (no filesystem scan)
    │      │
    │      └── Backend cache miss → SkillManager::scan() filesystem
    │             └── Update OnceLock + Redux cache
    │
    ▼
Dropdown renders with skill list
```

**Why three layers**:
| Layer | TTL | Purpose |
|-------|-----|---------|
| Redux (component) | 25s | Prevents re-dispatch on quick re-opens (component unmount/remount, tab switches) |
| Rust OnceLock | 30s | Prevents redundant filesystem scans within same process lifetime |
| Filesystem | — | Ground truth; scanned only when both caches expire |

## Tasks

| ID | Task | Owner | Status | ETA |
|----|------|-------|--------|-----|
| T1 | Add `OnceLock` TTL cache to `get_skills` in `app/src-tauri/src/lib.rs` | agent_core | Todo | 2026-06-28 |
| T2 | Add cache invalidation command `invalidate_skills_cache` in `app/src-tauri/src/lib.rs` | agent_core | Todo | 2026-06-28 |
| T3 | Extend `chatSlice.ts` — add `skillsCache` state, `cacheSkills` reducer, `fetchSkills` thunk | agent_core | Todo | 2026-06-28 |
| T4 | Create `app/src/hooks/useSkills.ts` with cache-aware hook | agent_core | Todo | 2026-06-28 |
| T5 | Create `app/src/components/chat/SkillSelector.tsx` dropdown component with refresh button, badge, recent skills | agent_core | Todo | 2026-06-28 |
| T6 | Create `app/src/styles/skill-selector.css` with skill item styles | agent_core | Todo | 2026-06-28 |
| T7 | Integrate `SkillSelector` into `ChatInput.tsx` — icon button + selection handler | agent_core | Todo | 2026-06-28 |
| T8 | Add keyboard shortcut (Cmd/Ctrl+K) to open SkillSelector | agent_core | Todo | 2026-06-28 |
| T9 | Add accessibility enhancements (ARIA labels, keyboard navigation) | agent_core | Todo | 2026-06-28 |
| T10 | Update `docs/index.md` with PLAN-0006 entry | agent_core | Todo | 2026-06-28 |

## Milestones

| Milestone | Description | Target Date |
|-----------|-------------|-------------|
| M1 | Backend cache + Redux cache + hook complete | 2026-06-28 |
| M2 | SkillSelector component + CSS complete | 2026-06-28 |
| M3 | ChatInput integration + full flow test | 2026-06-28 |
| M4 | Verify no regression — existing skills still work, cache layers validated | 2026-06-28 |

## Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| 30s cache TTL shows stale skills list (user installed a new skill but it's not visible) | Low | Low | User can force refresh by re-opening dropdown after 30s; also add a manual refresh button in the dropdown |
| `@skill:name` format conflicts with existing `@file` mention parsing | Medium | Low | Already supported — `parseMentions()` uses `@[^\s]+` pattern which matches both `@folder/` and `@skill:name`. No conflict. |
| SkillSelector component adds layout shift to the toolbar | Low | Low | Use fixed-size icon button (same as Plus button). Dropdown is absolutely positioned. |
| SkillManager::with_defaults() fails on first call (e.g. dirs not created yet) | Low | Low | Error state in dropdown handles this gracefully — shows error + retry button |

## Success Criteria

- [ ] Clicking the wand icon opens a dropdown with all installed skills (verified with 5+ installed skills)
- [ ] Search filters skills by name, description, and triggers
- [ ] Selecting a skill inserts `@skill:name ` at the cursor in the textarea
- [ ] Cache proven effective: 2nd+ open of dropdown within 30 seconds makes NO backend Tauri call
- [ ] Redux cache persists across component remounts (tab switching)
- [ ] Error state shows when backend is unavailable
- [ ] Empty state shows when no skills are installed
- [ ] All existing tests pass (294/294)
- [ ] No visual regression in the input toolbar area

## References

- `app/src-tauri/src/lib.rs` — existing `get_skills` command
- `app/src/components/chat/ModelSelector.tsx` — reference pattern for searchable dropdown
- `app/src/components/settings/SkillsTab.tsx` — existing skills list in settings
- `app/src/hooks/useAutocomplete.ts` — existing @-mention system
- `app/src/styles/model-dropdown.css` — shared dropdown CSS
- `PLAN-0005` — prior caching pattern (OnceLock + TTL)

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-28 | agent_core | Created as Draft |

---
*Generated by AI Agent (agent_core)*
*Model: gpt-4o | Timestamp: 2026-06-28T19:48:00+08:00*
