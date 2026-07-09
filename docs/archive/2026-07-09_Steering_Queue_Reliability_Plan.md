# Archived Plan: Steering Queue Reliability and Auto-Cleanup

Date: 2026-07-09

This plan updates the frontend event handlers and steering row UI to clear the steering queue and pending steer cards when runs terminate, and allows local-only cancellation when the run ID is null.

## Proposed Changes

### Frontend State Management
* **`app/src/features/chat/eventHandlers.ts`**: Clear steer queue and filter pending steer entries on agent run end.
* **`app/src/components/chat/SteerRow.tsx`**: Add local cancel fallback if run ID is null.

---

## Tasks

Detailed tasks were tracked locally in `task.md` and successfully completed.
* Update eventHandlers.ts: Completed
* Update SteerRow.tsx: Completed
* Verification: Completed
