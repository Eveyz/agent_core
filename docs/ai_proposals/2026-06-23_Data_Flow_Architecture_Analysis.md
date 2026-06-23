# 2026-06-23 — Data Flow Architecture Analysis & Event-ID Audit

> Scope: trace the full data path from user input → LLM → React/Redux →
> components (incl. subagents), identify where events fail to match their
> target component, and propose architecture upgrades so every event has a
> stable ID and routes deterministically.

---

## 1. End-to-end data flow (current)

```
ChatInput (React)
   │ handleSend
   ▼
App.tsx ── dispatch(userMessageSent) ──► Redux (push user entry, isProcessing=true)
   │ invoke('send_message', {message, sessionId})
   ▼
Tauri send_message (src-tauri/src/lib.rs)
   │ RunManager.create_run() → run_id
   │ manager.subscribe(run_id) → broadcast::Receiver<RunEvent>
   │ manager.command(run_id, Start)
   │ tokio::spawn: loop { rx.recv() → app_handle.emit("agent-event", &event) }
   ▼
Run::run() (core/src/runtime/run.rs)
   │ emit RunStarted → run_loop → run_turn:
   │   model_turn → collect_stream ──► event_tx.send(ModelStreaming{None,delta})   (*1 not logged*)
   │   no tools  → emit MessageEnd, emit TurnEnded
   │   tools     → emit MessageEnd → execute_tools:
   │                  orchestrator emit ToolExecutionStart → from_agent_event
   │                    → event_tx.send(ToolStarted{None})                          (*1 not logged*)
   │                  [subagent tool] drain internal AgentEvents → from_agent_event
   │                    → SubagentStarted / ModelStreaming{Some} / ToolStarted{Some}
   │                      / ToolEnded{Some} / SubagentEnded                         (*1 not logged*)
   │                  (orchestrator does NOT emit ToolExecutionEnd)
   │                → emit ToolEnded{None} per top-level call, emit TurnEnded
   ▼
Single Tauri channel "agent-event"  (ALL RunEvents, one stream)
   │ listen() → rAF coalesce buffer
   ▼
Redux chatSlice.agentEventReceived
   │ if raw.event is string → runEventToAgentEvent (RunEvent → legacy AgentEvent)
   │ route by subagent_id presence:
   │    None  → main turn blocks        (getActiveTurn = last open turn)
   │    Some  → turn.subagents[id] blocks
   ▼
AgentTurn.tsx → groupBlocksIntoItems → SubagentCard (looks up entry.subagents[id])
```

`(*1)` Events sent via raw `event_tx.send()` bypass `Run::emit()`, so they are
**not** appended to `EventLog`. Only `emit()`'d events are persisted.

---

## 2. The matching model (how an event reaches a component)

Every event is routed by **two implicit keys**, neither of which is a global ID:

| Key              | Carried on                          | Used for                              |
|------------------|-------------------------------------|---------------------------------------|
| `subagent_id`    | ModelStreaming, Tool*, Approval*, Subagent* | main-vs-subagent branch         |
| `call_id`        | ToolStarted/Update/Ended            | find tool block by id (first match)   |
| "last open turn" | (none — positional)                 | which turn entry receives the block   |

`getActiveTurn(state)` = the last entry with `type==='turn' && !endTime`.
ALL handlers (main + subagent) write into whatever this returns. A turn is
closed **only** by `run_completed → AgentEnd`. `TurnEnded` is converted but
has **no handler**, so it never closes a turn.

---

## 3. Confirmed bugs / weak points

### B1. No global event ID (your core concern)
`RunEvent` carries `call_id`, `prompt_id`, `subagent_id`, `run_id` — but **no
per-event unique id** and no monotonic sequence number.
Consequences:
- No dedup if an event is delivered twice.
- No way to detect a *gap* (missing event) → silent mis-match.
- `EventLog` entries have no stable key → "replay / fork / audit" (advertised
  in `event_log.rs`) cannot reference a specific event.
- Resume/retry cannot reconcile frontend state against the log.

