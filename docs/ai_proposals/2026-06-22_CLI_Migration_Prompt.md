# CLI Migration to RunManager — Session Prompt

## 背景

`agent_core` 项目已经完成了 Agent Runtime 管理系统的重构（Phase 1-6）。
新的 `runtime` 模块（`core/src/runtime/`）提供了 Brain + Run + RunManager 架构，
替代了旧的 `Agent` 单体结构。Tauri 后端已迁移完成，前端已适配。

**唯一未迁移的是 CLI**（`cli/src/`，约 6000 行）。CLI 仍走旧 `Agent` 路径，
相关的 `Agent`/`AgentState`/`global_pending_approvals` 已标记 `#[deprecated]`。

## 你的任务

把 CLI 从旧 `Agent` 路径迁移到新的 `RunManager` + `Run` 架构。
迁移完成后，删除 `Agent`、`AgentState`、`global_pending_approvals` 等废弃代码。

## 当前架构

### 新 runtime 模块（`core/src/runtime/`）

```
runtime/
├── mod.rs          — 模块入口 + re-exports
├── brain.rs        — Brain: 可复用大脑（config/memory/skills/client factory）
├── run.rs          — Run: 独立执行空间（状态机 + context + supervisor + JoinSet）
├── manager.rs      — RunManager: 创建/路由/追踪 Run
├── state.rs        — RunState: 8 状态 FSM（Created→Running→Completed/Cancelled/Failed...）
├── command.rs      — RunCommand: Start/Pause/Resume/Cancel/Steer/Approve/Answer
├── event.rs        — RunEvent: 生命周期+状态转换+turn+model+tool+审批+进程事件
├── approval.rs     — ApprovalResolver: per-Run 审批通道
├── supervisor.rs   — ProcessSupervisor: 进程组 kill（零泄漏）
└── event_log.rs    — EventLog: JSONL append-only 持久化
```

### 关键 API

```rust
// 创建 RunManager（从 config 文件）
let manager = RunManager::load_config("config.toml")?;

// 创建一个 Run（返回 run_id）
let run_id = manager.create_run("user message", session_id).await?;

// 订阅事件流
let mut rx = manager.subscribe(&run_id).await?;

// 发送命令
manager.command(&run_id, RunCommand::Start).await?;
manager.command(&run_id, RunCommand::Cancel).await?;
manager.command(&run_id, RunCommand::Pause).await?;
manager.command(&run_id, RunCommand::Resume).await?;
manager.command(&run_id, RunCommand::Steer { message: "use a different approach".into() }).await?;
manager.command(&run_id, RunCommand::Approve { prompt_id, choice }).await?;

// 查询状态
let state = manager.run_state(&run_id).await?; // RunState
let runs = manager.list_runs().await;

// Brain 操作（模型切换等）
manager.brain().switch_model("openai/gpt-4o")?;
manager.brain().config;  // 访问 Config
```

### RunEvent 格式

RunEvent 用 `#[serde(tag = "event", rename_all = "snake_case")]` 序列化：
```json
{ "event": "run_started" }
{ "event": "state_changed", "from": "created", "to": "running" }
{ "event": "turn_started", "index": 0 }
{ "event": "message_update", "delta": { "Text": "hello" } }
{ "event": "tool_started", "call_id": "...", "name": "bash", "args": {...} }
{ "event": "tool_ended", "call_id": "...", "result": "...", "is_error": false }
{ "event": "approval_required", "prompt_id": "...", "tool_name": "bash", ... }
{ "event": "run_completed", "final_text": "..." }
{ "event": "run_cancelled", "reason": "..." }
```

CLI 可以用 `RunEvent` 直接处理，也可以用 `RunEvent::from_agent_event()` 桥接旧逻辑。

## CLI 当前结构

```
cli/src/
├── main.rs           (1718 行) — REPL 模式 + 工具注册 + 命令处理
├── cli_completer.rs  (52 行)   — Tab 补全
├── tui/
│   ├── mod.rs        (317 行)  — TUI 入口 + run_app + process_command
│   ├── state.rs      (1197 行) — AppState + 事件处理 (handle_agent_event)
│   ├── render.rs     (309 行)  — 渲染
│   ├── input.rs      (361 行)  — 输入处理
│   └── markdown.rs   (495 行)  — Markdown 渲染
└── tui/widgets/      (1512 行) — UI 组件
```

### CLI 对 Agent 的 54 处调用

