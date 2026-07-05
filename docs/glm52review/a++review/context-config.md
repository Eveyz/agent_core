Now let me read the MCP, skills, workflow, and remaining files:
Now let me read the workflow files and remaining files:
Let me precisely identify production-path (non-test) unwraps/expects by finding the test module boundaries:
Now I have everything needed. Let me compile the comprehensive structured report.

---

# Deep-Dive Analysis Report: Context Engine, Compressor, Config & Supporting Modules

## Aggregate Metrics (Requested Commands)

```
grep -rn 'eprintln!' core/src/ | wc -l  →  24  (total across core/src/)
grep -rn 'unwrap()' core/src/ | grep -v test | wc -l  →  388  (total across core/src/)
grep -rn 'expect(' core/src/ | grep -v test | wc -l  →  8  (total across core/src/)
```

**Within the 27 analyzed files specifically:**

| Metric | Count | Location Breakdown |
|--------|-------|--------------------|
| `eprintln!` (production) | **12** | config.rs: 1, hooks/mod.rs: 8, mcp/mod.rs: 3 |
| `unwrap()/expect()` (production-path, non-test) | **3** | skills/manifest.rs:188, workflow/planner.rs:56, planner.rs:60 |
| `unwrap()/expect()` (in tests) | ~85 | spread across all test modules |

---

## File-by-File Analysis

### 1. `core/src/context.rs` (1,384 lines)

**(1) Production unwrap()/expect():** **None.** All `.unwrap()` calls (lines 982, 991, 1000, 1054, 1073, 1080, 1086, 1114, 1123, 1177, 1180, 1230, 1261, 1282–1284, 1351) are inside `#[cfg(test)] mod tests` (test module starts at line 962).

**(2) Error handling gaps:**
- **`segment()` setter methods (lines 313–375):** All seven `set_*` methods use `if let Some(seg) = self.segments.get_mut(...)` and silently no-op if the segment name doesn't exist. There's no logging, no error return, no panic — the caller has no way to know their update was silently dropped. This is a **silent data loss** path.
- **`invalidate_segment()` (line 371):** Same silent no-op pattern.
- **`verify_prefix_stability()` (line 494):** On drift, returns `Err(self.stable_segment_names.clone())` — returns *all* stable segment names, not the ones that actually drifted. The doc says "Vec of drifted segment names" but the implementation returns all of them. **Documentation/implementation mismatch.**

**(3) Code organization issues:**
- **`trim_to_fit_legacy()` (line 696):** Explicitly marked "Legacy method, prefer `trim_to_fit()`". Dead code that should be deprecated/removed or gated behind `#[deprecated]`.
- **`micro_compact()` (line 725):** Another compaction strategy that overlaps with `chunked_drop()`, `trim_to_fit()`, and the compressor pipeline. Four different compaction strategies coexist with unclear guidance on which to use when.
- **Token utilities at bottom (lines 882–957):** `rough_token_count`, `message_token_count`, `truncate_to_token_budget` are module-private free functions mixed into the context module. These are general-purpose utilities that belong in a `token` or `budget` module.
- **`Context` type alias (line 876):** Backward-compat alias `pub type Context = ContextEngine;` — adds API surface ambiguity.

