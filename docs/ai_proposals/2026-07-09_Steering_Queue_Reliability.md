# Proposal: Steering Queue Reliability and UI Polish

Date: 2026-07-09

## Goal
Improve the steering queue experience to make it "smooth and seamless." This ensures that steering messages can be injected between model requests/turns, and that the UI state correctly cleans up injected or cancelled steer cards without leaving orphaned items in the queue.

---

## 1. Problem Identification

### A. Orphaned Steering Queue Badges & Cards
* **Cause**: When a run finishes (completed, failed, or cancelled), the frontend clears `runId` (setting it to `null`). 
* **Effect**: Any remaining queued steer messages are left in the Redux state (`steerQueue` and `entries`). Clicking the `x` (close) button calls `cancel_steer` which returns early if `runId` is null, leaving the user with no way to clear the badge or card.

### B. Duplicate Render of Steered Messages
* **Cause**: When a steer message is injected, the backend adds it to context. Upon session save, it is saved as a standard user message. The next session sync/rebuild adds it as a normal `UserRow` bubble.
* **Effect**: The local orange steer card (which is kept in `entries` under status `"injected"`) and the new normal user message bubble both display, causing duplicate text.

### C. Injection Turn Boundaries
* **Cause**: The steer messages are currently popped and injected at turn boundaries in `core/src/runtime/run/turn.rs` (at the start of `run_turn` or end of `run_turn`).
* **Effect**: This works correctly for multi-turn runs (runs that call tools and continue), but if the model finishes with a final text reply (no tools), the run loop terminates.

---

## 2. Proposed Changes

### Frontend State Management

#### Reducer Cleanups (`app/src/features/chat/chatSlice.ts` & `eventHandlers.ts`)
1. **On Run Completion / Termination**:
   * When handling `run_completed`, `run_cancelled`, `run_failed`, or `handleAgentEnd`, clear `state.steerQueue[sessionId]` and filter out any steer entries (`entry.isSteer === true`) from `state.entries[sessionId]`. This prevents zombie queues from lingering.
2. **On Steer Injection**:
   * In `steerMessageInjected` (and `steer_injected` event handler), instead of marking `steerStatus = 'injected'`, we can remove the steer card entry from `state.entries` and `state.steerQueue` once the prompt list is updated, or transition it out of the active queue view so only the standard user message bubble shows up.

#### Cancel Behavior (`app/src/components/chat/SteerRow.tsx`)
1. **Local-only Fallback for Cancel**:
   * Modify `handleCancel` in `SteerRow.tsx`. If `runId` is null/empty, immediately dispatch `steerMessageCancelled` locally to remove the card from the UI instead of returning early.

---

## 3. Feedback Requested
1. Do you prefer that the orange card completely disappears once injected (since it will show up as a normal user message bubble), or do you want it to remain on screen but with an "Injected" status indicator?
2. Does the proposed automatic cleanup of the queue when the run ends align with your expectations?
