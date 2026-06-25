# PLAN-0003: Context Engine 修复与激活

```yaml
---
id: PLAN-0003
type: PLAN
title: Context Engine 修复与激活
status: Draft
author: zniverse
created: 2026-06-25
updated: 2026-06-25
reviewers: []
related: [ADR-0001, PLAN-0001, PLAN-0002]
supersedes: ~
superseded_by: ~
tags: [context, compression, context-engine]
---
```

## Objective

修复 Context Engine 的 7 段上下文装配和 5 阶段压缩管线中的 7 个实质问题，让 context 管理真正可靠工作。

## Background

诊断发现 7 段 Context Engine 和 5 阶段压缩管线设计合理，但实现中有多个断裂点：

1. **80% drain 暴力丢弃** — `trim_to_fit` 在 Stage 1-3 无效后直接 `drain` 最旧消息，不做摘要
2. **`chunk_compact` 破坏消息结构** — 把 `assistant(tool_call) + tool(result)` 合并成 `system` 消息，破坏 API 期望的消息配对
3. **Run recovery 用 `micro_compact`** — 只做粗糙文本摘要，不如 Agent 路径的 `force_compact`（LLM 摘要）
4. **`system_prefix_budget` 固定 2200** — tool catalog + principles 很容易超
5. **LLM compact 解析静默失败** — JSON parse 失败直接 return，不 fallback
6. **`dedup_compact` format bug** — tool name 丢失，参数错位
7. **`gradient_compact` 空壳** — 只跑 Stage 1-3，无 gradient 逻辑

## Scope

### In Scope

- `trim_to_fit` 改为 Stage 1-3 无效后调 LLM 摘要，而非暴力 drain
- `chunk_compact` 限制只处理老消息（跳过最近 N 条），保护消息结构
- Run 路径 `try_recover` 改用 `force_compact`（LLM 摘要）
- `system_prefix_budget` 按比例计算
- LLM compact 解析失败时 fallback 到 `micro_compact`
- `dedup_compact` format 字符串修复
- `gradient_compact` 清理：要么实现要么简化注释
- Active Memory 段加 recall search（如果有 memory）

### Out of Scope

- 替换 `rough_token_count` 为 tiktoken（当前估算够用）
- Context Engine 重构（7 段架构不变）
- 前端改动（对前端透明）
- `trim_to_fit_legacy` 维护（已废弃）

## Tasks

| ID | Task | 涉及文件 | Status |
|----|------|---------|--------|
| C1 | `trim_to_fit` 改为 LLM 摘要而非 drain | `core/src/context.rs` | Todo |
| C2 | `chunk_compact` 保护最近 N 条消息 | `core/src/compressor.rs` | Todo |
| C3 | Run `try_recover` 改用 `force_compact` | `core/src/runtime/run.rs` | Todo |
| C4 | `system_prefix_budget` 按比例计算 | `core/src/context.rs` | Todo |
| C5 | LLM compact 解析失败 fallback | `core/src/context.rs` + `run.rs` + `agent/mod.rs` | Todo |
| C6 | `dedup_compact` format bug 修复 | `core/src/compressor.rs` | Todo |
| C7 | `gradient_compact` 清理 | `core/src/compressor.rs` | Todo |
| C8 | Active Memory recall search | `core/src/runtime/run.rs` + `core/src/agent/mod.rs` | Todo |
| C9 | cargo check + cargo test | — | Todo |

## Design

### C1: `trim_to_fit` 改为 LLM 摘要而非 drain

当前 `trim_to_fit` 在 Stage 1-3 无效后直接 drain 最旧消息。改为：不在 `trim_to_fit` 里 drain，而是让调用方（`run_turn` / `Agent::run_turn`）在 `trim_to_fit` 后检查是否仍超阈值，如果是则调 `maybe_llm_compact`。