**(4) Missing test coverage:**
- No test for `cache_hint()` return values (strategy "full"/"partial"/"none").
- No test for `stable_prefix_fingerprint()` or `verify_prefix_stability()`.
- No test for `stable_prefix_text()`.
- No test for `should_auto_compact()` threshold logic.
- No test for `trim_to_fit_legacy()` (though it's legacy).
- No test for `build_tool_catalog_string()` with an empty danger_map (defaults to "ReadOnly").
- No test for the `system_prefix_budget` truncation path in `assemble_system_prompt()` (lines 408–419 — the `remaining == 0` and `seg_tokens > remaining` branches).

**(5) Performance issues:**
- **`truncate_to_token_budget()` (lines 922–957):** Uses **binary search** over char boundaries, calling `rough_token_count()` (which runs the full BPE tokenizer) at each step → **O(log n) tokenizer calls** per truncation. For a 10K-token segment, that's ~14 BPE encode operations. Since `rough_token_count()` uses a `OnceLock`-cached `CoreBPE`, each call allocates a `Vec<u64>`. **Recommendation:** Tokenize once, then binary-search over the token vector instead of re-encoding substrings.
- **`assemble_system_prompt()` and `assemble_context_injection()`:** Both call `self.segments.values().collect()` then `sort_by_key()` on every invocation. `messages()` calls both. `current_token_count()` calls both. `trim_to_fit()` calls `current_token_count()` + both assemble methods + the closure. **Multiple redundant sorts and allocations per turn.**
- **`current_token_count()` (line 629):** Calls `assemble_system_prompt()` + `assemble_context_injection()` (both allocate Strings) + iterates all messages calling `rough_token_count()` on each. Called by `should_auto_compact()`, `trim_to_fit()`, and potentially per-turn. **This is the hottest path in the context engine and it re-encodes the entire system prompt every time.**
- **`rough_token_count()` (line 882):** `b.encode_with_special_tokens(text).len()` allocates a `Vec<u64>` for every call. For repeated calls on the same text (e.g., system prompt), consider caching.

**(6) API design issues:**
- **`messages()` returns `Vec<Message>` by value (line 608):** Clones every message every call. Consider returning an iterator or a borrowed view, or caching the assembled result with a dirty flag.
- **`segment()` returns `Option<&ContextSegment>` (line 378):** Read-only access only; there's no `segment_mut()`. The `set_*` methods are the only mutation path, which is inflexible.
- **`ContextEngine::new()` takes `max_tokens` but doesn't validate it:** `max_tokens: 0` would make `auto_compact_threshold` = 0, causing `should_auto_compact()` to always return true. No guard.
- **`set_tool_result_budget()` (line 561):** Mutates `self.compressor.tool_result_budget` directly — breaks encapsulation of the `Compressor` struct.

---

### 2. `core/src/compressor.rs` (732 lines)

**(1) Production unwrap()/expect():** **None** (all in tests, module starts at line 507).

**(2) Error handling gaps:**
- **`dedup_compact()` (line 213):** Uses `HashMap<String, (usize, String)>` keyed by `format!("{}::{}", tool_name, content)`. For large tool results, this clones the entire content string into the hash key. No size guard beyond the 50-char minimum. **Memory spike risk** if many large unique results exist.
- **`chunk_compact()` (line 263):** `messages.remove(i + 1)` inside a loop — O(n²) for large message lists due to Vec shifting. Should use `swap_remove` or collect indices and drain.
- **`run_pipeline()` (line 458):** Only runs stages 1–3. Stage 5 (`gradientCompact`) is documented in the module header but **not implemented**. The `gradient_keep_recent` and `gradient_snip_range` fields are dead config.

**(3) Code organization issues:**
- **Module docstring claims 5 stages (line 1–13)** but only 4 are implemented (Stage 5 `gradientCompact` is missing). The `Compressor` struct has `gradient_keep_recent` and `gradient_snip_range` fields that are never read.
- **`SummarizeRequest` struct (line 492)** is defined after the `Compressor` impl block, separated from its usage in `prepare_summary_compact()`. Poor locality.
- **`truncate_preview()` (line 483)** is a private free function duplicated conceptually by `hygiene::policy::truncate_char_cap()`. Should consolidate.

**(4) Missing test coverage:**
- No test for `run_pipeline()` early-return when under threshold (line 467).
- No test for `run_pipeline()` actually invoking stages.
- No test for `dedup_compact()` with different tool names but same content (should NOT dedup).
- No test for `chunk_compact()` with `protect_recent` boundary (exactly 8 messages).
- No test for `chunk_compact()` with multiple consecutive tool_call/result pairs.

**(5) Performance issues:**
- **`dedup_compact()` (line 217):** Iterates all messages, cloning content for each Tool message into the HashMap key. For a conversation with 100 tool results of 16KB each, this allocates ~1.6MB of hash keys. **Recommendation:** Hash the content (e.g., `std::hash::DefaultHasher`) and store `(hash, usize, String)` with the tool_name only.
- **`chunk_compact()` (line 263):** `messages.remove(i + 1)` is O(n) per removal. With many pairs, this is O(n²).

**(6) API design issues:**
- **`Compressor` has public fields** (`tool_result_budget`, `auto_compact_threshold`, `target_ratio`, `gradient_keep_recent`, `gradient_snip_range`). Two of these (`gradient_*`) are dead. Public mutable fields break invariants — e.g., setting `auto_compact_threshold` to >1.0 or <0.0 has no validation.
- **`run_stages_1_3()` takes `&mut self`** but doesn't mutate any `self` fields. Should be `&self`.
- **`apply_summary()` is a static method** (line 398) while `prepare_summary_compact()` is an instance method. Inconsistent.

---

### 3. `core/src/config.rs` (998 lines)

**(1) Production unwrap()/expect():** **None** (all in tests, module starts at line 619).

**(2) Error handling gaps:**
- **`resolve_env_value()` (line 602):** Falls back to `eprintln!` + raw value when env var is missing. **This silently leaks `${VAR}` syntax to the API as an API key**, which will cause a 401 with a confusing error. Should return `Result<String>` or at minimum log at `warn!` level.
- **`auto_detect_max_context_tokens()` (line 109):** Only auto-detects if `max_context_tokens == 128000` (the default). If a user explicitly sets 128000 for a model that should be 64000, it won't correct. **Silent misconfiguration.**
- **`migrate_legacy_models()` (line 491):** Silently rewrites `default_model` if it matches a legacy key. No warning logged. If migration produces an unexpected key, `Config::load()` will bail with a confusing "not found" error.
- **`add_model()` (line 539):** `full_key.rfind('/').unwrap_or(0)` — if `full_key` has no `/`, `provider_key` = entire key, `model_key` = entire key. This creates a provider and model with the same name, which is likely unintended.

**(3) Code organization issues:**
- **`auto_detect_max_context_tokens()` (line 109):** A massive `if/else if` chain with hardcoded model name matching. This should be a match table or a lookup function. Adding a new model requires editing this function.
- **`from_env()` (line 377):** Duplicates much of the `ProviderConfig`/`ModelConfig` construction logic. Should delegate to `rebuild_models()`.
- **`default_true()`, `default_max_iterations()`, etc.** are module-private functions used as serde defaults. They're scattered throughout the file. Should be grouped.

**(4) Missing test coverage:**
- No test for `save()` round-trip (serialize → deserialize).
- No test for `from_env()`.
- No test for `add_model()`.
- No test for `migrate_legacy_models()` with a `default_model` that matches a legacy key.
- No test for `auto_detect_max_context_tokens()` with `gpt-4-32k`, `gpt-3.5-16k`, `llama-2`, `o1`/`o3` variants.
- No test for `resolve_env_value()` with malformed syntax (e.g., `${`, `${VAR`, `${}`).

**(5) Performance issues:**
- **`rebuild_models()` (line 460):** Clears and rebuilds the entire `models` HashMap. Called during `load()` and `add_model()`. For configs with many providers/models, this is O(n) per call. Not a hot path, but wasteful if called frequently.

**(6) API design issues:**
- **`Config::load()` takes `&str` path** instead of `&Path`. Inconsistent with Rust conventions.
- **`Config` has both `providers` and `models`** — `models` is a flattened cache derived from `providers`. The `legacy_models` field is `#[serde(skip_serializing)]` but still deserializes. This is confusing for users inspecting the struct.
- **`ModelConfig` and `ProviderConfig` have overlapping fields** (`base_url`, `api_key`, `temperature`, etc.). The precedence logic (model overrides provider) is implicit in `rebuild_models()`.
- **`MemoryMode::from_str()` (line 176):** Custom `from_str` instead of implementing `FromStr` trait. Inconsistent with Rust idioms.

---

### 4. `core/src/types.rs` (581 lines)

**(1) Production unwrap()/expect():** **None** (all in tests, module starts at line 355).

**(2) Error handling gaps:**
- **`Message::token_count()` (line 338):** Uses `content.len() / 4` — a crude byte-based estimate that diverges from `context::rough_token_count()` which uses BPE. Two different token counting methods for the same `Message` type. **Inconsistency.**

**(3) Code organization issues:**
- **`Message::token_count()` (line 338)** duplicates `context::message_token_count()` logic. Should be consolidated.
- **`AgentEvent` enum (line 118):** 25+ variants in a single flat enum. Consider grouping into sub-enums or using a struct-based approach for better extensibility.

**(4) Missing test coverage:**
- No test for `Message::token_count()`.
- No test for `CacheUsage::hit_rate()` with zero total.
- No test for `Message::tool()` constructor.
- No test for `Role` serialization/deserialization round-trip.
- No test for `StreamEvent::CompleteWithUsage` variant.

**(5) Performance issues:**
- **`AgentEvent` clones `Vec<Message>` in `AgentEnd` and `MessageStart`/`MessageEnd` variants.** For long conversations, this is expensive. Consider `Arc<Vec<Message>>`.

**(6) API design issues:**
- **`Message` has `content: Option<String>`** but some APIs (OpenAI) require content to be non-null for user/assistant roles. No validation at the type level.
- **`ToolCall.call_type` is a `String`** instead of an enum. Only "function" is valid per OpenAI spec.
- **`CacheUsage` has `Default` but not `Display`** — no way to pretty-print for logging.

---

### 5. `core/src/hygiene.rs` (227 lines)

**(1) Production unwrap()/expect():** **None** (all in tests, module starts at line 80).

**(2) Error handling gaps:**
- **`sanitize()` (line 22):** Returns count of modified messages, but the caller has no way to know *which* messages were modified or *what* was truncated. Consider returning a structured result.
- **`truncate_tool_result()` (line 39):** Clones the content string before passing to `policy::truncate_content()`. The policy function takes `&str`, so the clone is unnecessary.

**(3) Code organization issues:**
- Clean, well-factored. Delegates to `policy` module correctly.

**(4) Missing test coverage:**
- No test for `sanitize()` with a mix of tool results and tool args.
- No test for `truncate_tool_args()` with multiple tool calls in one message.
- No test for `truncate_tool_result()` with `None` content.

**(5) Performance issues:**
- **`truncate_tool_result()` (line 44):** `content.clone()` before passing to `truncate_content()`. The function only reads the content — pass `&content` directly. **Unnecessary allocation per Tool message.**

**(6) API design issues:**
- **`TOOL_ARG_MAX_CHARS` (line 18):** Hardcoded to 200. Should be configurable or at least in `policy.rs` alongside the other budgets.

---

### 6. `core/src/hygiene/policy.rs` (229 lines)

**(1) Production unwrap()/expect():** **None** (all in tests, module starts at line 154).

**(2) Error handling gaps:**
- **`truncate_head_tail()` (line 103):** If `content.lines()` produces lines but the total is under the head+tail threshold, it falls back to `truncate_char_cap()`. But if lines are extremely long (e.g., minified JS), the char-cap may still produce a very large result. No upper bound on the output size beyond the cap.

**(3) Code organization issues:**
- **`floor_char_boundary()` (line 143):** Duplicates `std::str::floor_char_boundary()` which is now stable in Rust. Should use the std version.
- **`INSTRUCTION_TOOLS`, `ACTIVE_READ_TOOLS` (lines 26, 29):** Hardcoded arrays. Should be configurable for custom tools.

**(4) Missing test coverage:**
- No test for `truncate_char_cap()` with content exactly at the cap.
- No test for subagent result truncation (`SUBAGENT_RESULT_MAX_CHARS`).
- No test for `classify()` with `Some("subagents")` (plural).

**(5) Performance issues:**
- **`truncate_head_tail()` (line 104):** `content.lines().collect()` allocates a `Vec<&str>` of all lines. For a 100K-line output, this is a large allocation. Could stream head/tail without collecting all lines.

**(6) API design issues:**
- **`classify()` is public** but `truncate_content()` is the main entry point. Consider making `classify` private or documenting it as part of the public API.
- **Budgets are `pub const`** — good for testing, but means they can't be runtime-configured.

---

### 7. `core/src/hooks/mod.rs` (415 lines)

**(1) Production unwrap()/expect():** **None** in production code. (Tests at line 259 use `.unwrap()` on channel sends.)

**(2) Error handling gaps:**
- **`fire_pre_tool_use()` (line 80):** If a hook returns `ModifyOutput` or `SkipModel` in response to `PreToolUse`, these are silently ignored (lines 96–97). No warning logged.
- **`LoggingHook` (line 205):** Uses `eprintln!` for all logging. Should use `tracing` macros for structured logging and level control.

**(3) Code organization issues:**
- **`HookAction` variants are not event-specific:** `ModifyOutput` is only valid for `PostToolUse`, `SkipModel` only for `BeforeModel`, but the type system doesn't enforce this. Hooks can return nonsensical actions.
- **`HookRegistry` has no `unregister()` method.** Once registered, hooks can't be removed.

**(4) Missing test coverage:**
- No test for `fire_post_tool_use()` with `ModifyOutput`.
- No test for multiple hooks where the first modifies input and the second sees the modified input.
- No test for `fire_after_model()`.
- No test for `LoggingHook` (it's the default hook but untested).

**(5) Performance issues:**
- **`fire_before_model()` (line 165):** Clones the entire `messages: &[Value]` into a `Vec<Value>` for every hook call. For large message lists, this is expensive. Should pass a reference.

**(6) API design issues:**
- **`Hook` trait requires `Send + Sync`** but `HookRegistry` is not `Clone` and has no async support. Hooks that need async operations (e.g., calling an external API) can't be implemented.
- **`PreToolResult` is a separate enum** but `Proceed(Value)` always clones the input even when no modification occurred. Should use `Cow<Value>` or return a reference.

---

### 8. `core/src/prompt.rs` (247 lines)

**(1) Production unwrap()/expect():** **None.** No tests either.

**(2) Error handling gaps:**
- None — this is a pure data/template module.

**(3) Code organization issues:**
- **`PromptBuilder` and `PromptAssembler` (lines 64, 144):** Both are "legacy" backward-compat wrappers. Two overlapping legacy APIs is confusing.
- **`DEFAULT_REACT_PROMPT` (line 40):** Explicitly marked "Deprecated" but still exported. Should be `#[deprecated]`.

**(4) Missing test coverage:**
- **No tests at all.** `PromptBuilder::build()`, `PromptAssembler::assemble()`, and `memory_mode_prompt()` are all untested.

**(5) Performance issues:**
- None significant (string concatenation only).

**(6) API design issues:**
- **`PromptBuilder` uses `&str` everywhere** and internally `to_string()s` everything. Should accept `Into<String>` or `Cow<'static, str>`.
- **`PromptAssembler::add_section()` takes `&str`** for name and content, forcing allocation.

---

### 9. `core/src/error_recovery/mod.rs` (214 lines)

**(1) Production unwrap()/expect():** **None** (all in tests, module starts at line 147).

**(2) Error handling gaps:**
- **`determine_strategy()` (line 92):** Uses string matching (`error.contains("too long")`, `error.contains("rate limit")`) to classify errors. This is fragile — different API providers use different error message formats. **No structured error type matching.**
- **`RecoveryEngine._strategies` field (line 55):** Prefixed with `_` — it's stored but never used. Dead field.
- **`token_escalation_factor` (line 58):** Used to compute `new_max_tokens` but the result is never validated against any upper bound (could exceed the model's actual limit).

**(3) Code organization issues:**
- **`RecoveryStrategy` enum (line 4):** Defined but never used. The `RecoveryEngine` hardcodes the strategy logic in `determine_strategy()` instead of using the `RecoveryStrategy` configs.
- **`RecoveryContext` has no `Display` or `Debug` formatting** beyond the derived `Debug`.

**(4) Missing test coverage:**
- No test for `EscalateTokens` when `token_escalation_factor` would exceed u32::MAX.
- No test for `Retry` backoff delay calculation (`500 * 2^attempt`).
- No test for `RecoveryContext::record_success()` resetting attempt count.

**(5) Performance issues:**
- None significant.

**(6) API design issues:**
- **`RecoveryEngine` has builder methods** (`with_fallback_model`, `with_max_retries`) but no way to set `token_escalation_factor` or `compact_threshold`. Incomplete builder.
- **`RecoveryAction::Fail` carries no error message.** Callers can't propagate context.

---

### 10. `core/src/mcp/` (all files: mod.rs, protocol.rs, sse.rs, tool.rs, transport.rs, channel.rs)

**(1) Production unwrap()/expect():** **None** in production code.

**(2) Error handling gaps:**
- **`mcp/mod.rs:connect_all()` (line 178):** Drains `self.servers` with `drain(..)` then clears at line 198. If `connect_one` panics mid-loop, servers are lost. The `clear()` at line 198 is redundant (already drained).
- **`mcp/transport.rs:request()` (line 92):** Parses every line as `JsonRpcResponse`. If the server sends a notification (no `id` field), `serde_json::from_str` may fail or produce a response with `id: None`. The code skips `id == None` implicitly (line 97), but **parse failures on non-response lines (e.g., log output) will cause `bail!`** instead of being skipped.
- **`mcp/transport.rs:request()` (lines 97 and 102):** **Duplicate check** — `if response.id == Some(id)` appears twice (line 97 and line 102). Dead code — the second check is unreachable.
- **`mcp/sse.rs:connect()` (line 85):** The buffer size check `if buffer.len() > 100_000` is **outside the `match` arms** — it only runs after a `Some(Ok(chunk))` iteration, not after `Some(Err)` or `None`. **The safety check is positioned incorrectly.**
- **`mcp/sse.rs:request()` (line 155):** `http_response.text().await.unwrap_or_default()` swallows the body-read error. If the response body can't be read, the error message will be empty.
- **`mcp/tool.rs:register_all()` (line 49):** Uses `try_lock()` and silently returns if the lock is held. **Tools are silently not registered** with no logging.

**(3) Code organization issues:**
- **`mcp/mod.rs` `Drop` impl (line 335):** Empty `drop()` with only a comment. The actual cleanup relies on `kill_on_drop` on the child process. This is correct but the empty `Drop` impl is misleading.
- **`mcp/transport.rs` `Drop` impl (line 156):** Same empty `Drop`.
- **Transport dispatch helpers (lines 349–379):** Four `match` functions for `Transport` enum dispatch. Could be a trait object instead.

**(4) Missing test coverage:**
- No integration tests for `StdioTransport` (requires spawning a process).
- No test for `McpClientManager::call_tool()`.
- No test for `McpClientManager::shutdown_all()`.
- No test for `SseTransport` URL resolution (relative vs absolute paths).
- No test for `transport_is_alive()` with a dead process.
- No test for `McpChannel::invoke()`.

**(5) Performance issues:**
- **`mcp/transport.rs:request()` (line 77):** `max_attempts = 100` safety limit. For a chatty server sending many notifications, this could exhaust the limit without receiving the response. Should use a timeout instead of attempt count.
- **`mcp/tool.rs:register_all()` (line 54):** Clones all tool definitions from the manager. For servers with many tools, this is a large allocation.

**(6) API design issues:**
- **`McpClientManager` is not `Send`** (holds `StdioTransport` which wraps `tokio::process::Child`). This forces the `Arc<Mutex<>>` wrapping in `McpTool`. A trait-based approach would be more flexible.
- **`McpTool::execute()` locks the entire `McpClientManager`** for the duration of the tool call (line 82). This means **all MCP tool calls are serialized** even across different servers. Should use per-server locks.
- **`McpServerConfig` has `transport: String`** instead of an enum. Typos like "stdo" silently fall through to the stdio default.

---

### 11. `core/src/skills/` (mod.rs, manifest.rs)

**(1) Production unwrap()/expect():**
- **`skills/manifest.rs:188`:** `let val = line.strip_prefix("- ").unwrap().trim();` — preceded by `if line.starts_with("- ")` on line 187, so it's **safe by construction** but still a code smell. **Recommendation:** Use `if let Some(val) = line.strip_prefix("- ")` pattern.

**(2) Error handling gaps:**
- **`skills/mod.rs:scan()` (line 139):** Silently skips directories that can't be read (`Err(_) => continue` at line 150) and manifests that fail to parse (`let Ok(manifest) = ...` at line 166). **No logging of skipped skills.**
- **`skills/mod.rs:load_content()` (line 241):** Returns `Result<String>` but `build_active_context()` (line 258) silently drops errors with `if let Ok(content) = self.load_content(...)`. **Active skill content fails to load silently.**
- **`skills/mod.rs:home_dir()` (line 16):** `dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))` — silently falls back to `.` (current directory). This could cause skills to be scanned from the CWD, which may be unexpected.

**(3) Code organization issues:**
- **`skills/manifest.rs:parse_yaml_frontmatter()` (line 157):** A hand-rolled YAML parser. This is fragile and doesn't handle nested structures, multi-line strings, or YAML edge cases. **Should use a proper YAML parser** (e.g., `serde_yaml`).
- **`skills/manifest.rs:from_markdown()` (line 71):** The `_body` variable (line 83) is computed but never used. Dead code.
- **`skills/mod.rs:collect_skills_dirs()` (line 26):** Recursive function with no depth limit. A symlink loop could cause infinite recursion.

**(4) Missing test coverage:**
- No test for `SkillManifest::read_body()` with no frontmatter.
- No test for `parse_inline_list()` with empty brackets `[]`.
- No test for `SkillManager::add_search_dir()`.
- No test for `SkillManager::with_defaults()` (tests the actual directory resolution).
- No test for `collect_skills_dirs()` with nested plugin directories.
- No test for `load_skill_context()` (backward compat method).

**(5) Performance issues:**
- **`skills/mod.rs:scan()` (line 139):** Reads every `SKILL.md` file synchronously on every scan. For large skill directories, this is slow. No caching of parsed manifests.
- **`skills/mod.rs:build_active_context()` (line 250):** Re-reads skill content from disk for every active skill on every call. No in-memory caching.
- **`skills/manifest.rs:matches_trigger()` (line 102):** Calls `to_lowercase()` on the entire user message and on every trigger for every skill. For N skills with M triggers each, this is O(N×M×|message|).

**(6) API design issues:**
- **`SkillManager::new()` takes a single `PathBuf`** while `with_dirs()` takes `Vec<PathBuf>`. Confusing dual API.
- **`LoadedSkill` struct (line 73):** Public fields, no encapsulation. `manifest` and `source_dir` are directly accessible.
- **`SkillManifest` has `content_path: PathBuf`** which is set to the SKILL.md file path, but the content is read separately via `read_body()`. The relationship between `content_path` and the body is unclear.

---

### 12. `core/src/workflow/` (all files)

**(1) Production unwrap()/expect():**
- **`workflow/planner.rs:56`:** `.unwrap()` on `successors.get_mut(edge.source_node_id.as_str())` — safe because line 47–48 validated the node exists, and lines 42–45 initialized the entry. **Safe by construction but not provably safe to the compiler.**
- **`workflow/planner.rs:60`:** Same pattern for `predecessors.get_mut()`.
- **`workflow/definition.rs:691,697,709`:** `.expect()` calls — but these are **inside `#[cfg(test)]`** (test module at line 686).

**(2) Error handling gaps:**
- **`workflow/executor.rs:execute()` (line 201):** `let _ = tokio::task::spawn_blocking(...)` — the `finish_run` result is silently discarded. If finalizing the run record fails, the run stays in "running" status forever.
- **`workflow/executor.rs:execute_agent_node()` (line 464):** `_run_id.to_string()` uses `_run_id` (prefixed underscore) but then uses it as a real value. The underscore prefix implies it's unused, but it is.
- **`workflow/executor.rs` (line 481):** `let tokens = (0i64, 0i64);` — token tracking is hardcoded to zero. **The executor never tracks actual token usage.** This is documented as "V1" but means all token-based cost analysis is wrong.
- **`workflow/definition.rs:delete()` (line 369):** No cascade protection. If FK constraints aren't set up, deleting a workflow may orphan nodes/edges/runs.

**(3) Code organization issues:**
- **`workflow/executor.rs` (line 337):** `execute_node()` is a free function with 8 parameters — `#[allow(clippy::too_many_arguments)]` is applied. Should be a method on a context struct.
- **`workflow/executor.rs:execute_agent_node()` (line 404):** Also 9 parameters with the same lint suppression.
- **`workflow/definition.rs`:** Mixes type definitions, DB CRUD, and run management in one 712-line file. Should be split into `definition.rs` (types), `db.rs` (CRUD), and `run.rs` (run management).

**(4) Missing test coverage:**
- No test for `WorkflowExecutor::execute()` (requires a `Brain` and `Storage`).
- No test for `execute_node()` with `Transform` or `HumanApproval` node types.
- No test for `apply_router()` skipping downstream nodes.
- No test for `format_agent_input()` with non-string JSON input.
- No test for `WorkflowDef` serialization/deserialization round-trip.
- No test for `list_runs()` or `get_run_node_results()`.
- No test for `set_node_status()`.

**(5) Performance issues:**
- **`workflow/executor.rs:execute()` (line 103):** `workflow.nodes.iter().find(|n| &n.id == id)` inside a loop — O(n) per node lookup. Should build a `HashMap<&str, &NodeDef>` once.
- **`workflow/context.rs:resolve_input()` (line 58):** Clones `self.shared.read().clone()` and `self.input.clone()` into every node's input. For large inputs, this is expensive per node.

**(6) API design issues:**
- **`NodeType::from_str()` (line 42):** Falls back to `Input` for unknown strings. Silent default — should return `Option<Self>` or log a warning.
- **`TrustMode::from_str()` (line 131):** Falls back to `Inherit` for unknown strings. Same issue.
- **`WorkflowDef` has public mutable fields** with no builder pattern. Construction is verbose.
- **`WorkflowRunResult` uses `String` for `status`** instead of an enum.

---

### 13. `core/src/project.rs` (259 lines)

**(1) Production unwrap()/expect():** **None** (all in tests, module starts at line 184).

**(2) Error handling gaps:**
- **`create()` (line 55):** The duplicate-path check uses `if let Ok(mut stmt) = db.prepare(...)` — if the prepare fails, it silently falls through to creating a new project. **Database errors are swallowed.**
- **`delete()` (line 139):** Deletes sessions and messages in separate `execute()` calls without a transaction. If the process crashes between deletes, data is left in an inconsistent state.

**(3) Code organization issues:**
- **`list_sessions()` (line 163):** Uses `format!()` to build SQL with `META_SELECT` from another module. SQL injection risk if `META_SELECT` changes. Should use parameterized queries.

**(4) Missing test coverage:**
- No test for creating a project with a duplicate path (returns existing).
- No test for `delete()` with associated sessions.
- No test for `rename()` on `__adhoc_chat__` (should return false).
- No test for `list_sessions()` with archived sessions (should be excluded).

**(5) Performance issues:**
- **`list()` (line 82):** Loads all projects with no pagination. For users with many projects, this could be slow.

**(6) API design issues:**
- **`ProjectManager` stores `Storage` by value** — not `Arc<Storage>`. Can't be shared across threads without cloning.
- **`Project::from_path()` (line 24):** Generates a UUID but doesn't validate the path exists.

---

### 14. `core/src/paths.rs` (69 lines)

**(1) Production unwrap()/expect():** **None.**

**(2) Error handling gaps:**
- **`get_agverse_dir()` (line 4):** `dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"))` — falls back to literal `"~"` which is not a valid path on any OS. File operations will fail with confusing errors.
- **`redirect_if_artifact()` (line 45):** `std::fs::create_dir_all(&chat_dir)` result is discarded with `let _ =`. If directory creation fails, the returned path won't exist and subsequent file writes will fail.

**(3) Code organization issues:**
- Clean module, well-scoped.

**(4) Missing test coverage:**
- **No tests at all.**
- No test for `redirect_if_artifact()` with various file types.
- No test for path resolution when `dirs::home_dir()` returns `None`.

**(5) Performance issues:**
- None significant.

**(6) API design issues:**
- **`redirect_if_artifact()` (line 45):** Takes `&str` path instead of `&Path`. Inconsistent with Rust conventions.
- Functions return `PathBuf` but the fallback paths are invalid (`"~"`). Should return `Result<PathBuf>`.

---

### 15. `core/src/mode.rs` (88 lines)

**(1) Production unwrap()/expect():** **None.**

**(2) Error handling gaps:**
- None — this is a pure data/policy module.

**(3) Code organization issues:**
- Clean and well-documented.

**(4) Missing test coverage:**
- **No tests at all.**
- No test for `tools_to_remove()` per mode.
- No test for `is_write_allowed()`.
- No test for `system_prompt_override()` content.

**(5) Performance issues:**
- None.

**(6) API design issues:**
- **`tools_to_remove()` returns `&'static [&'static str]`** — hardcoded tool names. If tools are renamed, this silently breaks mode restrictions. No validation at registration time.
- **No `from_str()` or `FromStr` implementation** for `AgentMode` — can't deserialize from config strings.

---

## Cross-Cutting Recommendations (A++ Priority)

### 1. Eliminate `eprintln!` in production code (12 instances)
Replace all `eprintln!` with `tracing::warn!` or `tracing::info!`:
- `config.rs:606` — env var resolution failure → `tracing::warn!`
- `hooks/mod.rs:215–247` (8 instances) — `LoggingHook` should use `tracing::debug!`
- `mcp/mod.rs:188,192,323` (3 instances) — MCP connection lifecycle → `tracing::info!`/`tracing::warn!`

### 2. Fix the 3 production-path `unwrap()` calls
- `skills/manifest.rs:188` → `if let Some(val) = line.strip_prefix("- ")`
- `workflow/planner.rs:56,60` → Use `if let Some(set) = successors.get_mut(...)` or `entry().or_default()` pattern

### 3. Consolidate token counting
`types.rs:Message::token_count()` (chars/4) and `context.rs:rough_token_count()` (BPE) give different results. **Pick one** (BPE) and make `Message::token_count()` delegate to it.

### 4. Fix `mcp/transport.rs` duplicate response-id check (lines 97 & 102)
Remove the dead duplicate check at line 102. Add handling for parse failures on non-response lines (skip instead of `bail!`).

### 5. Fix `mcp/sse.rs` buffer safety check position (line 85)
Move the `if buffer.len() > 100_000` check inside the loop body, before the `match`, so it runs on every iteration regardless of which arm matched.

### 6. Replace hand-rolled YAML parser in `skills/manifest.rs`
Use `serde_yaml` or `serde_yml` for frontmatter parsing. The current parser doesn't handle quoted strings with colons, multi-line values, or nested structures.

### 7. Add transaction wrapping to `project.rs:delete()`
Wrap the three DELETE statements in a single transaction to prevent partial deletes.

### 8. Implement per-server locking for MCP tools
`McpTool::execute()` locks the entire `McpClientManager`, serializing all MCP calls. Split into per-connection locks.

### 9. Add caching to context assembly
`current_token_count()` re-encodes the entire system prompt + injection on every call. Cache the assembled strings with a dirty flag.

### 10. Add the missing test coverage highlighted above
Priority gaps: `prompt.rs` (zero tests), `paths.rs` (zero tests), `mode.rs` (zero tests), `context.rs:cache_hint/fingerprint`, `config.rs:save round-trip`.
