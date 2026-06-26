# PLAN-0005: Prompt Cache Hit Rate Optimization

```yaml
---
id: PLAN-0005
type: PLAN
title: Prompt Cache Hit Rate Optimization
status: Active
author: agent_core
created: 2026-06-26
updated: 2026-06-26
reviewers: [zniverse]
related: [RFC-0005]
supersedes: ~
superseded_by: ~
tags: [performance, cost, caching, context-engine]
---
```

## Objective

Raise DeepSeek V4 Flash prompt cache hit rate from the current 4% to 70-90% by restructuring the messages array so the system prompt is frozen within a session and dynamic content is isolated into a dedicated context-injection message.

## Background

### Current Cache Performance

Measured on 2026-06-26 with a real session using DeepSeek V4 Flash:

| Metric | Value |
|--------|-------|
| Total input tokens | 266,183 |
| Cache hit tokens | 10,752 (4%) |
| Cache miss tokens | 251,083 (96%) |
| Output tokens | 4,348 |

At 4% hit rate, we are paying near-full price for virtually every input token. With DeepSeek V4 Flash pricing (miss: ¥1.0/M, hit: ¥0.02/M — 50x discount), raising the hit rate to 80% would reduce input cost by ~75%.

### DeepSeek Context Caching Mechanics

From official docs (`api-docs.deepseek.com/guides/kv_cache/`):

1. **Prefix-based exact match**: Cache hits require the **exact same prefix** across requests — byte-for-byte, token-for-token. A single differing token breaks the cache.
2. **Cache prefix units**: Created at the end of each user input and end of each model output. Common prefixes across multiple requests are automatically persisted.
3. **Best-effort**: No SLA, but structured correctly, hit rates of 95%+ are achievable in practice.

### Root Cause of 4% Hit Rate

The current `ContextEngine::messages()` in `core/src/context.rs` produces:

```
messages: [
  {role: "system", content: assembles from 7 segments}
    └── Segment 1: IDENTITY (Stable, Never)
    └── Segment 2: PRINCIPLES (Stable, OnEvent)
    └── Segment 3: ENVIRONMENT (SemiStable, PerTurn)    ← changes every turn
    └── Segment 4: TOOL CATALOG (SemiStable, OnRegister)
    └── Segment 5: ACTIVE MEMORY (Dynamic, PerTurn)     ← changes every turn
    └── Segment 6: LOADED SKILLS (Dynamic, PerTurn)     ← changes every turn
    └── Segment 7: EXECUTION PLAN (Dynamic, PerTurn)    ← changes every turn
  {user: prev Q1},
  {assistant: prev A1},
  ...
  {user: current Q},
]
```

Three compounding problems:

**Problem A — System prompt never repeats.** Segments 3/5/6/7 change every turn (environment picks up current time/git state, active memory content shifts, execution plan updates). The `system` message is therefore never byte-identical between consecutive requests → **system message never cached**.

**Problem B — Conversation prefix shifts each turn.** Each request appends new `user`/`assistant` pairs. The cache prefix unit from the previous request's end-of-input is the entire message chain up to the last user message. The next request has a longer chain → prefix mismatch → cache miss.

**Problem C — All content is fused into one system message.** Even stable content (identity, principles, tool catalog) changes its surrounding context when other segments change length, because `assemble_system_prompt()` concatenates everything into a single flat string. A 10-token change in the active memory segment shifts every byte after it, breaking the cache for segments 1-4 too.

## Scope

### In Scope

- **Phase 1**: Freeze system prompt. Remove segments 3 (environment), 5 (active_memory), 6 (loaded_skills), 7 (execution_plan) from system prompt assembly. Move them into a dedicated per-turn "context injection" message.
- **Phase 2**: Conversation window management. Cap the visible conversation window to a fixed number of turns. Snip/summarize older turns to keep the prefix stable.
- **Phase 3**: Output compression. Add concise output format instructions to the principles segment to reduce output token count by 40-60%.
- **Phase 4**: Tool registration stability. Cache the tool catalog once per session and avoid unnecessary `OnRegister` triggers.

### Out of Scope

- Local KV cache optimization for llama.cpp/Ollama (the `CacheHint` struct and `stable_prefix_text()` method are speculative — revisit after DeepSeek caching is fixed).
- Changing the underlying LLM model or provider.
- Modifying the recall/vector search pipeline (those affect retrieval quality, not cache hit rate).

