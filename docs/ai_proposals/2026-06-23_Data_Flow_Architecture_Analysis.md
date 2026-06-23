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
