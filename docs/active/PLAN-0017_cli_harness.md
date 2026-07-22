# PLAN-0017: CLI 主力 Harness 升级

```yaml
---
id: PLAN-0017
type: PLAN
title: CLI 主力 Harness 升级
status: Done
author: zniverse
created: 2026-07-23
updated: 2026-07-23
reviewers: []
related: []
supersedes: ~
superseded_by: ~
tags: [cli, harness]
---
```

## 目标

把 CLI 从"能跑"升级到"主力工具"级别，覆盖日常使用的最后一公里场景。

## 现状审计

CLI 核心功能完整度：**10/10**（含 TUI 接入，见 PLAN-0018）

✅ **已就位**：REPL、oneshot、会话管理、规划系统、权限系统、MCP、Eval、管道 stdin、历史（含 Ctrl+D）、零交互 REPL 启动、Shell completion、`config show/validate`、详细 `-V`、`--dry-run`、TUI（CliState）  
✅ **TUI**：已接 CliState / RunEvent（详见 [PLAN-0018](./PLAN-0018_tui_revival.md)）

## 实施计划

### P0 — ✅ 已完成

#### 任务 0.1：管道 / 非 TTY stdin 支持 ✅
#### 任务 0.2：历史在所有退出路径持久化 ✅
#### 任务 0.3：REPL 启动可跳过交互提问 ✅

### P1 — ✅ 已完成

#### 任务 1.1：Shell Tab 补全 ✅

```bash
ageverse completion zsh > ~/.zsh_completions/_ageverse
ageverse completion bash
ageverse completion fish
```

实现：`cli/src/completion.rs` + `SubCommand::Completion`（手写脚本，非 argh DynamicCompletion）。

#### 任务 1.2：配置子命令 ✅

```bash
ageverse config show
ageverse config validate
ageverse config validate --probe
```

实现：`cli/src/config_cmd.rs`。`validate` 仅本地；`--probe` 对 default model 发极小 chat 探测。

### P2

#### 任务 2.1：`--version` 详细输出 ✅

```bash
$ ageverse -V
ageverse 0.1.0
commit: … (YYYY-MM-DD)
build: debug|release
```

`cli/build.rs` 注入 `GIT_COMMIT_HASH` / `GIT_COMMIT_DATE` / `BUILD_PROFILE`。

#### 任务 2.2：Dry-run 模式 ✅

```bash
ageverse -p "…" --dry-run
```

语义：仍调 LLM；`DryRunHook` 在 `PreToolUse` veto，结果回灌模型（`Hook vetoed: [dry-run] …`）。

#### 任务 2.3：TUI 接 CliState — ✅ 已完成

见 [PLAN-0018](./PLAN-0018_tui_revival.md)：`CliState`/`RunManager`/`RunEvent`、五档审批、共享 slash dispatch、UX 拉满。

## 非目标

- ❌ 不重写核心逻辑
- ❌ 不把 GUI 专属能力（图片、`/goal` `/plan`、工作流）搬进 TUI
- ❌ 不在 CLI 加新核心能力——CLI 是接入层

## 验收标准（计划完成时）

- ✅ `cat file.rs | ageverse -p "review"` 正常；`echo x | ageverse` 走 oneshot 不进 REPL
- ✅ Ctrl+D 退出后 ↑ 仍能看到历史
- ✅ 带 flag / 默认路径启动 REPL 无需三次 Y/n
- ✅ `ageverse completion zsh` 输出可用脚本
- ✅ `ageverse config validate` 能报告本地配置问题
- ✅ `ageverse -V` 含 commit / build
- ✅ `ageverse -p "…" --dry-run` 不执行工具副作用
- ✅ 现有 REPL / session / todo / eval 不受影响

## 风险评估

**低风险**：改动主要在 `cli/`。dry-run 仅在显式 `--dry-run` 时注册 hook。
