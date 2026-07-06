# Proposal: Robust LLM Streaming and Resilience System
Date: 2026-07-06

This proposal addresses model streaming interruptions, rate limiting exhaustion, and partial message loss when using reasoning models like DeepSeek v4-flash or R1.

---

## 1. Root Cause Analysis

Based on the session logs and codebase audit, the failure chain is as follows:
1. **TCP/Read Timeout on Long Thinking Pauses**: Reasoning models spend a significant amount of time (often 10s-30s+) generating reasoning tokens before producing content. Mid-stream delays can cause TCP connections or proxies (such as NVIDIA's API gateway) to drop idle connections, resulting in a premature EOF/stream interruption.
2. **Shared Retry Limit Exhaustion**:
   - When a stream error is detected, the agent initiates retries.
   - Rapid retries hit the provider's rate limits, returning HTTP 429.
   - In `core/src/client/mod.rs`, the `for attempt in 0..max_retries` loop is hardcoded to **3 iterations** for all errors (both 429 and 5xx).
   - This causes the 429 rate limit retries to exhaust the entire retry budget immediately, returning "Max retries exceeded" or "API error 429" after just 3 quick attempts.
3. **Loss of Thinking Blocks on Failure**:
   - When a run fails, the frontend receives `run_failed` and sets `isProcessing = false`, triggering the auto-save.
   - The frontend's `entriesToMessages` helper only serializes blocks of `type: 'assistant'`, completely ignoring `type: 'thinking'` blocks.
   - If the model failed during or immediately after the thinking phase (before generating content), the assistant's content is empty, causing the auto-save to discard the assistant's turn entirely. As a result, the database contains only the user's prompt.
   - When resuming, since no assistant message is present in the database, the thinking process is lost.

---

## 2. Proposed Fixes

### A. Backend: Separate Network and Rate Limit Retries
Modify `send_with_retry` in `core/src/client/mod.rs` to track network errors and rate limits independently:
- **Rate Limit (429)**: Retry up to 10 times, starting with a 1s base delay up to a 30s maximum backoff.
- **Server Errors / Network Issues**: Retry up to 3 times with standard exponential backoff.
- **TCP Keep-Alive**: Enable TCP keep-alive (e.g., 60 seconds) on the `reqwest` client to prevent connection drops during long reasoning pauses.

### B. Frontend: Preserve Thinking Blocks in Message History
Modify `app/src/features/chat/utils.ts` and `app/src/features/chat/chatSlice.ts` to preserve reasoning/thinking content:
1. **Serialize `<think>` tags**: In `entriesToMessages`, if a turn contains a `thinking` block, wrap its text inside `<think>\n...\n</think>\n` and prepend it to the assistant's content.
2. **De-serialize `<think>` tags on resume**: In the `resumeSession.fulfilled` reducer, parse `<think>...</think>` from the resumed assistant message to cleanly restore the separate `thinking` and `assistant` blocks in the UI.

---

## 3. Impact & long-term robustness

These changes will ensure:
- Connections are kept alive during long reasoning pauses.
- Rate limits (429) are handled gracefully via a patient backoff strategy, rather than aborting immediately.
- Partial thinking progress is always saved to SQLite, making the system resilient to network/api failures during execution.
