# AI-NOTE-0004: Skill Load Truncation Analysis & Fix Plan

```yaml
---
id: AI-NOTE-0004
type: AI-NOTE
title: Skill Load Truncation Analysis & Fix Plan
status: Draft
author: agent_core
created: 2026-06-28
updated: 2026-06-28
reviewers: [zniverse]
related: [PLAN-0006]
supersedes: ~
superseded_by: ~
tags: [truncation, skills, hygiene, compressor, context-engine]
---
```

## 1. The Problem

`skill_load` 返回的完整技能内容在到达模型之前会被多层截断，导致模型看不到完整的技能指令。

## 2. Current Truncation Architecture

以下所有截断层都作用于 `Role::Tool` 消息（工具执行结果）：

```
skill_load 执行 → 返回完整内容（可能 8000+ chars）
   │
   ├── [Layer 1] hygiene::sanitize()          ← run.rs:787, 每次模型调用前
   │    条件: content.len > 4000
   │    结果: 保留 head(15行) + tail(8行) + signals(5行)
   │    效果: 头部 + 尾部，中间全部丢失
   │
   ├── [Layer 2] compressor::snip_compact()   ← run.rs:1234, maybe_compact 时
   │    条件: content.len > 4000
   │    结果: 保留前 4000 chars + "[... truncated from N chars]"
   │    效果: 粗暴截断前 4000 字符
   │
   └── [Layer 3] ContextSegment (loaded_skills) ← context.rs:291-292
        条件: 500 token budget
        结果: truncate_to_token_budget(500)
        效果: 很多技能内容超过 500 token
```

### What the Model Actually Sees

一次 `skill_load("pptx-generator")`（假设 ~10000 chars skill 内容）：

```
Skill 'pptx-generator' loaded and activated.

== Skill: pptx-generator (v1.0) ==
[truncated: 300 lines / 10500 chars → 4000 char budget]
--- head (15 lines) ---
Use this skill when visual quality and design identity matter for...
CREATE (generate from scratch): "make a PDF", "generate a report",
FILL (complete form fields): "fill in the form", "fill out this PDF",
REFORMAT (apply design to an existing doc): "reformat this document",
This skill uses a token-based design system...
...
--- tail (8 lines) ---
For tables, prefer alternating row colors.
When in doubt, use the primary color palette.
Always include page numbers.
Use consistent heading hierarchy.
...
--- signals ---
(none found)
```

**问题**：模型只看到技能的头部介绍和尾部注意事项，**缺失了中间具体的参数说明、模板结构、API 调用示例等关键指令**。对于 `minimax-pdf`、`deep-research`、`pptx-generator` 这类包含大量指令的技能，这会导致模型无法正确执行。

### Call Sites of Message::tool()

`Message::tool()` 目前不记录 tool name，当前有 3 个调用点：

| 位置 | 代码 |
|------|------|
| `core/src/runtime/run.rs:726` | `Message::tool(call.id.clone(), result.clone())` |
| `core/src/subagent/mod.rs:252` | `Message::tool(call.id.clone(), result.clone())` |
| `core/src/agent/mod.rs:723` | `Message::tool(call.id.clone(), result.clone())` |

每个调用点都有 `call.function.name` 在手边但没传给 Message。

## 3. Root Cause

**所有 `Role::Tool` 消息被统一对待为"工具输出"**。truncation 逻辑（hygiene/compressor）不区分工具类型，对所有工具结果一刀切。

但实际上工具结果有两类：
- **Data output** (`read`, `bash`, `grep`, `webfetch`, etc.)：内容是数据/日志，截断后模型依然可以理解概要。
- **Instruction output** (`skill_load`)：内容是指令/知识，截断会导致模型失去关键行为指导。

## 4. Solution Design

### 4.1 修改 `Message::tool()` — 记录工具名称

```rust
// Before
pub fn tool(tool_call_id: String, content: String) -> Self { ... name: None }

// After
pub fn tool(tool_call_id: String, content: String, tool_name: Option<String>) -> Self { ... name: tool_name }
```

更新 3 个调用点，传入 `call.function.name`。

### 4.2 在 Hygiene 层跳过非截断工具

