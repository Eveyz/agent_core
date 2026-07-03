# 2026-07-03 Context Compaction (chunked_drop) Bug Fix Proposal

## Error Summary
When executing tasks with multiple tool interactions (especially file/script executions), the agent crashes with the following API error:
```json
LLM request failed: API error 400 Bad Request: {"error":{"message":"Messages with role 'tool' must be a response to a preceding message with 'tool_calls'","type":"invalid_request_error","param":null,"code":"invalid_request_error"}}
```

---

## 1. Root Cause & Event Log Analysis

### What Happened
In run `66a6d3aa-87b2-43ac-9050-030e7fa3ff9b`:
- **Turn 40**: The agent called the `write_file` tool (`call_00_1LRwfkDbkkj5bMUiFkIK4436`) to write a large integration test file (`mcp_integration.rs`, `10570` bytes).
- **Turn 41**: The context size expanded significantly due to the large file content and tool results. At the start of the turn, `self.maybe_compact()` was triggered because the token count exceeded the `COMPACT_THRESHOLD` (80% of the model's limit).
- `maybe_compact()` called `self.context.chunked_drop(keep)`, which simply slices/drains the oldest messages from the message history to bring it down to `keep_recent` messages:
  ```rust
  let drop_count = self.messages.len() - keep_recent;
  self.messages.drain(..drop_count);
  ```
- Because this drain had no structural awareness of message roles, the split boundary cut right between the `Assistant` message (defining the `tool_calls` array) and the subsequent `Tool` response messages.
- The `Assistant` message containing `tool_calls` was dropped, but the matching `Tool` result message was kept.
- **The Outgoing Request**: The agent sent the compacted history. The API gateway (OpenAI / DeepSeek) received a `tool` message at the beginning of the history with no preceding `assistant` message that declared the matching `tool_calls` ID.
- **Model Response**: The model provider rejected the payload at the gateway/validation stage, returning a `400 Bad Request` error. No completion text or tokens were generated.

---

## 2. Proposed Solution

To resolve this issue, `chunked_drop` must only discard messages up to a safe boundary. A safe boundary is a `Role::User` message, which:
1. Always initiates a new turn.
2. Ensures that any previous tool calls and tool responses are completely discarded together.
3. Guarantees that the remaining history starts cleanly (avoiding orphaned `Tool` or intermediate `Assistant` messages).

### Proposed Changes in `core/src/context.rs`

We will modify `chunked_drop` to search backwards from the target split point `self.messages.len() - keep_recent` for the nearest `User` message, and only drop up to that index:

```rust
    pub fn chunked_drop(&mut self, keep_recent: usize) -> usize {
        if self.messages.len() <= keep_recent {
            return 0;
        }

        // Target split index based on the number of messages we want to keep
        let max_split_idx = self.messages.len() - keep_recent;
        let mut drop_count = 0;

        // Find the closest User message boundary at or before the target split index
        for i in (0..=max_split_idx).rev() {
            if self.messages[i].role == Role::User {
                drop_count = i;
                break;
            }
        }

        if drop_count > 0 {
            self.messages.drain(..drop_count);
        }
        drop_count
    }
```

This ensures we keep *at least* `keep_recent` messages, and the kept messages always start cleanly with a `User` message.
