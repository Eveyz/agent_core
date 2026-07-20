# PLAN-0014: Dual-track transcript (UI full + model window)

**Status:** Done  
**Date:** 2026-07-20  
**Tags:** context, compaction, session, resume

## Goal

Keep UI / SQLite history full forever while the in-memory model context may be compacted at 80%. Resume always loads the full transcript into a new Run, then `maybe_compact` rebuilds the model window.

## Semantics

| Track | Storage | Mutated by compact? |
|-------|---------|---------------------|
| **Full transcript** | `Run.full_transcript` → `context_snapshot` → SQLite `session_messages` / crash `.messages.json` | No |
| **Model window** | `ContextEngine.messages` | Yes (`chunked_drop` / LLM summary / `micro_compact`) |

- Context Usage ring reads **model window** (`usage_snapshot`).
- UI resume / `rebuildEntries` reads **full** (via `resume()` → prompts/messages).
- LLM `[Compressed turns…]` summaries exist only in the model window and must never be written to the canonical store.

## Explicit non-goals

- No separate `model_window` SQLite cache / “restore from cache” toggle.
- No rebuild of UI from event JSONL.
- No change to the 80% threshold or compact algorithms.
- Sessions already overwritten by pre-fix compacted snapshots cannot be recovered.

## Implementation notes

- All real conversation writes go through `Run::append_conversation`.
- `refresh_context_snapshot` persists full; `refresh_usage_snapshot_only` updates the ring after compact.
- `RunEvent::ContextCompacted` is emitted for observability (does not prune UI entries).