具体改动：
- `trim_to_fit` 删掉 drain 逻辑，只跑 Stage 1-3 + 返回结果
- `maybe_llm_compact` 的触发阈值从 95% 降到 80%（和 `auto_compact_threshold` 一致）
- 这样形成连续的压缩链：Stage 1-3（80%）→ Stage 4 LLM 摘要（80% 仍超时）→ 如果还超，micro_compact 兜底

### C2: `chunk_compact` 保护最近 N 条消息

给 `chunk_compact` 加 `protect_recent` 参数，跳过最后 N 条消息不做合并。N 默认为 8（约 2-3 轮对话）。

```rust
pub fn chunk_compact(&self, messages: &mut Vec<Message>) -> usize {
    let protect = 8.min(messages.len());
    let limit = messages.len().saturating_sub(protect);
    // 只处理 0..limit 范围内的消息
}
```

### C3: Run `try_recover` 改用 `force_compact`

把 Run 路径的 `try_recover` 中的 `micro_compact` 改为调用 LLM 摘要（和 Agent 路径的 `force_compact` 一致）。需要给 Run 加一个 `force_compact` 方法。

### C4: `system_prefix_budget` 按比例计算

```rust
// 在 new() 中
system_prefix_budget: (max_tokens as f64 * 0.08) as usize,
```

128K context → 10240 tokens 的 system prefix 预算，绰绰有余。

### C5: LLM compact 解析失败 fallback

`maybe_llm_compact` 中 JSON parse 失败时，fallback 到 `micro_compact` 而非静默 return。

### C6: `dedup_compact` format bug

```rust
// 修复前
format!("[Same output as message #{} ({}): {}]", first_idx + 1, truncated, content.len())

// 修复后
format!("[Same as #{} — {}... ({} chars)]", first_idx + 1, truncated, content.len())
```

### C7: `gradient_compact` 清理

`gradient_compact` 实际只跑 Stage 1-3，gradient 逻辑未实现。把方法重命名为 `run_stages_1_3`，删除误导性注释。`run_pipeline` 直接调它。

### C8: Active Memory recall search

在 `refresh_context_segments` 中，如果有 memory，用当前最后一条 user message 做 recall search，把 top-3 结果注入 Segment 5。

```rust
// Segment 5: ACTIVE MEMORY — core memory + recall search
if let Some(ref mem) = self.brain.memory {
    let m = mem.lock();
    let mut mem_str = m.core().to_context_string();
    
    // Recall search: 用最近的 user message 搜索相关历史对话
    if let Some(last_user) = self.context.raw_messages().iter().rev()
        .find(|msg| msg.role == Role::User)
        .and_then(|msg| msg.content.as_deref())
    {
        let results = m.search_conversations(last_user, 3);
        if !results.is_empty() {
            let recall_str: String = results.iter()
                .map(|r| format!("  • [{}] {}", r.0, r.1))
                .collect::<Vec<_>>().join("\n");
            mem_str.push_str(&format!("\nRecall:\n{}", recall_str));
        }
    }
    
    if !mem_str.is_empty() {
        self.context.set_active_memory(&mem_str);
    }
}
```

## Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| LLM 摘要调用增加延迟 | Med | Med | 只在 80% threshold 触发时调，正常对话不会触发 |
| chunk_compact 保护后压缩效果降低 | Low | Low | 只保护最近 8 条，老消息仍可压缩 |
| recall search 增加每 turn 延迟 | Low | Med | embedding 查询很快（ms 级），top-3 结果不大 |
| force_compact 在 Run 中可能死锁 | Low | Low | 用独立的 chat_completion 调用，不经过 stream |

## Success Criteria

- 对话超过 80% context 时自动触发 LLM 摘要，不暴力 drain
- 最近 2-3 轮的 tool_call/tool_result 消息结构不被 chunk_compact 破坏
- Run 路径的 recovery 和 Agent 路径行为一致
- system prefix 不会被 2200 token 硬限制截断
- LLM compact 失败时有 fallback 而非静默丢弃
- `cargo test` 全部通过
- `cargo check` 无新增 warning

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-25 | zniverse | Created as Draft |
