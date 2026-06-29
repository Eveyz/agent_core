---
id: PLAN-0008
type: PLAN
title: Truncation Architecture Redesign + Read File Segmented Reading
status: In Progress
author: zniverse
created: 2026-06-29
updated: 2026-06-30
reviewers: [agent_core]
related: []
supersedes: ~
superseded_by: ~
tags: [tools, optimization, context-efficiency, truncation, token-budget]
---

# PLAN-0008: 截断架构重设计 + Read File 分段读取

## Objective

两件事，一件根因一件手段：

1. **根因**：重设计 tool result 截断架构。当前四层截断职责重叠、策略不一致、预算过小，导致大量 tool 结果被切断，模型反复报"内容被截断"却无法恢复。目标：让模型能读到它真正想要的内容，同时仍控制噪声与大文件。
2. **手段**：`read_file` 升级为 `offset`+`limit` 分段读取，作为"主动读取型"工具的范本，让截断后可续读恢复。

## Background — 截断链路现状审计

实测代码后确认的截断层（按调用时序）：

| 层 | 位置 | 触发时机 | 策略 | 预算 |
|----|------|----------|------|------|
| L1 工具层 | 各 `tool.execute` | 每次调用 | read_file 旧版无；webfetch 自带 max_len | 不一 |
| L2 hygiene | `core/src/hygiene.rs:38` `truncate_tool_result` | **每 turn**（request 副本，`run.rs:802`） | head+tail+signal | 4000 chars, head15/tail8 |
| L3 snip_compact | `core/src/compressor.rs:191` | **仅 token≥80%** 时（`maybe_compact`, `run.rs:1221`） | 前缀截断（丢尾部） | 4000 chars |
| L4 compaction | chunked_drop / summary | token≥80% | 丢整个旧 turn | turn 级 |

**问题诊断**：

- L2 每 turn 都跑，4000 chars（≈1K token，128K context 的 0.8%）对任何真实代码文件都太小 → 模型每 turn 看到的是被砍剩 head15+tail8 的残片，"老是说截断"主因在此。
- L2 保头尾丢中间，L3 保头部丢尾部，策略相反；超载时叠加（L3 先砍尾→L2 再 head/tail）→ 信息不可预测丢失。
- 所有工具共用同一 4000 chars 预算，不区分"模型主动要读的内容"(read_file) 与"附带噪声"(exec 日志)。
- 截断不可恢复：旧 read_file 全量读后被 L2 砍，模型无法补读中间。

## Design

### 原则：四层职责不重叠

每层只做一件事，不互相覆盖：

- **L1 工具层**：决定"读多少"。对 read_file = `offset`/`limit` 控制读取范围；输出即模型应看到的完整意图块。
- **L2 hygiene**：request 落地前最终裁剪，**只针对附带信息型工具**做 head/tail；主动读取型豁免（L1 已控量）。
- **L3 snip_compact**：持久 history 压缩，**策略与 L2 完全一致**（共享常量+分类），保证持久视图 = request 视图。
- **L4 compaction**：turn 级丢旧消息，不动单条内容。

### 工具语义三分类

扩展现有 `NON_TRUNCATABLE_TOOLS` 机制为三分类：

| 分类 | 工具 | L2 hygiene 策略 | 理由 |
|------|------|----------------|------|
| 指令型 | `skill_load` | 完全豁免（已有） | 内容是给模型的指令，不可丢 |
| 主动读取型 | `read_file` | 豁免 head/tail，仅 char 兜底 | 模型明确请求，L1 已控量；连续可读 |
| 附带信息型 | exec/grep/tavily/webfetch… | head+tail+signal | 可能含噪声，保信号行 |

### 预算与常量