### B2. Broadcast channel drops events on lag (primary cause of mis-match)
Capacity = 1024 (`EVENT_CHANNEL_CAPACITY`). A single `tokio::spawn` task
forwards to Tauri; on `RecvError::Lagged(n)` it `eprintln!`s and **continues**
— those events are gone. Heavy streaming (dozens of `ModelStreaming`/sec) can
overflow 1024 easily. If a dropped event is `SubagentStarted` or `ToolStarted`,
every later update for that id finds no target block and is **silently
dropped** (`turn.subagents[id]` is undefined; `blocks.find(call_id)` returns
undefined). This is the most likely root cause of "data flows to the wrong / no
component."

### B3. Event log is incomplete (streaming + tool/subagent events not logged)
`collect_stream` and the tool-event forwarding closure use `event_tx.send()`
directly, **not** `Run::emit()`. So `ModelStreaming`, `ToolStarted`, and all
`Subagent*` events are never written to `~/.agent_core/runs/{run_id}.jsonl`.
Replay/fork/audit cannot reconstruct a run; session-resume falls back to the
separate saved-messages path.

### B4. RunEvent→AgentEvent shim is lossy (several event types silently no-op)
`runEventToAgentEvent` returns `{}` (no-op) for: `message_start`,
`model_call_started`, `model_call_ended`, `turn_ended`, `approval_resolved`,
`input_requested`, `context_compacted`, `process_spawned`, `process_killed`.
So the UI never sees: turn boundaries (`TurnEnded`), approval resolution from
the backend (`ApprovalResolved`), input requests, or process lifecycle. The
double conversion (new → legacy → handler) is the source of these black holes.

### B5. Positional turn routing via `getActiveTurn`
All blocks attach to "last open turn." Because `TurnEnded` is dropped, a turn
stays open until `run_completed`. If a turn is ever closed early (error path,
or a future multi-turn feature), subsequent subagent events attach to the wrong
turn or are dropped. There is **no `turn_id` on events** — the backend's
`TurnStarted{index}` is stored but never used for routing.

### B6. No explicit wrapper-tool ↔ subagent link
When the main agent calls the `subagent`/`subagents` tool:
- `ToolStarted{subagent_id:None, call_id:W}` (wrapper)
- `SubagentStarted{subagent_id:S}` (no `parent_call_id`)
There is no field tying `S` to `W`. Grouping is purely positional in the blocks
array. Reordering, interleaving, or multiple wrapper calls in one turn break
the visual/logical grouping.

### B7. `Subagent::new` ignores its first arg as an id
`Subagent::new(role_name, …)` sets `self.id = Uuid::new_v4()` and treats the
first arg as `role_name`. `spawn_single` passes the task-spec `id` as that
arg, so the **model-specified id becomes role_name**, and the real
`subagent_id` is an unrelated UUID. No collision, but the task-spec id ≠
subagent_id, and the "emit all SubagentStart upfront" comment in
`tools/subagent.rs` is stale (they emit on start, not upfront).

### B8. Duplicate / empty tool call IDs (your `fix_ids.patch`)
The model can return empty or duplicate `tool_call.id`s. `handleToolEnd` uses
`blocks.find(b => b.call_id === toolCallId)` — first match wins, so a
duplicate id closes the **wrong** tool block. Your patch dedups IDs in
`client/mod.rs`; good, but the frontend should also be resilient (key by
`call_id + occurrence`).

### B9. `userMessageSent` does not close a stale open turn
A failed run (`run_failed → Error → handleError`) leaves the turn with no
`endTime`. The next `userMessageSent` pushes a user entry but does not close
it → the failed turn renders as perpetually "open."

### B10. `ApprovalResolved` / `InputRequested` dead in the UI
Backend emits them; frontend drops them. Approval status is updated only by the
local `toolApprovalResponded` action, so server-side / auto approvals are not
reflected. `resolve_input` is a TODO, so `InputRequested` is fully inert.

---

## 4. Architecture upgrade recommendations

