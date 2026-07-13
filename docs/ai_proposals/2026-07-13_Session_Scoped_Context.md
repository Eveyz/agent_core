# 2026-07-13 — Session-scoped agent context (todos / skills / recall)

Date: 2026-07-13

## Problem

Chat sessions shared Brain-level ephemeral context across sessions:

1. A single global `todo_list` was injected into every Run's EXECUTION PLAN
2. `SkillManager.active_skills` was global — skills loaded in session A stayed in session B
3. Recall auto-inject searched the whole `recall_memory` table with no session filter

Conversation message history was already per-session; the leak was in injected context segments.

## Fix

| Surface | Change |
|---------|--------|
| Todos | `SessionTodoStore` on Brain — keyed by `session_id`; same session multi-Run still shares |
| Skills | `active_by_scope: HashMap<session, HashSet>` — tools use injected `_session_id` |
| Recall auto-inject | Retain only records matching the current `session_id` (over-fetch then filter) |
| Cleanup | `delete_session` / `clear_session_goal` clear that session's todos (+ skills on delete) |

## Intentionally still global

- Core Memory blocks (user traits / preferences)
- Global `~/.agverse/agverse.md`
- Explicit `conversation_search` tool (cross-session on purpose when the model asks)

## Key files

- `core/src/todo/mod.rs` — `SessionTodoStore`
- `core/src/runtime/brain.rs` — `todo_lists`
- `core/src/skills/mod.rs` — session-scoped active skills
- `core/src/tools/skill.rs` — `_session_id` aware
- `core/src/runtime/run/{context,lifecycle,turn,mod}.rs`
- `app/src-tauri/src/lib.rs` — clear/remove on goal clear & session delete
