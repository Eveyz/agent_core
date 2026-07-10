# PLAN-0010: Harness Evaluation — Ledger, Report & Model Matrix

```yaml
---
id: PLAN-0010
type: PLAN
title: Harness Evaluation — Ledger, Report & Model Matrix
status: Draft
author: zniverse
created: 2026-07-10
updated: 2026-07-10
reviewers: []
related: [PLAN-0003, PLAN-0005]
supersedes: ~
superseded_by: ~
tags: [eval, harness, metrics, scorecard, mock, live]
---
```

## Objective

为 agent_core **脚手架（harness）** 建立可复现的评测闭环：

1. **Mock 模式**：不测模型智能，只验 runtime 契约（生命周期、工具配对、权限、steer、recovery、subagent 收尾）。
2. **Live 模式**：固定 suite，接真模型，产出 token / 成本 / 时延 / 步数等 metrics 报告。
3. **Compare 模式**：同一 suite + 同一 harness，多模型并排 matrix；可选 harness ablation（固定模型扫开关）。

产物不是「测试绿了」，而是 **`run_ledger` + `summary.json` + `report.md`（+ `matrix.md`）**。

## Background

- 已有 `RunEvent` / `Envelope`、`CacheInfo`/`CacheSummary`、turn timing、workflow `cost_usd` 雏形，但缺少统一 ledger 与评测 runner。
- `core/tests/integration.rs` 只覆盖 Brain/permission 构建，无端到端 run 契约。
- PLAN-0003（自我进化）需要适应度函数；本计划先提供 **固定 eval + scorecard**，进化后续再接。
- 明确边界：**评 harness，不评模型智商**。跨模型对比时，`harness_fail_rate` 应接近 0 且稳定；pass/$/步数可以因模型而变。

## Scope

### In Scope

- Eval crate / CLI：`agent-eval`（或 `cli` 子命令）跑 suite
- `RunLedger` 从事件流汇总 metrics
- Mock LLM 剧本 + 真模型 backend（复用 `OpenAIClient`）
- Grader：`command` / `file_equals` / `exit_code`（首期不做 LLM-as-judge）
- Reporter：JSON + Markdown；`--compare` 出 matrix
- 价格表 `evals/prices/*.toml` 算 `cost_usd`
- Failure taxonomy（脚手架标签）
- 最小 golden suite（mock 10 题 + live 可跑的 10–20 题骨架）
- CI：mock suite 门禁

### Out of Scope

- SWE-bench / 公开榜全量接入（可后续 L2）
- GEPA / DSPy / 自动进化（PLAN-0003）
- LLM-as-judge 主 grader
- 前端 Trace UI 产品化（可复用 ledger，但不在本计划做 UI）
- 改模型路由 / 自动选模产品功能

## Design

### Architecture

```
EvalRunner
  ├─ TaskLoader          evals/suites/<suite>/tasks/*
  ├─ Workspace           tempdir / git worktree
  ├─ LlmBackend
  │    ├─ MockScript     evals/scripts/*.toml
  │    └─ LiveClient     OpenAI-compatible (existing client)
  ├─ HarnessRuntime      Brain + Run (existing)
  ├─ EventCollector      Vec<Envelope<RunEvent>> → RunLedger
  ├─ Grader              command | file_equals | expect_events
  └─ Reporter            summary.json + report.md + matrix.md
```

### Modes

| Mode | Flag | LLM | CI |
|------|------|-----|-----|
| mock | `--mode mock` | 剧本 | PR 门禁 |
| live | `--mode live --model …` | 真模型 | Nightly / 手动 |
| compare | `--compare --model A --model B` | 多模型 | Weekly |

### Failure taxonomy（脚手架标签）

`hung_no_terminal` · `double_terminal` · `tool_unpaired` · `orphan_subagent` · `max_iterations` · `permission_false_positive` · `permission_false_negative` · `approval_deadlock` · `steer_dropped` · `steer_after_terminal` · `pause_resume_corrupt` · `context_lost_after_compact` · `recovery_exhausted` · `process_leak` · `seq_gap` · `cache_ledger_missing`

非门禁：`model_fail` · `grader_fail`（live 任务失败；mock 剧本应保证 grader pass，否则是 runner bug）。

### RunLedger（单次 run 账本）

每题一次，字段：

| 区 | 字段 |
|----|------|
| meta | `task_id`, `suite`, `mode`, `harness{permission_mode,max_iterations,compression,git_sha}`, `model{provider,model_id,price_profile}` |
| result | `pass`, `grader`, `fail_tags[]`, `terminal` |
| metrics | `wall_ms`, `model_ms`, `tool_ms`, `turns`, `tool_calls`, `tool_errors`, `unique_tools[]`, `approvals`, `steers`, `compactions`, `retries`, `tokens_in`, `tokens_out`, `cache_hit`, `cache_miss`, `cache_hit_rate`, `cost_usd` |
| artifacts | `trace_path` |

事件映射（实现时对照）：

