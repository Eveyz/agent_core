# Completed Plan: Redesign Session Memory Model (Scheme B)

Timestamp: 2026-07-08

This plan has been fully implemented, verified, and tested to simplify and unify session storage by removing the `session_event_log` table, removing the `user_message` column from `prompts`, and using the `session_messages` table with a rich `metadata` column on assistant messages as the single source of truth for messages, thinking, subagents, and UI blocks.

## Database Schema Redesign
- Removed `session_event_log` table creation from `init_tables()`.
- Updated `prompts` table definition to remove `user_message`.
- Updated `session_messages` table definition to add `model TEXT DEFAULT ''` and `metadata TEXT DEFAULT '{}'`.
- Cleaned up old migrations related to `prompt_id` or `session_messages` columns since they are now part of the base schema.
- Added auto-reset migration logic to `Storage::new` to drop outdated tables if migrating from pre-Scheme B, letting the app cleanly re-create them.

## Backend (Rust Core & Tauri) Modifications
- Added `pub metadata: Option<serde_json::Value>` to the `Message` struct in `core/src/types.rs`.
- Updated all message constructors to initialize `metadata: None`.
- Updated all test struct initializers of `Message` to include `metadata: None`.
- Updated `create_prompt` in `core/src/session.rs` to not take or store `user_message`.
- Updated `save_full` in `core/src/session.rs` to persist the complete metadata of the assistant message (containing blocks, timings, subagents, and hit rate) and resolved a duplicate key overwrite bug on `msg_index`.
- Updated `resume` in `core/src/session.rs` to retrieve the metadata column from the `session_messages` table and reconstruct clean message histories without using the deleted event log table.
- Removed `get_event_log`, `log_event`, `clear_event_log`, and `truncate_payload` functions from `core/src/session.rs`.
- Updated `FrontendMessage` struct in `app/src-tauri/src/lib.rs` to add `#[serde(default)] metadata: Option<serde_json::Value>` and mapped it in `to_agent_message` and inside `resume_session`.
- Removed `event_log` from `FrontendSession` struct and `resume_session` return value.
- Updated `save_session_messages` parameters to remove `event_log_json` and do not invoke `clear_event_log` or `log_event`.
- Updated `send_message` call to `create_prompt` to remove the user message content argument.
- Updated `DEFAULT_IDENTITY` in `core/src/prompt.rs` to explicitly explain MCP tool prefix conventions (`mcp__<server>__<tool>`), allowing models to understand and reference them correctly when queried about available MCP resources.
- Updated `MAX_STREAM_RETRIES` to 10 in `core/src/runtime/run/turn.rs` to align mid-stream connection drop recovery attempts with the general HTTP client network retry budget of 10.

## Frontend (React & Redux) Modifications
- Added `metadata?: Record<string, any>` to `FrontendMessage` and removed `user_message` from `FrontendPrompt` in both `app/src/features/chat/types.ts` and `app/src/features/project/projectSlice.ts`.
- Removed `allEventLog` and `eventLogBySession` from `ChatState` interface and `EventLogEntry` interface definition.
- Updated `ResumeSessionResult` to remove `event_log`.
- Updated `saveSessionMessages` thunk to remove `eventLog` argument.
- Updated `entriesToMessages` in `app/src/features/chat/utils.ts` to accept `subagents` as the second argument, and for each turn assistant message, pack `blocks`, `startTime`, `endTime`, `cacheHitRate`, `turnIds`, and related `subagents` details into the message's `metadata` property.
- Removed `entriesToEventLog` and `getFullEventLog` from `app/src/features/chat/utils.ts`.
- Added `getTimingMetrics(entries: ChatEntry[])` in `app/src/features/chat/utils.ts` to calculate timing from visible turn entries.
- Removed `allEventLog` and `eventLogBySession` from `initialState` and slice actions (`cacheCurrentSession`, `restoreOrClearSession`, `deleteSession`) in `app/src/features/chat/chatSlice.ts`.
- Added `isDirty` and `isDirtyBySession` state tracking to `initialState`, reducers (`cacheCurrentSession`, `restoreOrClearSession`, `userMessageSent`, `agentEventReceived`, `clearChat`, `steerMessageQueued`), and extraReducers (`saveSessionMessages.fulfilled`, `deleteSession.fulfilled`, `resumeSession.fulfilled`) in `chatSlice.ts`.
- Simplify `loadMorePrompts` to just increment `visiblePromptsCount` and call `rebuildEntries(state)`.
- Rewrote `rebuildEntries(state)` to:
  - Reconstruct `subagents` from prompts' assistant messages metadata.
  - Extract user messages from prompt messages.
  - Reconstruct turn entries, blocks, timings, and cache metrics directly from the assistant message's `metadata` block.
  - Implement robust fallback logic for legacy messages (no metadata) by parsing `<think>` tags and tool calls from raw message lists.
- Updated `resumeSession.fulfilled` in `app/src/features/chat/chatSlice.ts` to set `state.isProcessing = state.allPrompts.some(p => p.status === 'running')` to fix the stop button state on resume.
- Updated `useSaveSession.ts` hook to compare target `activeSessionId` against `chatState.activeSessionId` to block stale transition saves, read `isDirty` to skip redundant saves, and compute timings using `getTimingMetrics`. Removed unused parameter from call-sites.
- Rewrote `useAutoSaveSession.ts` to rely on strict `isProcessing` (true -> false) transitions for the same session ID via refs, preventing any auto-save triggers during session switching.
- Updated `agentEventsBatch` listener in `app/src/store.ts` to compute timing metrics using `getTimingMetrics` and remove `eventLog` from `saveSessionMessages` call.

## Verification & Testing
- Ran cargo tests successfully:
  ```bash
  cargo test -p agent_core
  ```
- Checked building of workspace binaries:
  ```bash
  cargo check
  ```
- Verified TypeScript compilation successfully across client react components:
  ```bash
  npx tsc
  ```
