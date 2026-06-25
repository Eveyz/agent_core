# PLAN-0004: Skill 系统激活 + Legacy Agent 迁移

```yaml
---
id: PLAN-0004
type: PLAN
title: Skill 系统激活 + Legacy Agent 迁移到 Runtime
status: Draft
author: zniverse
created: 2026-06-25
updated: 2026-06-25
reviewers: []
related: [ADR-0001, PLAN-0001, PLAN-0002, PLAN-0003]
supersedes: ~
superseded_by: ~
tags: [skills, runtime, migration, cli]
---
```

## Objective

1. 在 Runtime 路径（Brain/Run）激活 Skill 系统——扫描、catalog 注入、auto-trigger、skill tools
2. 将 CLI 从 Legacy Agent 路径迁移到 Runtime 路径（Brain + RunManager）

## Background

### Skill 系统现状

Skill 系统的 progressive disclosure 设计是合理的：
- **第一层**：catalog（所有 skill 的名字+描述）注入 Segment 6，让模型知道有什么 skill 可用
- **第二层**：auto-trigger 或 `skill_load` 时把完整 skill body 注入 context

但实现有两个断裂：
- `Brain::build_skill_manager` 直接返回 `None`——Run 路径完全没有 skill
- Legacy Agent 路径有 skill，但 Segment 6 刷新策略是 `OnDemand`，auto-trigger 后不刷新

### Legacy Agent 迁移

CLI 目前用 `AgentBuilder` → `Agent`，TUI 直接调 `agent.run_with_events()`。
Runtime 路径用 `Brain` + `RunManager` → `Run`，通过 command/event 通道通信。
两条路径的 API 完全不同，迁移需要重写 CLI 的 TUI 交互层。

## Scope

### In Scope

- Brain 构造时扫描 skills、注册 skill tools
- Run 路径注入 catalog 到 Segment 6 + auto-trigger
- Segment 6 刷新策略改为 PerTurn
- CLI 迁移到 Runtime 路径

### Out of Scope

- Skill manifest 格式变更
- Skill 内容分段加载（当前全量够用）
- Subagent 内 skill 支持
- 前端 skill UI

## Tasks

| ID | Task | 涉及文件 | Status |
|----|------|---------|--------|
| S1 | Brain 构建 SkillManager + 注册 skill tools | `core/src/runtime/brain.rs` | Todo |
| S2 | Run 注入 catalog + auto-trigger | `core/src/runtime/run.rs` | Todo |
| S3 | Segment 6 刷新策略改为 PerTurn | `core/src/context.rs` | Todo |
| S4 | CLI 迁移到 Runtime | `cli/src/main.rs`, `cli/src/tui/` | Todo |
| S5 | cargo check + cargo test | — | Todo |

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-25 | zniverse | Created as Draft |