| metrics | RunEvent 来源 |
|---------|----------------|
| turns | `TurnStarted`/`TurnEnded` |
| tool_* | `ToolStarted`/`ToolEnded` |
| approvals | `ApprovalRequired`/`ApprovalResolved` |
| steers | `SteerQueued`/`SteerInjected`/`SteerCancelled` |
| compactions | `ContextCompacted` |
| retries | `Error` 中 recovery 文案 / recovery 计数 |
| cache_* | `CacheInfo` / `CacheSummary` |
| terminal | `RunCompleted`/`RunFailed`/`RunCancelled` |
| wall_ms | RunStarted → terminal 墙钟 |
| tokens / cost | live usage + `evals/prices`；mock 可为 0 或剧本填数 |

### Report 北极星（五列）

1. `pass@1`
2. `$ / successful task`
3. `p90 wall_ms`
4. `median tool_calls`
5. `harness_fail_rate`（脚手架标签占比；跨模型应稳且近 0）

报告分三栏：**Harness health** / **Task outcomes** / **Efficiency & cost**。

### 目录布局

```
evals/
  suites/
    contract_v1/           # mock 契约（CI）
      suite.toml
      tasks/
        L1_happy_text/
          task.toml
          script.toml      # mock 剧本
        T1_single_tool/
        ...
    golden_v1/             # live + mock 均可
      suite.toml
      tasks/
        fix_typo/
          task.toml
          prompt.md
          workspace/
          expect/checker.sh
  prices/
    openai_2026_07.toml
  out/                     # gitignore
    <run_id>/
      summary.json
      report.md
      matrix.md            # compare 时
      runs/*.json
      traces/*.jsonl

core/src/eval/               # 或独立 crate eval/
  mod.rs
  ledger.rs
  collector.rs
  grader.rs
  reporter.rs
  mock_llm.rs
  runner.rs
```

> 实现选型：优先 `core/src/eval` + `cli` 暴露 `agent-eval` / `cargo run -p cli -- eval …`，避免过早拆 workspace crate；若 cli 耦合过重再抽 `eval` crate。

### CLI 草图

```bash
# PR / 本地：契约
cargo run -p cli -- eval run --suite contract_v1 --mode mock

# 单模型报告
cargo run -p cli -- eval run --suite golden_v1 --mode live --model openai/gpt-4.1

# 多模型矩阵
cargo run -p cli -- eval run --suite golden_v1 --mode live \
  --model openai/gpt-4.1 --model deepseek/v3 --compare

# harness ablation（固定模型）
cargo run -p cli -- eval run --suite golden_v1 --mode live \
  --model openai/gpt-4.1 --ablate permission,compression,max_iterations
```

### Mock 开工最小 10 题（contract_v1）

| ID | 覆盖 | 失败标签 |
|----|------|----------|
| L1 | 纯 text → Completed | hung / double terminal |
| L2 | cancel → Cancelled + tool 收尾 | tool_unpaired |
| L3 | 不可恢复错 → Failed | hung |
| L4 | max_iterations | max_iterations |
| T1 | 单 tool 配对 | tool_unpaired |
| T5 | tool 中 cancel | tool_unpaired / process_leak |
| P2 | 破坏性命令 → Approval → Allow | permission_false_negative / deadlock |
| P3 | Approval Deny | approval_deadlock |
| S1 | SteerQueued → Injected | steer_dropped |
| A2 | subagent 失败必 Ended | orphan_subagent |

## Phased Delivery

### Phase 0 — Schema & 离线报告（0.5–1 天）

不跑完整 agent，先能从「已有 event 列表 / 手工 fixture」生成 ledger + report。

- 定义 `RunLedger` / `SuiteSummary` serde 类型
- `collector`：`&[Envelope]` → `RunLedger` metrics
- `reporter`：写 `summary.json` + `report.md`
- 1 个手工 `evals/fixtures/sample_trace.jsonl` 验证报告可读

**出口**：`cargo test` 覆盖 collector 聚合；fixture 能出报告。

### Phase 1 — Mock runner + contract_v1（2–4 天）★ 开工主路径

- Mock LLM 剧本（按 call index 返回 tool_calls / text / error）
- 接线现有 `Run`/`Brain`（permission 可自动 Allow/Deny 策略注入）
- EventCollector 订阅 broadcast
- Grader：`expect_events`（契约）+ 可选 `command`
- 落地最小 10 题
- CLI：`eval run --mode mock`
- CI job：mock suite 必须绿；`harness_fail_rate == 0`

**出口**：PR 上 mock 报告 artifact；改 orchestrator/permission/steer 可回归。

### Phase 2 — Live + 成本账本（2–3 天）

- Live backend 复用 `OpenAIClient` + config.toml models
- 从 SSE usage / Cache* 填 tokens；`prices/*.toml` → `cost_usd`
- `golden_v1` 先 10 题（小 workspace + `checker.sh`）
- 报告含 Scorecard 三栏 + cost rollup
- `evals/out/` gitignore