### R1. Add a universal event envelope (the ID fix)
Wrap every emission in a typed envelope carrying identity + correlation:

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct Envelope {
    pub seq: u64,                 // monotonic per-Run sequence (the global event id)
    pub event_id: String,         // UUID — stable across transport/log/replay
    pub run_id: RunId,
    pub turn_id: Option<String>,  // explicit turn this event belongs to
    pub parent_call_id: Option<String>, // wrapper tool that spawned a subagent
    pub subagent_id: Option<String>,
    pub event: RunEvent,          // the payload
}
```
- `Run` owns a `seq: u64` counter incremented on every emit (thread-safe).
- Frontend stores `lastSeq` per run; a gap ⇒ request resync (R3).
- `EventLog` keys on `event_id`/`seq` → real replay/fork/audit.
- `SubagentStarted` sets `parent_call_id` from the orchestrator's call_id (R5).

### R2. Lossless transport (fix the drop-on-lag)
Replace the single broadcast→Tauri fan-out with a **per-subscriber mpsc** with
backpressure, OR keep broadcast but:
- raise capacity AND
- add a **resync protocol**: on `Lagged`, the bridge asks the Run for a
  state snapshot (or replays from `EventLog` since `lastSeq`) instead of
  dropping. Streaming deltas can be compacted (send accumulated text), but
  lifecycle/correlation events must never be lost.

### R3. Frontend gap-detection + resync
Track `expectedSeq`. On a gap or `Lagged`, invoke a Tauri command
`replay_since(run_id, from_seq)` that streams missed events from `EventLog`
(R6). This self-heals B2/B5.

### R4. Kill the RunEvent↔AgentEvent shim
Handle `RunEvent` directly in the reducer. Delete `runEventToAgentEvent` and
the legacy `AgentEvent` branch. Add explicit handlers for the currently
no-op'd types (`turn_ended`, `approval_resolved`, `input_requested`,
`process_spawned/killed`, `context_compacted`). This removes the B4 black holes.

### R5. Explicit correlation on subagent events
- Add `parent_call_id` to `SubagentStarted` (set by the orchestrator when it
  spawns the subagent tool).
- Add `subagent_id` (Option) to `MessageStart`/`MessageEnd` so subagent
  message boundaries are explicit (currently `MessageEnd{Some}` is a no-op).
- Route by `turn_id` + `subagent_id`, not `getActiveTurn`.

### R6. Persist ALL events (complete the EventLog)
Route streaming + tool/subagent events through `emit()` (or a logging
wrapper). This makes replay/fork/audit/R3 real. Consider logging the envelope
(R1) so the log is self-describing.

### R7. Route by ID, not position
- Carry `turn_id` on events; `handleTurnStart` creates/looks-up by `turn_id`.
- `TurnEnded` closes the turn (or marks an iteration boundary) by `turn_id`.
- Subagent events look up `entry.subagents[id]` by id (already do) but the
  *entry* is found by `turn_id`, not "last open turn."

### R8. Frontend: normalized event store
Consider a flat, id-keyed store (entities: turns, subagents, tool-calls,
approvals) with selectors composing the view — instead of nested
`turn → blocks + subagents → blocks` with positional append. Easier to test,
dedup, and reconcile against R3.

### R8b. Make tool-block matching resilient
Key tool blocks by `(call_id)` but tolerate duplicates: when a second
`ToolStarted` arrives with an existing `call_id`, open a new block instead of
reusing. Or include the orchestrator's `seq` in the block key.

---

## 5. Suggested rollout order (smallest blast radius first)

1. **R1 envelope + seq** (backend-only, additive) — unblocks everything.
2. **R6 log-all-events** — makes the system observable & enables R3.
3. **R3 resync on lag** — kills the silent-drop mis-match (B2).
4. **R4 kill shim + handle dropped types** — removes black holes (B4/B10).
5. **R5/R7 explicit correlation + turn_id routing** — fixes B5/B6.
6. **R2 lossless transport** — defense in depth once resync exists.
7. **R8 normalized store** — larger refactor, do last with tests.

Each step is independently shippable and each removes a concrete class of the
"event doesn't reach its component" failures.

---

## 6. File reference map

| Layer        | File                                          | Role |
|--------------|-----------------------------------------------|------|
| Event types  | `core/src/runtime/event.rs`                   | `RunEvent` enum, `from_agent_event` shim |
| Run loop     | `core/src/runtime/run.rs`                     | emit/streaming/tool dispatch |
| Orchestrator | `core/src/agent/executor.rs`                  | tool event channel + drain |
| Subagent     | `core/src/subagent/mod.rs`                    | `Subagent::run_with_sender`, AgentEvent emission |
| Subagent tool| `core/src/tools/subagent.rs`                  | `spawn_single`, concurrent batch |
| Event log    | `core/src/runtime/event_log.rs`               | JSONL persistence (incomplete) |
| Manager      | `core/src/runtime/manager.rs`                 | broadcast channel (cap 1024), subscribe |
| Bridge       | `app/src-tauri/src/lib.rs`                    | `send_message`, emit("agent-event"), lag-drop |
| Listener     | `app/src/hooks/useAgentEventListener.ts`      | rAF coalesce → dispatch |
| Redux        | `app/src/features/chat/chatSlice.ts`          | routing, `getActiveTurn`, `runEventToAgentEvent` |
| Render       | `app/src/components/chat/AgentTurn.tsx`       | `groupBlocksIntoItems`, `SubagentCard` |

---

## 7. Adjusted Execution Plan (post-review)

Combines the Subagent UI redesign with the architecture upgrades (R4 + R8).
Three stages; stages 0 and 1 ship together, stage 2 follows, stage 3 is
follow-up backend work.

> Adjustments vs. the original proposal (R4+R8 only):
> - Lifted **minimal `seq` envelope** (R1 core) and **B9 fix** ahead of
>   Phase 1 — both are tiny and directly treat the reported symptoms; they
>   also let the new reducer gap-detect from day one and keep the temporary
>   `getActiveTurn` bridge from misattaching to a failed turn.
> - Expanded Phase 1 scope to include the **save/resume rewrite** — the whole
>   persist/restore chain depends on the nested `entries` structure.
> - Phase 2 must **preserve approvals + abort** in the drill-down view.

### Stage 0 — Backend groundwork (do first, minimal, symptom-fixing)

1. **Add monotonic `seq` + `event_id` envelope to `RunEvent` emissions.**
   `Run` owns a `seq: u64` counter incremented on every emit (thread-safe).
   Frontend reducer tracks `lastSeq`; a gap ⇒ resync hook (full resync is
   Stage 3, but detection lands now). This unblocks everything and is the
   prerequisite for killing silent drop-on-lag (B2).

2. **Fix B9: close failed turns.** `handleError` / `run_failed` sets
   `endTime` on the active turn so the temporary `getActiveTurn` bridge
   never attaches subagent events to a perpetually-open failed turn.

### Stage 1 — Frontend normalization (R8) + direct event handling (R4)

3. **Flatten the Redux store (`chatSlice.ts`):**
   - `turns: Record<string, ChatEntry>`
   - `subagents: Record<string, SubagentEntry>` (global dictionary)
   - `turnSubagentMap: Record<string, string[]>` (turn → its subagents)
   - reverse `subagentTurn: Record<string, string>` (subagent → owning turn)
   - `viewingSubagentPath: { id: string; name: string }[]` (drill-down depth)

4. **Refactor `agentEventReceived` to consume `RunEvent` directly**, delete
   `runEventToAgentEvent` and the legacy `AgentEvent` branch. Add explicit
   handlers for the previously no-op'd types (`turn_ended`, `approval_resolved`,
   `input_requested`, `process_spawned/killed`, `context_compacted`). Removes
   the B4 black holes and gives direct access to `run_id` / `subagent_id` /
   `seq`.

5. **Rewrite the save/resume chain** to the flat shape: `entriesToMessages`,
   `entriesToEventLog`, `cacheCurrentSession`, `restoreOrClearSession`, and
   the `resumeSession` rebuild logic. Without this, autosave / session switch
   / resume break after the flatten. `selectEntryById` and
   `selectPendingApprovalCount` must also be updated (approvals now span both
   turns and the global subagents dict).

### Stage 2 — UI redesign (widgets & drill-down)

6. **Compact subagent widget.** Delete the inline `<AgentTurnUI />`
   expansion from `SubagentCard`. Widget shows `role_name`, status
   (spinner/check), `toolCount`, elapsed time, and a "View Details →" target.
   All live subagents render together; selection (not just the viewed one)
   drives display.

7. **Header breadcrumbs (`App.tsx`).** Replace the static session name with a
   dynamic breadcrumb (`Session Title > weather-shenzhen > …`) backed by
   `viewingSubagentPath`. Include an **abort entry point** so a run can be
   stopped from the drill-down while the main input is hidden.

8. **Secondary subagent page view (`App.tsx`).** When
   `viewingSubagentPath.length > 0`: hide main chat history + chat input.
   Render a simulated `UserRow` with the subagent's `task` for context, then
   the subagent's `blocks` via a modified `AgentTurnUI`. **Approval blocks
   must still render and be actionable** (`toolApprovalResponded` +
   `invoke('approve_tool')`); subagent approvals resolve via the global
   pending map (`src-tauri/src/lib.rs` `approve_tool`), so only UI placement
   changes.

### Stage 3 — Backend correlation (R1 full, R5, R7) *[follow-up]*

9. Full envelope + lossless transport: attach `turn_id`/`parent_call_id` to
   all events (R5/R7) so the frontend stops guessing the active turn and can
   explicitly link a wrapper tool block to its subagent. Add `replay_since`
   resync to self-heal B2. Then retire the `getActiveTurn` temporary bridge.

### Rollout order

Stage 0 → Stage 1 (steps 3-5 together) → Stage 2 (steps 6-8 together) →
Stage 3. Each stage is independently shippable.

---

## 8. Implementation Status (2026-06-23)

Stage 0, Stage 1, and Stage 2 are implemented and verified. Stage 3 remains
future backend work.

### Stage 0 — Backend groundwork ✅
- **R1 envelope**: `Envelope { seq, event_id, run_id, #[serde(flatten)] event }`
  in `core/src/runtime/event.rs`. `Run` owns an `Arc<AtomicU64>` seq counter
  (`wrap()`), shared with `RunManager` so `RunCreated` = seq 0 and the Run's
  events form one monotonic sequence. Every emission site (`emit`, `transition`,
  `collect_stream`, the tool-forwarding closure, manager's `RunCreated`) now
  sends an `Envelope`. `#[serde(flatten)]` keeps the on-the-wire `{event, seq,
  event_id, run_id, ...}` shape backward-compatible (the `event` tag stays
  top-level). Verified by 2 new `event::tests`.
- **B9**: failed runs close their active turn (`run_failed` → `handleError` +
  `endTime` + `stopDanglingSubagents`), so `getActiveTurn` no longer misattaches
  to a perpetually-open failed turn.
- Frontend gap-detection: `lastSeqByRun` + `console.warn` on seq gaps (resync is
  Stage 3).

### Stage 1 — Frontend normalization (R8) + direct RunEvent (R4) ✅
- **Flatten**: subagent data moved out of the turn tree into a global
  `state.subagents: Record<string, SubagentEntry>` dict. `ChatEntry.subagents`
  → `subagentIds: string[]`. Ownership tracked via `subagentIds` on the turn;
  `subagentsBySession` caches subagents across session switches.
- **Kill shim**: deleted `runEventToAgentEvent` and the legacy `AgentEvent`
  type/routing. `agentEventReceived` now switches on `RunEvent.event` directly.
  Previously-dropped `turn_ended` and `approval_resolved` now route
  (`handleTurnEnded` finalizes streaming; `resolveApprovalBlock` reflects
  server/auto approvals). `run_completed`/`run_cancelled` call `handleAgentEnd`;
  `run_failed` displays the error + closes the turn.
- **Save/resume chain** rewritten for the flat shape: `entriesToEventLog(entries,
  subagents)`, `resumeSession` rebuilds the global dict, `toolApprovalResponded`
  + `selectPendingApprovalCount` scan the global dict. Callers
  (`useAutoSaveSession`, `Sidebar`) updated.

### Stage 2 — UI redesign ✅
- **Compact subagent widget**: `SubagentCard` no longer inlines an
  `AgentTurnUI`; it shows role/status/toolCount/elapsed + a "View Details →"
  button that dispatches `viewSubagent`. `AgentTurnUI` reads the global dict via
  `useSelector` and threads it to `TurnIterationUI` for `subagent_ref` blocks.
- **Breadcrumbs**: header shows `Session › subagentName › …`; clicking a
  segment pops back, clicking the session name clears the view.
- **Secondary page**: when `viewingSubagentPath.length > 0`, main chat + input
  hide; a `SubagentDetailPage` renders the subagent's task (simulated `UserRow`)
  + its blocks (`AgentRow`/`AgentTurnUI`), with a Back button and a Stop
  (abort) control while processing. Approvals remain actionable in this view.

### Verification
- `cargo test -p agent_core runtime::` → 37 passed (incl. 2 envelope tests).
- `cargo check --workspace` → clean (pre-existing warnings only).
- `tsc --noEmit` → clean; `npm run build` (tsc + vite) → success.

### Stage 3 — Backend correlation (pending)
- Attach `turn_id` + `parent_call_id` to events (R5/R7) so the frontend stops
  guessing the active turn and can link wrapper tool blocks to subagents.
- Add `replay_since(run_id, from_seq)` resync to self-heal broadcast lag (B2),
  then retire the `getActiveTurn` temporary bridge.

---

## 9. Stage 3 — Backend correlation + resync (2026-06-24) ✅

### turn_id + parent_call_id (R5/R7)
- `Envelope` gained `turn_id: Option<String>` and `parent_call_id:
  Option<String>` (`#[serde(skip_serializing_if)]` so they're omitted when
  `None`). `Run` tracks `current_turn_id`: set to a fresh UUID on each turn
  start, cleared after the turn ends. Every `wrap()` stamps the active
  `turn_id`.
- The tool-forwarding closure now receives `parent_call_id: &str` from the
  orchestrator and stamps it on the emitted envelope, so subagent events carry
  the wrapper tool's `call_id` — the UI can explicitly link a subagent to its
  spawning tool block instead of relying on positional ordering (B6).
- Orchestrator `on_event` signature changed `Fn(AgentEvent)` →
  `Fn(AgentEvent, &str)`; the legacy `agent/mod.rs` path wraps to
  `|ev, _call_id| on_event(ev)`.

### EventLog stores Envelopes (B3 fix)
- `EventLog` now stores `Vec<Envelope>` (was `Vec<RunEvent>`). `Run` no longer
  owns an `EventLog` or calls `event_log.append()` in `emit()`.
- `RunManager` spawns a **logging subscriber** task per Run that persists every
  `Envelope` from the broadcast channel to `~/.agent_core/runs/{run_id}.jsonl`.
  Because it subscribes to the channel (not just `emit()` calls), **streaming
  events and tool/subagent events are now logged too** — fixing B3 (incomplete
  event log). The log task is awaited alongside the state-mirror task on Run
  completion so no events are lost on shutdown.

### replay_since (B2 self-heal)
- `EventLog::replay_since(path, from_seq)` loads envelopes with `seq >
  from_seq`. `RunManager::replay_since(run_id, from_seq)` exposes it; a Tauri
  command `replay_since(run_id, from_seq)` returns the missing envelopes as
  JSON strings.

### Frontend: turn_id routing + gap resync
- `RunEventPayload` + `ChatEntry` gained `turn_id`/`turnId`. The reducer sets
  `state._pendingTurnId = ev.turn_id` before the switch; `getActiveTurn` now
  prefers a turn matching `_pendingTurnId` (falling back to last-open-turn for
  events without a turn_id, e.g. lifecycle). This retires positional guessing
  for turn-scoped events (B5).
- On a seq gap, the reducer dispatches an `agent-event-gap` `CustomEvent`;
  `useAgentEventListener` catches it and dispatches `resyncRun({runId,
  fromSeq})`, which invokes `replay_since` and re-feeds each missing envelope
  through `agentEventReceived`. A `resyncing` flag prevents re-entrancy. This
  self-heals broadcast-lag drops (B2) — the primary cause of "event doesn't
  reach its component."

### Verification
- `cargo test -p agent_core` → 266 unit + 11 integration passed.
- `cargo check --workspace` → clean.
- `tsc --noEmit` → clean; `npm run build` → success.

All stages (0–3) of the plan are now complete.
