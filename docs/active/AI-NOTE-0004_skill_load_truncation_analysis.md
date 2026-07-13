# AI-NOTE-0004: Skill Load Truncation Deep Analysis & Refined Fix Plan

```yaml
---
id: AI-NOTE-0004
type: AI-NOTE
title: Skill Load Truncation Deep Analysis & Refined Fix Plan
status: Draft
author: agent_core
created: 2026-06-28
updated: 2026-06-28
reviewers: [zniverse]
related: [PLAN-0006, PLAN-0005]
supersedes: ~
superseded_by: ~
tags: [truncation, skills, hygiene, compressor, context-engine, cache-hit]
---
```

## 1. Problem Statement

`skill_load` 返回完整技能内容（8000~15000 chars for pptx-generator, minimax-pdf, deep-research），但在到达模型前被截断到 ~4000 chars，中间 60-80% 的指令被丢弃。

## 2. Full Truncation Flow (Two Paths)

### Path A: Manual skill_load (Tool Call) — Main Concern

```
Turn N: Model calls skill_load("pptx-generator")
  ↓
SkillLoadTool::execute() → load_content() → 完整 SKILL.md body (e.g., 10500 chars)
  包装: "Skill 'pptx-generator' loaded and activated.\n\n== Skill: pptx-generator (v1.0) ==\n<full content>\n== End Skill: pptx-generator =="
  ↓
run.rs:726 → context.add(Message::tool(call.id, result))  [name: None, 全量存储]
  ↓
Turn N+1:
  ↓
  model_turn() → build_messages()
    → context.messages()                  [system(frozen) + history + context_injection in last user]
    → hygiene::sanitize(&mut messages)    [COPY, 不修改存储]
        ↓
        truncate_tool_result() →
          条件: role==Tool && content.len > 4000
          效果: head(15行) + tail(8行) + signals(5行)
          丢失: 中间约 60-80% 的内容
    ↓
  LLM API call ← 模型看到截断后的 skill 内容
```

```
Later (context grows large):
  maybe_compact() → chunked_drop() + trim_to_fit()
    → compressor::snip_compact(&mut context.messages)  [IN-PLACE, 修改存储!]
        条件: role==Tool && content.len > 4000
        效果: 保留前 4000 chars + "[... truncated from N chars]"
```

### Path B: Auto-Trigger (Segment 6) — Fixed by PLAN-0012

`loaded_skills.max_tokens` is now **0** (unlimited). Auto-injected catalog + active skill bodies are no longer hard-truncated at assemble time. Global `maybe_compact` remains the safety net.

~~Previously: truncate_to_token_budget(..., 2000)~~
## 3. Root Cause

| Layer | Location | Budget | Effect on skill_load |
|-------|----------|--------|---------------------|
| Hygiene | `hygiene.rs:33-84` | 4000 chars | 保留 head+tail, 丢失中间 |
| Compressor | `compressor.rs:186-203` | 4000 chars | 保留前 N chars (permanently!) |
| Segment 6 | `context.rs:291-292` | 500 tokens | 自动触发时截断 |

**核心问题**: 所有 `Role::Tool` 消息被统一截断，但 `skill_load` 返回的是 **指令 (instruction)** 而非 **数据 (data)**。

**次要问题**: `Message::tool()` 不记录 `tool_name`，导致截断层无法区分工具类型。

## 4. Cache Hit Impact Analysis (PLAN-0005)

### 4.1 Current Cache Structure (from PLAN-0005)

```
messages: [
  system(frozen: identity + principles + tool_catalog)  ← CACHED
  user(q1) ────┐
  asst(a1) ────┤
  user(q2) ────┤  conversation history
  asst(a2) ────┤  (not cached)
  tool(truncated skill) ──┤  ← skill_load result, truncated by hygiene
  user(q3 + context_injection) ──┘  ← last user message
]
```

### 4.2 What Changes If We Skip Truncation?

| Before (truncated 4000 chars) | After (full 10000 chars) | Cache impact |
|-------|-------|-------------|
| `tool` message: ~4000 chars | `tool` message: ~10000 chars | **None** — this is conversation history, not cacheable prefix |
| `system` message: unchanged | `system` message: unchanged | **Hit preserved** |
| Total tokens: N | Total tokens: N + ~1500 | Reaches context limit faster → maybe_compact fires sooner |