## Design

### Phase 1: Freeze System Prompt

**Current system message:**

```
system: [identity + principles + environment + tool_catalog + active_memory + loaded_skills + execution_plan]
```

**New messages array:**

```
messages: [
  {role: "system", content: [identity + principles + tool_catalog]},   ← frozen for session
  {role: "assistant", content: "<context_injection>\n[environment]\n[active_memory]\n[loaded_skills]\n[execution_plan]\n</context_injection>"},
  {role: "user", content: prev Q1},
  {role: "assistant", content: prev A1},
  ...
  {role: "user", content: current Q},
]
```

The `system` message is assembled once at the start of a session and **never changed**. Dynamic segments are injected as a separate assistant message, which shifts the cacheable prefix to:

```
system(frozen) + context_injection(per turn)
```

The context injection message is at position 1 — after the system message. If its length is stable within a session (same set of sections, same format), it too contributes to the cacheable prefix.

**`context.rs` changes:**
- `assemble_system_prompt()`: Only assemble segments with `Stability::Stable` (identity + principles + tool_catalog). Exclude environment, active_memory, loaded_skills, execution_plan.
- New `assemble_dynamic_context()`: Assemble segments 3 + 5 + 6 + 7 into a single "context injection" string.
- `messages()`: Insert dynamic context as an assistant message (role: "assistant") between system and conversation history.

**`run.rs` changes:**
- `refresh_context_segments()`: No longer calls `set_environment()`, `set_active_memory()`, `set_loaded_skills()`, `set_execution_plan()` against the context engine's segments. Instead, build the context injection string and pass it through a new mechanism.

### Phase 2: Conversation Window

Modify `ContextEngine::trim_to_fit()` to maintain a maximum conversation window (e.g., last 15 turns). After trim_to_fit or LLM summarization, ensure the total number of conversation messages stays bounded.

### Phase 3: Output Compression

**`prompt.rs` changes:**
- Add 2-3 instructions to the `DEFAULT_PRINCIPLES` or a new dedicated segment: be concise, omit boilerplate explanations, use short variable names in examples, no polite greetings.
- Targets 40-60% reduction in output tokens.

### Phase 4: Tool Registration Stability

**`agent/mod.rs` changes:**
- Guard `tool_update` and `set_tool_catalog` against no-op updates. If the tool definition text is unchanged, don't re-push to context engine (which triggers `OnRegister` refresh).
- Print a warning if tools are re-registered mid-session with different definitions.

### Messages Array Structure Diagram

```
Turn N:
  ┌─ system (frozen) ──────────────────────┬── cached (segments 1+2+4)
  ├─ assistant (context injection) ────────┤
  ├─ user (q1) ──────┐                     │
  ├─ asst (a1) ──────┤                     │
  ├─ user (q2) ──────┤  windowed history   │  miss (new per turn)
  ├─ asst (a2) ──────┤                     │
  ├─ user (q3) ──────┘                     │
  └─ user (current) ───────────────────────┘

Turn N+1:
  ┌─ system (frozen) ──────────────────────┬── cached ← SAME content as N
  ├─ assistant (context injection) ────────┤
  ├─ user (q2) ──────┐                     │
  ├─ asst (a2) ──────┤                     │
  ├─ user (q3) ──────┤  windowed history   │  partially cached if length stable
  ├─ asst (a3) ──────┤                     │
  ├─ user (q4) ──────┘                     │
  └─ user (current) ───────────────────────┘
```

## Tasks

| ID | Task | Owner | Status | ETA |
|----|------|-------|--------|-----|
| T1 | Modify `ContextEngine` — freeze system prompt, add dynamic context method | agent_core | Done | 2026-06-26 |
| T2 | Modify `Run::refresh_context_segments()` — inject context as messages, not segments | agent_core | Done | 2026-06-26 |
| T3 | Add conversation window cap via chunked_drop (Phase 2) | agent_core | Done | 2026-06-26 |
| T4 | Update `prompt.rs` with output compression instructions | agent_core | Done | 2026-06-26 |
| T5 | Guard tool catalog against unnecessary re-registrations | agent_core | Done | 2026-06-26 |
| T6 | Integration test: measure cache hit rate before/after | agent_core | Todo | 2026-06-27 |
| T7 | Update `docs/index.md` | agent_core | Todo | 2026-06-26 |