按频率排序：
- `agent.tool_registry_mut()` ×5 — 注册 todo/skill/task/subagent 工具
- `agent.config` ×5 — 读 permissions/mcp 配置
- `agent.current_model_config()` ×4 — 获取模型配置给 subagent/task 工具
- `agent.current_model()` ×4 — 显示当前模型名
- `agent.context_messages()` ×4 — /context 命令、session 保存
- `agent.tool_execution_mode()` ×3 — 显示工具模式
- `agent.memory` ×3 — /memory 命令
- `agent.context_token_count()` ×3 — /tokens 命令
- `agent.clear_context()` ×3 — /clear /new 命令
- `agent.tool_registry()` ×2 — 列出工具名
- `agent.state()` ×2 — /state 命令
- `agent.set_tool_execution_mode()` ×2 — /tool-mode 命令
- `agent.cancel_token` ×2 — abort
- `agent.switch_model()` ×1 — /model 命令
- `agent.steer()` ×1 — /steer 命令
- `agent.set_temperature()` ×1 — /temp 命令
- `agent.set_max_tokens()` ×1 — /max-tokens 命令
- `agent.rewind_context_to()` ×1 — /rewind 命令
- `agent.permission_policy_mut()` ×1 — /perm 命令
- `agent.list_models()` ×1 — /models 命令
- `agent.follow_up()` ×1 — /follow-up 命令
- `agent.context_mut()` ×1 — session 恢复
- `agent.context_cache_hint()` ×1 — /context 命令
- `agent.clear_all_queues()` ×1 — /clear-queues 命令

### TUI 模式（`run_tui_mode` + `tui/`）

- `tui/mod.rs:run_app` 接收 `Arc<tokio::sync::Mutex<Agent>>`
- 用 `agent.lock().await` 获取锁后调用各种方法
- `agent.run_with_events(&req, |event| { tx.send(AppEvent::Agent(event)) })` 发起执行
- `tui/state.rs:handle_agent_event` 处理 `AgentEvent`（已处理 `Aborted` 变体）
- `process_command` 处理 slash 命令（list_models, switch_model, clear, register_model 等）

### REPL 模式（`main` 函数）

- 直接用 `agent.run_with_events(input, |event| { ... })` 执行
- 事件处理内联在闭包里（打印 thinking/text/tool output）
- 审批用 `global_pending_approvals()` 自动 AllowSession

## 迁移策略建议

### 方案 A：RunManager 包装层（推荐）

不改 TUI 结构，把 `Arc<Mutex<Agent>>` 替换为 `Arc<RunManager>` + 当前 run_id。

1. `run_tui_mode` 和 `main` 改为构建 `RunManager` 而非 `Agent`
2. 工具注册：RunManager 不直接暴露 tool_registry_mut。
   需要在 Brain 层面注册全局工具（todo/skill/task/subagent），
   或在 `create_run` 后通过 Run handle 注入。
   **可能需要给 Brain 加一个 tool 注册回调或扩展 build_tool_registry。**
3. `agent.run_with_events(req, cb)` → `manager.create_run(req, None)` + `manager.command(id, Start)` + subscribe events
4. `agent.steer(msg)` → `manager.command(id, RunCommand::Steer { message: msg })`
5. `agent.cancel_token.cancel()` → `manager.command(id, RunCommand::Cancel)`
6. `agent.state()` → `manager.run_state(id)` (返回 RunState 而非 AgentState)
7. `agent.current_model()` → `manager.brain().current_model_name()`
8. `agent.config` → `manager.brain().config`
9. `agent.context_messages()` → 需要从 Run 获取（Run handle 需要暴露 context_messages）
10. `agent.clear_context()` → 需要新建一个 Run（旧 Run cancel + 新 Run create）

### 方案 B：保留 Agent 作为 CLI 内部实现

RunManager 作为外部 API，但 CLI 内部仍用 Agent。
这不算真正迁移，不推荐。

### 需要新增的 API

CLI 迁移可能需要给 runtime 模块加几个方法：

1. **Brain 工具注册** — 当前 `build_tool_registry` 只注册默认工具。
   CLI 需要注册 todo/skill/task/subagent 工具。
   建议给 Brain 加 `register_extra_tools` 回调或 `build_tool_registry_with_extra` 方法。

2. **Run context 访问** — CLI 的 /context、/tokens、/rewind 命令需要访问 Run 的 context。
   建议给 RunHandle 加 `context_messages()` 方法（需要 Run 把 context 共享出来）。

3. **Run 模型操作** — CLI 的 /temp、/max-tokens 命令需要改 Run 的 client 参数。
   建议给 RunHandle 加 `set_temperature` / `set_max_tokens` 方法。

4. **Run clear_context** — CLI 的 /clear、/new 命令需要清空 context。
   建议直接 cancel 当前 Run + 创建新 Run。

## 删除清单（迁移完成后）

```
core/src/agent/mod.rs          — Agent + AgentBuilder（整个文件）
core/src/types.rs              — AgentState 枚举
core/src/permission/mod.rs     — global_pending_approvals() + PendingApprovalMap
core/src/agent/                — executor.rs + scheduler.rs 可以保留（Run 用）
                                 但 ToolOrchestrator 的 approval_resolver: None
                                 回退路径可以删除
core/src/lib.rs                — 移除 Agent/AgentBuilder/AgentState 导出
```

## 测试要求

- 迁移后 `cargo check` 全 workspace 通过
- `cargo test -p agent_core --lib` 全绿（当前 264 个测试）
- `cargo test -p agent_core --test integration` 全绿（当前 11 个测试）
- CLI 能正常启动、对话、执行工具