### 4.3 Why Cache Is Safe

1. **System prompt 不变**: frozen 段 (identity/principles/tool_catalog) 完全不变，KF cache 前缀命中。
2. **Context injection 不变**: 注入在最后一个 user message，本就每次变化。
3. **skill_load 结果稳定**: 无论截断与否，内容是确定性的（同一 skill 同一内容）。不会引入 non-deterministic 变化。
4. **sanitize 操作 COPY**: 不修改 `context.messages` 存储，只修改临时副本。
5. **唯一的代价**: 更多 tokens → 更快触发 maybe_compact（但 PLAN-0005 的 chunked_drop 优先处理，影响有限）。

### 4.4 Compressor snip_compact 的 Cache 风险

⚠️ **重要**: `snip_compact` 修改的是 `context.messages` **存储本身**。如果 skill_load 结果超过 4000 chars，它会永久截断存储的消息。这意味着：

- 截断前：所有后续 turn 模型都能看到完整内容（sanitize 只截断副本）
- 截断后（snipCompact）：后续 turn 模型看到的都是被截断后的 4000 chars

所以 **compressor 层的跳过比 hygiene 层更重要**！hygiene 只影响当前 turn，compressor 影响所有后续 turn。

✅ 跳过后不会破坏 cache：skill_load 结果保持完整且稳定。system prompt 依然 frozen。

## 5. Refined Solution Design

### 5.1 修改 `Message::tool()` — 记录工具名称

```rust
// types.rs — Before
pub fn tool(tool_call_id: String, content: String) -> Self {
    Self { role: Role::Tool, content: Some(content), tool_calls: None,
           tool_call_id: Some(tool_call_id), name: None }
}

// After
pub fn tool(tool_call_id: String, content: String, tool_name: Option<String>) -> Self {
    Self { role: Role::Tool, content: Some(content), tool_calls: None,
           tool_call_id: Some(tool_call_id), name: tool_name }
}
```

**3 个调用点全部传入 `call.function.name.clone()`**:
| File | Line |
|------|------|
| `core/src/runtime/run.rs` | 726 |
| `core/src/subagent/mod.rs` | 252 |
| `core/src/agent/mod.rs` | 723 |

### 5.2 Hygiene Layer — Skip Non-Truncatable Tools

```rust
// hygiene.rs — 在文件顶部添加
const NON_TRUNCATABLE_TOOLS: &[&str] = &["skill_load"];

fn truncate_tool_result(msg: &mut Message) -> bool {
    if msg.role != Role::Tool { return false; }
    // ⭐ NEW: skip non-truncatable tools (skill_load content is instruction, not data)
    if msg.name.as_deref()
        .map_or(false, |n| NON_TRUNCATABLE_TOOLS.contains(&n))
    {
        return false;
    }
    // ... existing truncation logic
}
```

### 5.3 Compressor Layer — Skip Non-Truncatable Tools

```rust
// compressor.rs — 在文件顶部添加相同的常量
const NON_TRUNCATABLE_TOOLS: &[&str] = &["skill_load"];

pub fn snip_compact(&self, messages: &mut Vec<Message>) -> usize {
    let mut modified = 0;
    for msg in messages.iter_mut() {
        if msg.role == Tool
            // ⭐ NEW: skip non-truncatable tools
            && !msg.name.as_deref()
                .map_or(false, |n| NON_TRUNCATABLE_TOOLS.contains(&n))
            && let Some(ref content) = msg.content
            && content.len() > self.tool_result_budget
        {
            // ... existing truncation
        }
    }
}
```

### 5.4 Segment 6 Budget: 500 → 2000 tokens

```rust
// context.rs:292
let skills = ContextSegment::new(
    "loaded_skills", "Loaded Skills", 5,
    2000,  // ← 从 500 改为 2000 (~8000 chars, 足够大多数 skill)
    RefreshPolicy::PerTurn, Stability::Dynamic,
);
```

**Note**: 即使 manual skill_load path 不经过 Segment 6，但 `skill_load` 调用后会 `mgr.activate(name)`，下一 turn `build_active_context()` 会重建 Segment 6。如果此时 budget 只有 500 tokens，auto-trigger 的 skill 内容仍会被截断。二者应同步修复。

