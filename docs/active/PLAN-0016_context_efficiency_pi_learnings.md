---
id: PLAN-0016
type: PLAN
title: Context Efficiency (Pi Learnings)
status: Implemented
author: agent_core
created: 2026-07-23
updated: 2026-07-23
reviewers: [zniverse]
related: [PLAN-0008, PLAN-0014]
supersedes: ~
superseded_by: ~
tags: [context, truncation, compaction, tool-spill, rolling-summary]
---

# PLAN-0016: Context Efficiency (Pi Learnings)

## Objective

Adopt Pi-style context efficiency without duplicating live todo/execution progress:

1. Tail-heavy incidental truncation (errors usually at the end of shell logs).
2. Spill oversized incidental tool output to disk; model sees truncation + path.
3. Bounded RollingSummary with incremental merge + deterministic file ledger (`read` / `wrote` / `deleted`).
4. Do **not** put Goal/Progress checklists in the summary — Segment 7 todo + `EXECUTION STATE` remain the source of truth.

## Design summary

### Tool result path

- Live UI: full body via `ToolEnded`.
- Persist / model window: if incidental and over the dual budget (`INCIDENTAL_MAX_LINES` =
  2000 **or** `INCIDENTAL_MAX_CHARS` = 50KB, whichever first), write
  `~/.agverse/sessions/<id>/tool_spills/<call_id>.txt` (or global fallback), store
  tail-heavy truncation + `read_file` hint.
- L2 hygiene and L3 `snip_compact` still share [`hygiene::policy`](../../core/src/hygiene/policy.rs).

### RollingSummary

Schema (JSON delta from LLM; files primarily from ledger):

```json
{
  "goal": "optional one-liner",
  "decisions": [],
  "files": { "read": [], "wrote": [], "deleted": [] },
  "errors_open": [],
  "facts": [],
  "notes": []
}
```

Model-window message prefix: `[RollingSummary]`.

`maybe_compact` / `force_compact`:

1. Upsert `FileLedger` into RollingSummary.
2. Prefer `chunked_drop` (cache-friendly).
3. Else LLM delta → `merge_summary` with ledger.
4. Else `micro_compact` + ledger upsert.

## Implementation map

| Piece | Location |
|-------|----------|
| Tail-heavy policy | `core/src/hygiene/policy.rs` |
| Spill helper | `core/src/hygiene/spill.rs`, `core/src/paths.rs` |
| Ingest wiring | `core/src/runtime/run/turn.rs` |
| File ledger | `core/src/runtime/file_ledger.rs` |
| RollingSummary / merge | `core/src/compressor.rs` |
| Compact wiring | `core/src/runtime/run/compact.rs` |

## Success criteria

- [x] Long shell failure logs in the model window are tail-heavy under ≤2000 lines / ≤50KB with spill path.
- [x] RollingSummary stays bounded across merges and lists touched files without a Progress checklist.
- [x] Dual-track invariant unchanged (full transcript not compacted).
- [x] L2 = L3 truncation policy sharing preserved.

## Status

Implemented 2026-07-23.
