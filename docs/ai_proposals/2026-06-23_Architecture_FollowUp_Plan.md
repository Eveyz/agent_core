# 2026-06-23_Architecture_FollowUp_Plan.md

## Overview
The main architecture refactoring plan (`2026-06-23_Data_Flow_Architecture_Analysis.md`) is excellent and resolves the majority of the systemic issues. Specifically, it perfectly solves:
- **Accidental Complexity**: By flattening the Redux store (R8), killing the event shim (R4), and using explicit IDs (R7).
- **Robustness (Drop on Lag)**: By introducing monotonic sequence numbers (`seq`), event envelopes (R1), and gap-detection/resync (R3).

However, my previous architectural review identified two critical areas that are **not fully addressed** by the current execution plan. This follow-up plan proposes solutions for them.

---

## 1. Missing Piece: IPC Overload & Streaming Performance
**The Problem:**
Currently, every tiny text chunk emitted by the LLM stream triggers an immediate `RunEvent` over the Tauri IPC channel. While the frontend uses `requestAnimationFrame` to batch React renders, the IPC channel and the Tokio runtime are still flooded with thousands of tiny messages during high-concurrency subagent tasks. The current plan mentions compaction but doesn't define a mechanism.

**The Solution: Backend Token Accumulator / Debouncer**
Instead of forwarding every `TextDelta` immediately, we should implement a time-and-size based accumulator in the Rust backend (`core/src/client/streaming.rs` or `manager.rs`).
- **Mechanism**: The backend buffers incoming text tokens. It flushes the buffer and emits a single consolidated `TextDelta` event if:
  1. `N` milliseconds have passed since the last flush (e.g., 50ms, which is 20fps, smooth enough for human eyes).
  2. The stream explicitly completes (`Done`).
- **Impact**: This reduces IPC traffic and `AgentEvent` dispatches by up to 90% without noticeably impacting perceived latency.

---

## 2. Missing Piece: Deterministic Error Lifecycle Guarantees (RAII)
**The Problem:**
The previous bug where the "Thinking..." spinner spun endlessly was caused by an unexpected backend error (e.g., a network timeout) returning `Err(?)` early. Because the Rust function returned early, the code that emits `SubagentEnd` or `ToolEnded` was skipped. The current plan's "Stage 0 (Fix B9)" patches this *on the frontend* by forcefully closing dangling subagents when the main agent errors. This is a reactive band-aid. 

**The Solution: Rust `EventGuard` (RAII Pattern)**
The backend state machine must guarantee that every `Start` event is paired with an `End` event, even if the thread panics or returns an `Err`.
- **Mechanism**: Implement an `EventGuard` struct in Rust. 
  ```rust
  // Conceptual implementation
  struct EventGuard<'a> {
      id: String,
      tx: &'a EventSender,
      completed: bool,
  }
  impl Drop for EventGuard<'_> {
      fn drop(&mut self) {
          if !self.completed {
              // Automatically emit End(Error) if dropped prematurely
              let _ = self.tx.send(AgentEvent::SubagentEnd { ... success: false });
          }
      }
  }
  ```
- **Usage**: Whenever `RunStarted`, `ToolStarted`, or `SubagentStarted` is emitted, an `EventGuard` is instantiated. If the function completes successfully, we call `guard.complete()` and emit the normal `End` event. If the function returns `?` early or panics, Rust automatically calls `drop()`, which emits the `End(Error)` event.
- **Impact**: Zero possibility of orphaned spinners or dangling states on the frontend. The backend guarantees a closed state machine structurally.

---

## Execution Recommendation
These two improvements act as "Stage 4" of the ongoing architecture refactoring.
- **IPC Debouncing** should be implemented when we notice CPU spikes or lag during heavily concurrent subagent executions.
- **RAII Event Guards** should be implemented alongside Stage 3 (Backend Correlation) when we touch the backend event emission code.

---

## 10. Implementation Status (2026-06-24) ✅

Both follow-up improvements are implemented and verified.

### 1. IPC Token Accumulator / Debouncer ✅
- `TokenAccumulator` in `core/src/client/streaming.rs`: batches text/thinking
  deltas, flushing on 50ms elapsed (≈20fps) or 256 chars accumulated, plus a
  `force_flush()` on stream end. Text and thinking are tracked separately so
  they never mix.
- Wired into both `Run::collect_stream` (`runtime/run.rs`) and
  `Subagent::collect_stream` (`subagent/mod.rs`). Each delta is pushed into the
  accumulator instead of emitted immediately; flushes emit consolidated
  `ModelStreaming`/`SubagentMessageUpdate` events.
- A final `force_flush()` after the stream loop ensures no text is lost.
- 5 new unit tests cover empty-flush, drain, text/thinking separation, and
  size/time thresholds.

### 2. RAII EventGuard ✅
- `EventGuard<E>` in `core/src/runtime/guard.rs`: holds an `on_incomplete`
  closure fired in `Drop` **only if** `complete()` was never called. 3 unit
  tests cover drop-without-complete, complete-suppresses-drop, and early-return
  (`?`) triggers drop.
- **Subagent lifecycle**: `Subagent::run_with_sender` constructs a guard right
  after `SubagentStart`. The guard's closure emits `SubagentEnd{success:false}`
  — so any `?` early return (stream error, `collect_stream` failure) or panic
  guarantees the frontend gets a terminal event. Success paths call
  `guard.complete()` before the explicit `SubagentEnd` emit.
- **Tool lifecycle**: `Run::run_turn` constructs a per-tool-call guard before
  `execute_tools`. If the tool-execution stage panics or is aborted mid-flight
  (skipping the `ToolEnded` loop), each guard's `Drop` emits a
  `ToolEnded{is_error:true}` so no tool block spins forever. The normal path
  disarms all guards after the observe loop.

### Verification
- `cargo test -p agent_core` → 274 unit (8 new) + 11 integration passed.
- `cargo check --workspace` → clean.
- `tsc --noEmit` → clean; `npm run build` → success.