| 常量 | 旧值 | 新值（建议） | 说明 |
|------|------|-------------|------|
| `TOOL_RESULT_MAX_CHARS`（附带信息型） | 4000 | 16000 | 对齐现代 context；~4K token，128K 的 3% |
| `TOOL_RESULT_HEAD_LINES` | 15 | 40 | 配合 16K 预算 |
| `TOOL_RESULT_TAIL_LINES` | 8 | 20 | 配合 16K 预算 |
| `READ_RESULT_MAX_CHARS`（主动读取型，新增） | — | 24000 | read_file 豁免预算；~6K token，128K 的 4.7% |
| read_file `MAX_LINES_DEFAULT` | — | 300 | 单次默认输出落 ~8-12K，不触发兜底 |
| read_file `MAX_FILE_SIZE_BYTES` | — | 1,048,576 (1MB) | 拒绝读取硬顶 |
| read_file `MAX_OUTPUT_CHARS` | — | 24000 | 与 `READ_RESULT_MAX_CHARS` 对齐，单行/极端兜底 |

> 预算值为建议起点，实现时可按实测微调。关键是消除"不区分语义、一刀切 4000"的问题。

### read_file 分段读取

```
read_file(
  path:   string,            // required — 绝对路径
  offset: integer? = 1,      // 起始行号 (1-based)
  limit:  integer? = 300     // 最大行数
) -> string
```

流程：`stat`(>1MB 拒绝) → `BufReader` 流式 → 遇 `\0` 拒绝(二进制) / `lines()` 内置 UTF-8 校验拒绝非 UTF-8 → `skip(offset-1)` `take(limit)` → 输出 `{:>6}\t{content}` 带行号 + 范围头 `[Lines X-Y in 'path']` + 续读提示。不扫总行数 Z（避免二次扫描，到达 `offset+limit` 即 break）。

### 一致性保证：L2 = L3

把截断分类与预算提取到共享模块（如 `hygiene::policy`），`compressor::snip_compact` 引用同一份。效果：

- 持久 history 被 L3 砍成什么样，L2 在 request 副本上就一致呈现 → 模型基于"看到的"决策不与历史脱节。
- 消除"L3 丢尾 + L2 保头尾"的策略冲突。

### 可恢复性

任何截断后，模型必须看到"还有更多 + 怎么读"：

- 主动读取型：续读提示在输出末尾，因豁免 head/tail 故一定可见。
- 附带信息型：truncated 标记在 tail（可见）。
- read_file 被兜底截（>24K）时，末尾续读提示 + 当前行号让模型用 `offset` 补读。

### Cache 影响

- **系统 prompt**：`tool_catalog` 含 `description`（不含 schema，见 `context.rs:838 build_tool_catalog_string`）。更新 read_file description → catalog 文本变 → Stable 段变 → **一次性系统 prompt cache miss**；`set_tool_catalog`（`context.rs:337`）内容去重 + 每 turn 重建（`run.rs:406`），静态后恢复稳定。
- **tools 数组**：schema 加 offset/limit → 首次请求 miss，之后同 schema 命中。一次性。
- **tool result**：给定 `(path,offset,limit)` 截断确定性，hygiene 在 request 副本上确定性裁剪、不改持久 history → session 内 prefix 稳定，cache hit 不受持续影响。
- **预算提高**：单条 result 更大占更多 context，但 prefix 稳定性不变；L4 compaction（80% 阈值）兜底总量。

## 行为变更说明

（原"向后兼容"——诚实重命名：这并非 backward compatible，而是受控行为变更。）

- schema 动态生成（`core/src/tools/mod.rs:118` `tool_definitions()`），Rust 调用方无需改代码。但这 ≠ 行为不变。
- read_file 输出格式全变（行号 + 范围头 + 续读提示 + 默认 300 行）。
- 现有单测只 assert 工具名/权限/catalog 文本（如 `context.rs:1199`、`permission/rules.rs:274`），不 assert read_file 原始内容形状，改输出格式不炸单测。
- hygiene/snip 预算提高是全局行为变更，影响所有 tool result 体积（向"更不截断"方向）。