### 5.5 为什么不用 Tool Trait？

**不推荐方案** (Option A): 
- 需要在 `Tool` trait 加 `truncatable()`
- ToolRegistry 需要提取非截断工具名称
- 需要 plumb 到 hygiene/compressor 调用点
- 增加约 30 行改动，但只有一个工具需要 (skill_load)

**推荐方案** (Option B):
- 硬编码常量列表，2 个地方（hygiene + compressor）
- 改动量最小，不易出错
- 后续如有新工具需要，加一行字符串即可

## 6. Updated Files to Modify

| # | File | Change | 影响范围 |
|---|------|--------|---------|
| 1 | `core/src/types.rs:305-313` | `Message::tool()` + tool_name param | API change |
| 2 | `core/src/runtime/run.rs:726` | `Message::tool(call.id, result, Some(call.function.name.clone()))` | Main path |
| 3 | `core/src/subagent/mod.rs:252` | 同上 | Subagent path |
| 4 | `core/src/agent/mod.rs:723` | 同上 | Legacy agent path |
| 5 | `core/src/hygiene.rs` | 添加 `NON_TRUNCATABLE_TOOLS` + skip logic | **主修复** |
| 6 | `core/src/compressor.rs` | 添加 `NON_TRUNCATABLE_TOOLS` + skip logic | **永久存储修复** |
| 7 | `core/src/context.rs:292` | loaded_skills budget: 500 → 2000 | 自动触发修复 |
| 8 | `core/src/hygiene.rs` (tests) | 添加 skill_load skip 的测试 | 回归保障 |

## 7. Test Plan

### 7.1 Existing Tests
- hygiene.rs: 6 个测试 (truncate, skip, error-signals, truncate-args, sanitize-count)
- 所有修改不应破坏现有测试

### 7.2 New Test
```rust
#[test]
fn skip_truncation_for_skill_load() {
    let big = "line ".repeat(2000); // ~10000 chars
    let mut msg = Message {
        role: Role::Tool,
        content: Some(big.clone()),
        tool_calls: None,
        tool_call_id: Some("t1".into()),
        name: Some("skill_load".into()),  // ← 现在 name 被传入了
    };
    assert!(!truncate_tool_result(&mut msg));  // ← 不会被截断
    assert_eq!(msg.content.unwrap(), big);     // ← 内容保持完整
}
```

## 8. Verification Checklist

- [ ] `cargo build` 成功
- [ ] `cargo test` 全部通过（含新测试）
- [ ] 验证: skill_load 结果 ≥ 8000 chars 时，sanitize 返回 count=0 for that message
- [ ] 验证: 其他 tool (read/bahs/grep) 仍然正常截断
- [ ] 验证: system prompt frozen → cache hit 不受影响
- [ ] End-to-end: 加载 pptx-generator skill → 模型能看到完整 PPTX 生成指令

## References

- `core/src/hygiene.rs:33-84` — `truncate_tool_result()`
- `core/src/compressor.rs:186-203` — `snip_compact()`  
- `core/src/context.rs:160-169` — `seg.assemble()` — truncate per segment budget
- `core/src/context.rs:291-292` — Segment 6 "loaded_skills" 500 token budget
- `core/src/context.rs:384-458` — `assemble_system_prompt()` / `assemble_context_injection()` — PLAN-0005 cache structure
- `core/src/types.rs:29-39` — `Message` struct with `name` field
- `core/src/types.rs:305-313` — `Message::tool()` constructor
- `core/src/tools/skill.rs:110-131` — `SkillLoadTool::execute()`
- `core/src/skills/mod.rs:345-355` — `load_skill_context()`
- `core/src/runtime/run.rs:712-727` — tool result → context
- `core/src/runtime/run.rs:775-826` — model_turn + sanitize + LLM call
- `core/src/runtime/run.rs:1206-1250` — maybe_compact + trim_to_fit

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-28 | agent_core | Initial draft |
| 2026-06-28 | agent_core | Refined analysis: added cache impact analysis (vs PLAN-0005), compressor in-place risk, two-path analysis, updated test plan |

---
*Generated by AI Agent (agent_core)*
*Model: gpt-4o | Timestamp: 2026-06-28T20:13:00+08:00*