**出口**：一条命令出单模型 `report.md`（含 $ / cache / turns）。

### Phase 3 — Compare & Ablation（1–2 天）

- `--compare` → `matrix.md`
- `--ablate`：permission / compression / max_iterations 变体
- pass@1 与 harness_fail_rate 分列，避免把模型差距当成脚手架差距

**出口**：可贴进周报的 model matrix + harness ablation 表。

### Phase 4 — 加固（按需）

- pass@3（方差）
- process_leak / pause-resume / compact-goal 补题
- Nightly live smoke（5 题）
- 与 PLAN-0003 Reflector 对接：失败标签写入 suggestion 输入

## Tasks

| ID | Task | Phase | Status | ETA |
|----|------|-------|--------|-----|
| T1 | 定 `RunLedger` / `SuiteSummary` / taxonomy 常量 | P0 | Todo | 2026-07-11 |
| T2 | 实现 `EventCollector` + unit tests（fixture trace） | P0 | Todo | 2026-07-11 |
| T3 | 实现 `Reporter`（json + md） | P0 | Todo | 2026-07-11 |
| T4 | Mock LLM script 格式 + 实现 | P1 | Todo | 2026-07-14 |
| T5 | EvalRunner 接线 Brain/Run + 隔离 workspace | P1 | Todo | 2026-07-14 |
| T6 | Grader：`expect_events` + `command` | P1 | Todo | 2026-07-14 |
| T7 | contract_v1 最小 10 题 | P1 | Todo | 2026-07-15 |
| T8 | CLI `eval run --mode mock` + CI | P1 | Todo | 2026-07-15 |
| T9 | Live backend + price table + cost | P2 | Todo | 2026-07-17 |
| T10 | golden_v1 10 题骨架 + live report | P2 | Todo | 2026-07-18 |
| T11 | `--compare` matrix 报告 | P3 | Todo | 2026-07-21 |
| T12 | `--ablate` harness 变体矩阵 | P3 | Todo | 2026-07-21 |

## Milestones

| Milestone | Description | Target |
|-----------|-------------|--------|
| M0 | Ledger + 离线报告从 fixture 生成 | 2026-07-11 |
| M1 | mock contract_v1 CI 门禁 + 报告 | 2026-07-15 |
| M2 | live 单模型 scorecard（含 $） | 2026-07-18 |
| M3 | 多模型 matrix + ablation | 2026-07-21 |

## Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| Run/Brain 难注入 Mock client（client 未 trait 化） | High | High | P1 先最小侵入：test-only mock server 或临时 `Box<dyn …>` / 特征缝；不阻塞 P0 |
| Live 用量字段不全（仅有 cache，无 prompt/completion） | Med | Med | P2 先用 cache + 估算；缺字段在 ledger 标 `tokens_estimated=true` |
| 自动 Approval 与真实 UI 路径不一致 | Med | Med | mock 用 programmatic resolve；单独测 ApprovalRequired 事件存在性 |
| golden 题不稳定（模型非确定） | Med | High | CI 只门禁 mock；live 看趋势与 harness_fail，不设死 pass 阈值 |
| 评测变相变成模型排行榜 | Med | Med | 报告强制分栏；compare 突出 harness_fail_rate |

## Success Criteria

- [ ] `eval run --mode mock --suite contract_v1` 稳定绿，产出 `report.md`
- [ ] CI 对 mock suite 失败（含 `harness_fail_rate > 0`）拦截
- [ ] `eval run --mode live --model <id>` 产出含 turns / wall / tokens / $ / cache 的 scorecard
- [ ] `--compare` 至少 2 个模型并排，且 harness health 与 task outcomes 分列
- [ ] 文档：本 PLAN + `evals/README.md` 说明如何加题

## Open Questions（开工前拍板）

1. **代码位置**：`core/src/eval` vs 新 workspace crate `eval/`？ → 建议先 `core/src/eval` + cli 入口。
2. **Mock 注入点**：trait 化 `OpenAIClient` vs 本地 mock HTTP server？ → 建议优先 **mock HTTP**（少改生产路径），并行评估薄 trait。
3. **Approval 在无 UI 下**：eval 内置 `AutoAllow` / `AutoDeny` / `Scripted` policy？ → 建议 `Scripted`（按 task.toml）。
4. **golden_v1 语言**：Rust-only fixtures 还是混合？ → 建议先 **纯文本/小脚本**，避免依赖完整 cargo 工程拖慢。

## 开工顺序（立刻可做）

```
Day 1:  T1–T3  (P0 schema + collector + reporter + fixture)
Day 2–3: T4–T6 (mock + runner + grader)
Day 4:  T7–T8 (10 题 + CI)
Day 5+: T9–T12 (live / compare)
```

**今天第一刀**：落地 `core/src/eval/{ledger,collector,reporter}.rs` + fixture，不阻塞在 Mock 注入设计上。

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-07-10 | zniverse | Created as Draft — harness eval ledger/report/matrix plan |