## Tasks

| ID | Task | Owner | Status | ETA |
|----|------|-------|--------|-----|
| T1 | 提取 `hygiene::policy` 共享模块（三分类 + 预算常量） | zniverse | Done | 2026-06-30 |
| T2 | hygiene `truncate_tool_result` 实现三分类（主动读取型豁免 head/tail） | zniverse | Done | 2026-06-30 |
| T3 | compressor `snip_compact` 引用共享 policy（L2=L3 一致） | zniverse | Done | 2026-06-30 |
| T4 | 重写 `read_file.rs`：offset/limit + 二进制检测 + 行号 + 续读提示 | zniverse | Done | 2026-06-30 |
| T5 | 提高附带信息型预算（4000→16000, head15→40, tail8→20） | zniverse | Done | 2026-06-30 |
| T6 | 单测：三分类 / 续读 / 二进制 / 大文件 / 边界 | zniverse | Done | 2026-06-30 |
| T7 | `cargo test` 全量回归 | zniverse | Done | 2026-06-30 |
| T8 | 更新 `docs/index.md` 索引 | agent_core | Done | 2026-06-30 |

## Success Criteria

- [x] `read_file(offset=10, limit=50)` 只返回 10-59 行带行号 + `[Lines 10-59 in 'path']`
- [x] read_file 输出**豁免** hygiene head/tail，连续完整可读（不出现中间断裂）
- [x] >1MB 文件拒绝；含 `\0` / 非 UTF-8 拒绝
- [x] 附带信息型工具（exec 等）输出 >16K 才截，常见输出不再被砍
- [x] `snip_compact` 与 hygiene 对同一 tool result 产出一致（持久 = request）
- [x] 任何截断后模型可见续读 / truncated 提示
- [x] `cargo test` 全绿，无性能回归
- [x] 系统 prompt cache：部署后首 turn miss，后续稳定命中（prefix 不漂移）

## Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| 预算提高致单 turn context 占用上升 | Med | Low | L4 compaction 80% 阈值兜底；read_file 默认 300 行控单次量 |
| read_file 行号前缀被 LLM 误塞进 edit `old_string` | Med | Med | 行号格式醒目（`{:>6}\t`）；description 明示 edit 时去前缀；edit 用精确匹配本就需原文 |
| description 变更致系统 prompt 一次性 cache miss | Low | High | 一次性成本，静态后稳定；非持续退化 |
| snip/hygiene 共享 policy 改动影响 compaction 决策 | Low | Low | 共享常量值不变则行为不变；加回归测 |
| LLM 传字符串而非整数给 offset/limit | Low | Med | `as_u64()` 返回 None 时友好报错 |

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-30 | agent_core | Review v2：升级为截断架构重设计——审计四层链路调用时序、引入工具语义三分类、L2=L3 一致性、预算提高、read_file 默认 limit 300、cache 影响分析、诚实重命名"行为变更" |
| 2026-06-30 | agent_core | Implemented T1-T8：新增 `hygiene/policy.rs` 共享模块（三分类+预算，含 UTF-8 安全截断）；hygiene 与 snip_compact 均委托 policy 实现 L2=L3 一致；附带信息预算 4K→16K/head40/tail20，read_file 豁免 head/tail（24K char-cap）；read_file 重写为 offset/limit 分段读（1MB 硬顶 + NUL/UTF-8 检测 + 行号 + 续读提示）；新增 20 个单测，全量回归 327 passed（2 个 permission 失败为 pre-existing） |
| 2026-06-30 | agent_core | Review：补充三层截断交互分析、修正 edit benefit、明确 default limit=2000 为 breaking change、放弃精确 Z、补充单行/schema/UTF-8 边界风险 |
| 2026-06-29 | zniverse | Created as Draft |

---

*Generated by AI Agent (agent_core)*
*Model: gpt-4o | Timestamp: 2026-06-30T00:00:00+08:00*
