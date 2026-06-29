# PLAN-0007: Cronjob System Implementation

```yaml
---
id: PLAN-0007
type: PLAN
title: Cronjob System Implementation
status: Draft
author: agent_core
created: 2026-06-29
updated: 2026-06-29
reviewers: [zniverse]
related: []
supersedes: ~
superseded_by: ~
tags: [cronjob, backend, frontend, scheduler]
---
```

## Objective

实现一个端到端的定时任务系统（Cronjob System），允许用户配置和管理后台自动运行的 Agent 任务。该系统涵盖后端调度骨架、持久化存储以及前端的配置界面。

## Background

为了让 Agent 能够定期执行特定的后台任务（如定期检查代码库、总结日志、执行常规维护脚本等），我们需要一个定时调度机制。用户需要能够直观地在前端侧边栏发起调度，并为任务设定一系列参数（周期、目标项目、使用的技能、权限范围等），后端则需要稳定可靠地执行这些定时任务。

## Scope

### In Scope
- **Backend**: 
  - 集成 `tokio-cron-scheduler` 进行任务的基于 Cron 表达式的调度。
  - 在 SQLite 数据库中创建 `cronjobs` 表和 `cronjob_runs` 表。
  - 实现增删改查（CRUD）定时任务的 Tauri Commands。
  - 调度触发时，初始化并运行一个带有指定上下文（Prompt, Project, Skills, Permission Level）的 Agent 会话。
- **Frontend**: 
  - 在侧边栏（Sidebar）添加一个 `Schedule Task` 按钮/入口。
  - 实现一个 `ScheduleTaskModal`，支持配置：Name, Cadence (Cron), Prompt, Project, Skills, Permission Level。
  - 实现任务列表视图或在某处展示当前的定时任务（基础支持）。

### Out of Scope
- 高可用/分布式调度（目前为单机本地 SQLite + Tokio）。
- 过于复杂的 Cron 表达式可视化构造器（V1 阶段可直接让用户输入 Cron 表达式，或提供几个简单的预设，如 "Every hour", "Every day"）。

## Tasks

| ID | Task | Owner | Status | ETA |
|----|------|-------|--------|-----|
| T1 | 设计并实现 SQLite 数据库 Schema (cronjobs, cronjob_runs) | agent_core | Todo | 2026-06-29 |
| T2 | 在 core 中集成 `tokio-cron-scheduler` 并实现调度循环 | agent_core | Todo | 2026-06-29 |
| T3 | 实现 Tauri Commands (create, list, delete, pause/resume) | agent_core | Todo | 2026-06-29 |
| T4 | 实现调度触发后的 Agent 执行逻辑绑定 (带入参数) | agent_core | Todo | 2026-06-29 |
| T5 | 前端：在 Sidebar 添加入口，并编写 Modal UI 组件 | agent_core | Todo | 2026-06-29 |
| T6 | 前后端联调测试及界面美化优化 | agent_core | Todo | 2026-06-29 |

## Milestones

| Milestone | Description | Target Date |
|-----------|-------------|-------------|
| M1 | 后端调度和存储骨架就绪 | 2026-06-29 |
| M2 | 前端配置 Modal 及 Sidebar 入口就绪 | 2026-06-29 |
| M3 | 端到端测试通过 (定时任务准时执行) | 2026-06-29 |

## Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| 多个任务并发执行导致资源枯竭 | High | Med | 在后端调度器中对并发运行的 Agent 任务数进行限制，或加入队列排队执行。 |
| Cron 表达式不合法导致崩溃 | Med | High | 在前端和后端双重校验 Cron 表达式的合法性，不合法则拒绝创建。 |

## Success Criteria

- 用户可以在侧边栏点击 "Schedule Task" 调出弹窗。
- 用户可以输入任务名、Cron 周期、Prompt，并选择 Project、Skills、Permission Level。
- 保存后任务配置存入 SQLite。
- 达到设定时间后，后台自动唤起 Agent，执行指定的 Prompt 并在正确的权限和技能上下文中运行。

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-29 | agent_core | Created as Draft |