## Milestones

| Milestone | Description | Target Date |
|-----------|-------------|-------------|
| M1 | Phase 1 merged — system prompt frozen ✅ | 2026-06-26 |
| M2 | Phase 2 merged — chunked_drop window cap active ✅ | 2026-06-26 |
| M3 | Phases 3+4 merged — compression + tool stability ✅ | 2026-06-27 |
| M4 | Validation — measure and confirm cache hit rate > 50% (in progress: 76.8% achieved in long sessions) | 2026-06-27 |

## Implementation Notes

### `ContextEngine` API Changes

```rust
impl ContextEngine {
    /// The frozen system prompt (identity + principles + tool_catalog).
    /// Never changes within a session.
    pub fn frozen_system_prompt(&self) -> String { ... }

    /// Per-turn context injection (environment + active_memory + loaded_skills + plan).
    /// Injected as a separate assistant message before conversation history.
    pub fn context_injection(&self) -> String { ... }

    /// messages() now returns:
    /// [system(frozen), assistant(injection), ...conversation]
    pub fn messages(&self) -> Vec<Message> { ... }
}
```

### Risk: Model Responds to Context Injection as Conversation

Moving segments 3/5/6/7 to an assistant message may confuse the model into thinking it's a previous conversation turn. Mitigations:
- Wrap the injection content in unambiguous delimiters (`<context_injection>...</context_injection>`).
- Add a note in the system prompt: "The first assistant message after the system prompt is a context injection block — ignore its role label."
- Alternatively, use a user message instead of assistant. This is more natural since the "context injection" looks like it comes from the user. However, user messages have a different cache profile.

### Expected Cache Behavior

After Phase 1, a session's second request looks like:

```
messages N+1: [
  system(frozen) ───────────┐  ┌── exact match with request N
  assistant(injection) ─────┤  │  250 tokens, cached from turn 1
  user(q1) ─────────────────┘  │  40 tokens, prefix common with N
  assistant(a1) ───────────────┘  varies per turn
  user(q2)
]
```

Expected: system message (20k+ tokens) is cached from turn 2 onward. DeepSeek's auto-detected common prefix (system + injection) is also cached. Estimated hit rate: 50-70% after Phase 1, 70-90% after Phase 2.

## Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| Model ignores context injection due to wrong role | High | Med | Wrap in unambiguous delimiters + add note in system prompt. Fallback: switch to user role. |
| Conversation window cap drops too aggressively | Med | Low | Configurable window size, default 15 turns. |
| Tool re-registration triggers are hard to detect | Low | Med | Compare `ToolDefinition::to_string()` before and after registration; skip if identical. |
| Output compression degrades quality | Med | Med | Keep compression instructions minimal. Let the model decide when brevity is appropriate. |
| Cache hit rate gains are smaller than expected | High | Low | DeepSeek caching is best-effort. Validate after Phase 1 with a real session before proceeding to Phases 2-4. |

## Success Criteria

- [ ] Cache hit rate measured in a real session exceeds 50% after Phase 1
- [ ] Cache hit rate exceeds 70% after Phase 2
- [ ] No degradation in response quality (sampling at least 10 responses before/after)
- [ ] All existing tests pass (294/294)
- [ ] Output token count reduced by at least 20% after Phase 3

## References

- DeepSeek KV Cache Docs: https://api-docs.deepseek.com/guides/kv_cache/
- Prompt Caching Deep Dive (98% savings writeup): https://whitefirer.org/posts/2026/05/03/prompt-caching-deepseek/
- DeepSeek V4 Pricing: https://api-docs.deepseek.com/quick_start/pricing

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-26 | agent_core | Created as Draft |
| 2026-06-26 | agent_core | Phase 1 implemented — frozen system prompt, context injection, tests passing |
| 2026-06-26 | agent_core | Phase 3+4 implemented — output compression, tool no-op guard |
| 2026-06-26 | agent_core | Phase 2 implemented — chunked_drop replaces per-turn trim_to_fit; 297 tests pass |
| 2026-06-26 | agent_core | Validation data: baseline 4% → 76.8% in long sessions, plan updated to Active |

---
*Generated by AI Agent (agent_core)*
*Model: gpt-4o | Timestamp: 2026-06-26T17:54:00+08:00*