```rust
// hygiene.rs
const NON_TRUNCATABLE_TOOLS: &[&str] = &["skill_load"];

fn is_truncatable(name: &Option<String>) -> bool {
    match name {
        Some(n) => !NON_TRUNCATABLE_TOOLS.contains(&n.as_str()),
        None => true, // 没有工具名称的默认截断
    }
}

fn truncate_tool_result(msg: &mut Message) -> bool {
    if msg.role != Role::Tool { return false; }
    if !is_truncatable(&msg.name) { return false; } // ← 新增跳过
    // ... 原有截断逻辑
}
```

### 4.3 在 Compressor 层跳过非截断工具

```rust
// compressor.rs
const NON_TRUNCATABLE_TOOLS: &[&str] = &["skill_load"];

pub fn snip_compact(&self, messages: &mut Vec<Message>) -> usize {
    for msg in messages.iter_mut() {
        if msg.role == Tool
            && msg.name.as_deref().map_or(true, |n| !NON_TRUNCATABLE_TOOLS.contains(&n))
            && let Some(ref content) = msg.content
            && content.len() > self.tool_result_budget
        {
            // ... 原有截断逻辑
        }
    }
}
```

### 4.4 ContextSegment (loaded_skills) 不截断

Segment 6 的 `max_tokens` 从 500 增加到 0（不限制）或一个较大值。因为技能内容已经通过 context injection 发送，不应在 segment 层面再次截断。

实际上，auto-trigger 和 manual load 走的是不同路径：
- **Auto-trigger** → `set_loaded_skills()` → Segment 6 (500 token budget)
- **Manual load** → `skill_load` tool → `Role::Tool` message

对于 manual load 路径，Layer 1 (hygiene) 和 Layer 2 (compressor) 是主要问题。对于 auto-trigger 路径，Layer 3 (segment budget) 是主要问题。

当前我们的使用场景是：通过 dropdown 选择 skill → `@skill:name` → 发送消息 → 模型自动调用 `skill_load`。所以主要关注 **manual load 路径**。

但 auto-trigger 路径也应该处理。建议同时修复：
- Segment 6 的 budget 增加到 2000 tokens（足以容纳大多数技能内容）
- hygiene/compressor 跳过 `skill_load`

### 4.5 扩展性考虑

不硬编码 `skill_load` 字符串。更好的做法：

**Option A**: Tool trait 新增 `truncatable()` 方法
```rust
pub trait Tool: Send + Sync {
    // ...
    /// Whether this tool's result should be truncated if too long.
    fn truncatable(&self) -> bool { true }
}
```
- SkillLoadTool 返回 `false`
- ToolRegistry 收集非截断工具名称，生成 `non_truncatable: HashSet<String>`
- 运行时传入 hygiene/compressor

**Option B**: 硬编码列表 + 后续维护
- 简单直接，当前只需 `skill_load` 一个例外
- 后续有新的非截断工具时再维护

**推荐 Option B**，简单且现阶段足够了。后续如果有更多工具要加入（如 `skill_reload`），维护一个常量列表也很容易。

## 5. Files to Modify

| # | File | Change |
|---|------|--------|
| 1 | `core/src/types.rs` | `Message::tool()` 接受 tool_name 参数 |
| 2 | `core/src/runtime/run.rs:726` | 传入 `call.function.name` |
| 3 | `core/src/subagent/mod.rs:252` | 传入 tool name |
| 4 | `core/src/agent/mod.rs:723` | 传入 tool name |
| 5 | `core/src/hygiene.rs` | `truncate_tool_result()` 跳过非截断工具 |
| 6 | `core/src/compressor.rs` | `snip_compact()` 跳过非截断工具 |
| 7 | `core/src/context.rs` | `loaded_skills` segment budget 从 500 → 2000 tokens |

## 6. Verification

- [ ] 编译通过（cargo build）
- [ ] 测试通过（cargo test）
- [ ] 加载一个 8000+ chars 的技能 → hygiene 日志显示 "skipped: skill_load"
- [ ] 模型能看到完整的技能内容（可以在测试中 mock 检查 messages 内容）

## References

- `core/src/hygiene.rs:33-84` — `truncate_tool_result()` 截断逻辑
- `core/src/compressor.rs:186-203` — `snip_compact()` 截断逻辑
- `core/src/types.rs:305-313` — `Message::tool()` 构造函数
- `core/src/runtime/run.rs:712-727` — 工具结果添加到 context
- `core/src/tools/skill.rs:110-131` — SkillLoadTool::execute()
- `core/src/context.rs:291-292` — loaded_skills segment budget

---
*Generated by AI Agent (agent_core)*
*Model: gpt-4o | Timestamp: 2026-06-28T20:13:00+08:00*
