# 2026-06-22 Agent Runtime 管理系统重构方案

## 一、目标

将当前"单 Agent 实例 + 大锁串行 + 全局审批 + 无状态机 + 无持久化"的架构，
重构为"Brain 可复用 + Run 独立生命周期 + 状态机驱动 + 进程级零泄漏"的
Actor-EventSourcing 架构。

三条硬约束：

1. **独立空间可随时停止** — 每次 Run 是自包含执行单元，Cancel 从任何状态生效，
   且保证所有子进程、子任务、子 Agent、通道、文件句柄全部回收。
2. **子进程零泄漏** — bash、MCP server、subagent 都纳入 ProcessSupervisor 统一管理，
   用进程组 kill，不存在孤儿进程。
3. **干净退出** — 无论正常完成、中途 stop、还是 App 关闭，不留任何僵尸进程、
   泄漏通道、悬挂 future。

---

## 二、当前泄漏点盘点（全部带代码位置）

### 2.1 进程泄漏

| 位置 | 问题 | 后果 |
|------|------|------|
| `core/src/tools/bash.rs:71-78` | `sh -c "cmd"` 用 `kill_on_drop(true)`，但只 kill 直接子进程。`sh -c "build \| grep"` 中 grep 成孤儿 | stop 后编译器继续跑 |
| `core/src/mcp/transport.rs:28` | MCP server 同样 `kill_on_drop`，无进程组。且没有 graceful shutdown（应先发 JSON-RPC shutdown 再 kill） | MCP server 僵尸进程 |
| `core/src/agent/executor.rs:389-392` | `select!` 取消时 `tool_fut` 被抛弃，但 `Child` 在 future 内部，tokio runtime 可能延迟 drop | Child 句柄滞留 |

**根因：`kill_on_drop` 是假安全。** 它依赖 Child 被 drop 的时机，但：
- `tokio::select!` 取消 future 时，内部资源不一定立即 drop
- 即使 drop，也只 kill 直接子进程，不 kill 进程组

### 2.2 任务泄漏（detached spawn）

| 位置 | 代码 | 问题 |
|------|------|------|
| `core/src/background/mod.rs:55` | `tokio::spawn(async move { ... })` | JoinHandle 丢弃，无法取消，pool drop 后任务继续跑 |
| `core/src/agent/mod.rs:1080` | memory consolidation `tokio::spawn` | 同上，且持有 `Arc<Mutex<MemoryManager>>` |
| `core/src/tools/subagent.rs:247` | 并行 subagent `tokio::spawn` | JoinHandle 存入 vec，但父 Run 取消时这些 handle 不会被 abort |

### 2.3 通道泄漏

| 位置 | 问题 |
|------|------|
| `core/src/permission/mod.rs:34` `global_pending_approvals()` | `OnceLock` 全局 HashMap，abandoned approval 的 oneshot::Sender 永远不被清理 |
| `core/src/agent/executor.rs:370` | 每次工具执行创建 `unbounded_channel`，取消时 sender 可能泄漏 |

### 2.4 状态泄漏

| 位置 | 问题 |
|------|------|
| `Agent` 结构体 20+ 字段 | 大脑配置与运行时状态混在一起，Run 结束后状态残留 |
| `cancel_token` 重置陷阱 | `mod.rs:552` 注释承认：core 不能重建 token，否则 caller 持有的旧 token 失效 |
| `steering_queue` / `follow_up_queue` | Run 结束才 clear，中途 Cancel 不 clear |

---

## 三、目标架构

```
┌─────────────────────────────────────────────────────────┐
│  Frontend (React / TUI)                                  │
│    subscribe(run_id → events)  /  command(run_id, cmd)   │
├─────────────────────────────────────────────────────────┤
│  Bridge (Tauri / CLI)                                    │
├─────────────────────────────────────────────────────────┤
│  RunManager                                               │
│    ├── runs: HashMap<RunId, RunHandle>                   │
│    ├── brain: Brain (单例，所有 Run 共享)                  │
│    └── shutdown: broadcast → 所有 Run                     │
├──────────────────────┬──────────────────────────────────┤
│  Run (Actor)         │  Brain (可复用)                    │
│   ├ state: FSM       │   ├ client: OpenAIClient          │
│   ├ context: Context │   ├ tool_factory                  │
│   ├ event_log        │   ├ recovery: RecoveryEngine      │
│   ├ cmd_rx           │   └ prompt templates              │
│   ├ event_tx         │                                   │
│   ├ supervisor ──────┼──► ProcessSupervisor              │
│   ├ cancel_token     │   ├ children: HashMap<ChildId,    │
│   └ join_set         │   │     SupervisedChild>           │
│                      │   ├ kill_all()                    │
│                      │   └ Drop: kill 进程组             │
└──────────────────────┴───────────────────────────────────┘
```

### 3.1 Brain（可复用，单例）

从现有 `Agent` 抽出**无状态、可共享**的部分：

```rust
pub struct Brain {
    config: Config,
    /// 工厂方法：每个 Run 用它构建自己的 ToolRegistry
    tool_factory: ToolFactory,
    recovery: RecoveryEngine,
    memory: Option<Arc<MemoryManager>>,
    skill_manager: Option<Arc<Mutex<SkillManager>>>,
    mcp_manager: Option<Arc<McpClientManager>>,
}
```

Brain 不持有 context、不持有 cancel_token、不持有运行时状态。
它只提供"构建一个 Run 所需的一切"的方法。

### 3.2 Run（独立空间，每次请求一个）

```rust
pub struct Run {
    pub id: RunId,
    pub session_id: Option<String>,
    state: RunState,
    context: ContextEngine,           // 独立，不共享
    event_log: EventLog,              // append-only 持久化
    cmd_rx: mpsc::Receiver<RunCommand>,
    event_tx: broadcast::Sender<RunEvent>,
    cancel: CancellationToken,
    supervisor: ProcessSupervisor,     // 独立，管理本 Run 的所有子进程
    join_set: JoinSet<()>,            // 独立，管理本 Run 的所有子任务
    brain: Arc<Brain>,                // 共享引用
    permission: PermissionPolicy,     // 独立拷贝
    hooks: HookRegistry,              // 独立
}
```

**关键：Run 拥有自己的 `ProcessSupervisor` 和 `JoinSet`。**
Run 被 drop → supervisor kill 所有子进程 → join_set abort 所有子任务 → cancel 触发。
这是 RAII 保证的，不依赖任何手动 cleanup 调用。

### 3.3 RunManager

```rust
pub struct RunManager {
    brain: Arc<Brain>,
    runs: HashMap<RunId, RunHandle>,
    shutdown_tx: broadcast::Sender<()>,
}

pub struct RunHandle {
    pub id: RunId,
    pub state: Arc<RwLock<RunState>>,
    event_tx: broadcast::Sender<RunEvent>,
    cmd_tx: mpsc::Sender<RunCommand>,
    /// Drop 时自动 cancel + join
    join_handle: Option<JoinHandle<()>>,
}
```

RunManager 的职责：
- `create_run(session_id, user_message) -> RunId` — 构造独立 Run
- `command(run_id, cmd)` — 路由命令到指定 Run
- `subscribe(run_id) -> Receiver<RunEvent>` — 订阅事件流
- `shutdown_all()` — App 关闭时杀掉所有 Run

---

## 四、状态机

```
Created ──Start──► Running ────final answer──► Completed ✓
                       │                        
            ┌──────────┼──────────┐              
            │          │          │              
         need appr  user pause  cancel         
            │          │          │              
            ▼          ▼          ▼              
     AwaitingAppr  Paused    Cancelled ✓         
            │          │                          
       approve/     resume                       
       deny         │                            
            │          │                          
            └────► Running ◄──────────────────────
                       
         unrecoverable error ──► Failed ✓
         max iterations ──────► Failed ✓
```

### 状态定义

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Created,           // 已构造，未启动
    Running,           // turn loop 执行中
    AwaitingApproval,  // 阻塞等权限审批
    AwaitingInput,     // 阻塞等用户输入（agent 反问）
    Paused,            // 用户主动暂停
    Completed,         // 正常完成（终态）
    Cancelled,         // 用户取消（终态）
    Failed,            // 不可恢复错误（终态）
}
```

### 转换守卫

```rust
impl RunState {
    fn transition(self, cmd: RunCommand) -> Result<RunState> {
        use RunCommand::*;
        match (self, cmd) {
            (Created,        Start)     => Ok(Running),
            (Running,        Pause)     => Ok(Paused),
            (Paused,         Resume)    => Ok(Running),
            (Running,        Cancel)    => Ok(Cancelled),
            (Paused,         Cancel)    => Ok(Cancelled),
            (AwaitingApproval, Cancel)  => Ok(Cancelled),
            (AwaitingInput,    Cancel)  => Ok(Cancelled),
            (AwaitingApproval, Approve) => Ok(Running),
            (AwaitingApproval, Deny)    => Ok(Running),  // 否决后继续 loop
            (AwaitingInput,    Answer)  => Ok(Running),
            (Running,        Steer)     => Ok(Running),  // 不改状态
            (s, Cancel) if !s.is_terminal() => Ok(Cancelled),
            _ => Err(format!("invalid: {s:?} + {cmd:?}")),
        }
    }
    fn is_terminal(self) -> bool {
        matches!(self, Completed | Cancelled | Failed)
    }
}
```

### 关键：Cancel 从任何非终态生效

Cancel 不走状态机转换函数——它直接触发 `CancellationToken`，
状态机在 turn boundary 检测到 cancelled 后转为 `Cancelled`。
这保证 Cancel **立即生效**，不需要等当前操作完成。

---

## 五、ProcessSupervisor（零泄漏核心）

这是整个方案最关键的部分。当前 `kill_on_drop` 不可靠，
必须用进程组 + 显式回收。

### 5.1 设计

```rust
pub struct ProcessSupervisor {
    children: HashMap<ChildId, SupervisedChild>,
    cancel: CancellationToken,
}

struct SupervisedChild {
    child: tokio::process::Child,
    pgid: Option<i32>,       // 进程组 ID（Unix）
    label: String,           // "bash: cargo build" / "mcp: filesystem"
    spawned_at: Instant,
}

impl ProcessSupervisor {
    /// spawn 一个 bash 命令，纳入进程组管理
    pub fn spawn_bash(&mut self, command: &str, cwd: &str) -> Result<ChildId> {
        // 关键：用 process_group(0) 让子进程成为新进程组的 leader
        // 这样 killpg(pgid, SIGKILL) 能杀掉整个进程树
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command)
           .current_dir(cwd)
           .stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped())
           .kill_on_drop(false);  // 我们自己管，不依赖 drop

        // Unix: 设置新进程组
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);  // tokio 1.33+ / std 1.64+
        }

        let mut child = cmd.spawn()?;
        let pid = child.id();

        #[cfg(unix)]
        let pgid = pid;  // process_group(0) → 子进程自己成为 pgid=pid

        let id = ChildId::new();
        self.children.insert(id, SupervisedChild {
            child,
            pgid: Some(pid),
            label: format!("bash: {}", command),
            spawned_at: Instant::now(),
        });
        Ok(id)
    }

    /// 杀掉指定子进程及其整个进程组
    pub fn kill(&mut self, id: ChildId) -> Result<()> {
        if let Some(mut sc) = self.children.remove(&id) {
            #[cfg(unix)]
            if let Some(pgid) = sc.pgid {
                // kill 整个进程组，包括 sh 的子进程
                unsafe {
                    libc::killpg(pgid, libc::SIGTERM);
                }
                // 给 2 秒 graceful，然后 SIGKILL
                // （实际实现用 tokio::time::timeout 等待 child 退出）
            }
            let _ = sc.child.kill().await;
            let _ = sc.child.wait().await;  // 回收 zombie
        }
        Ok(())
    }

    /// 杀掉所有子进程（Run cancel / drop 时调用）
    pub fn kill_all(&mut self) {
        let ids: Vec<_> = self.children.keys().cloned().collect();
        for id in ids {
            let _ = self.kill(id);
        }
    }
}

impl Drop for ProcessSupervisor {
    fn drop(&mut self) {
        // RAII 兜底：即使忘了手动 kill，drop 时也杀干净
        self.kill_all();
    }
}
```

### 5.2 bash 工具改造

```rust
// before: child 直接在函数内 spawn，靠 kill_on_drop
// after:  通过 supervisor.spawn_bash()，cancel 时 supervisor.kill()

async fn execute(&self, args: Value, supervisor: &ProcessSupervisor) -> Result<String> {
    let child_id = supervisor.spawn_bash(&command, &working_dir)?;
    // 拿到 stdout handle 后 stream...
    // cancel 时：supervisor.kill(child_id) 由 Run 层统一调用
}
```

Tool trait 签名需要加 `supervisor` 参数（或通过 context 注入）。

### 5.3 MCP server 管理

```rust
impl StdioTransport {
    // 改造：spawn 时纳入 supervisor，shutdown 时先发 JSON-RPC shutdown 再 kill
    pub async fn spawn(command: &str, args: &[String], supervisor: &mut ProcessSupervisor) -> Result<Self> {
        let child_id = supervisor.spawn_mcp(command, args)?;
        // ...
    }

    pub async fn shutdown(&self, supervisor: &ProcessSupervisor) -> Result<()> {
        // 1. 先发 JSON-RPC "shutdown" notification（graceful）
        self.notify("shutdown", json!({})).await.ok();
        // 2. 等 2 秒
        tokio::time::sleep(Duration::from_secs(2)).await;
        // 3. kill 进程组
        supervisor.kill(self.child_id)?;
    }
}
```

---

## 六、子任务管理（JoinSet 替代 detached spawn）

### 6.1 Run 拥有自己的 JoinSet

```rust
pub struct Run {
    // ...
    join_set: JoinSet<()>,  // 替代所有裸 tokio::spawn
}
```

所有需要并发的地方（并行工具执行、subagent、memory consolidation）
都通过 `run.join_set.spawn()` 而非 `tokio::spawn()`。

### 6.2 Cancel 时的清理

```rust
impl Run {
    async fn cancel_and_cleanup(&mut self) {
        // 1. 触发 cancel token（传播到 model stream + tool exec）
        self.cancel.cancel();

        // 2. abort 所有子任务（subagent、background、consolidation）
        self.join_set.abort_all();
        while (self.join_set.join_next().await).is_some() {}

        // 3. kill 所有子进程
        self.supervisor.kill_all();

        // 4. 关闭命令通道（拒绝新命令）
        // cmd_rx 会在 Run drop 时自动关闭

        // 5. 标记终态
        self.state = RunState::Cancelled;
        self.emit(RunEvent::StateChanged { to: RunState::Cancelled });
    }
}
```

### 6.3 替代清单

| 当前裸 spawn | 改为 |
|---|---|
| `background/mod.rs:55` `tokio::spawn` | `run.join_set.spawn` |
| `agent/mod.rs:1080` memory consolidation | `run.join_set.spawn` |
| `tools/subagent.rs:247` 并行 subagent | `run.join_set.spawn` |
| `executor.rs` 中 FuturesUnordered | `run.join_set` 或保留但绑定 cancel |

---

## 七、事件与命令通道

### 7.1 RunCommand（前端 → Run）

```rust
pub enum RunCommand {
    Start,
    Pause,
    Resume,
    Cancel,
    Steer { message: String },
    Approve { prompt_id: String, choice: ApprovalChoice },
    Answer { prompt_id: String, answer: String },
}
```

通过 `mpsc::Sender<RunCommand>` 发送，Run 在 run_loop 中 select 接收。

### 7.2 RunEvent（Run → 前端）

从现有 `AgentEvent` 演化，增加生命周期事件：

```rust
pub enum RunEvent {
    // ── 生命周期（新增）──
    RunCreated { id: RunId, session_id: Option<String> },
    RunStarted,
    RunPaused,
    RunResumed,
    RunCompleted { final_text: String },
    RunCancelled { reason: String },
    RunFailed { error: String },

    // ── 状态转换（新增）──
    StateChanged { from: RunState, to: RunState },

    // ── Turn（沿用）──
    TurnStarted { index: usize },
    TurnEnded { index: usize },

    // ── 模型（沿用）──
    ModelCallStarted,
    ModelStreaming { delta: MessageDelta },
    ModelCallEnded { text: String, tool_count: usize },

    // ── 工具（沿用）──
    ToolStarted { call_id: String, name: String, args: Value },
    ToolUpdate { call_id: String, partial: String },
    ToolEnded { call_id: String, result: String, is_error: bool },

    // ── 交互点（沿用 + 明确状态）──
    ApprovalRequired { prompt_id: String, tool: String, danger: String, explanation: String },
    ApprovalResolved { prompt_id: String, choice: ApprovalChoice },
    InputRequested { prompt_id: String, question: String },

    // ── 上下文（沿用）──
    ContextCompacted { result: CompressionResult },

    // ── 子 Agent（沿用）──
    SubagentStarted { id: String, role: String, task: String },
    SubagentEnded { id: String, success: bool, iterations: usize },

    // ── 进程（新增）──
    ProcessSpawned { child_id: ChildId, label: String },
    ProcessKilled { child_id: ChildId, reason: String },
}
```

通过 `broadcast::Sender<RunEvent>` 广播。多个订阅者（前端 UI、event log、trace）可同时消费。

### 7.3 审批改为 per-Run

```rust
// 废弃 global_pending_approvals()
// Run 内部：
struct PendingApproval {
    prompt_id: String,
    responder: oneshot::Sender<ApprovalChoice>,
}
// 存在 Run 的字段里，Cancel 时 drop 所有 responder → 工具收到 dropped error
```

---

## 八、EventLog 持久化

```rust
pub struct EventLog {
    run_id: RunId,
    path: PathBuf,     // ~/.agent_core/runs/{run_id}.jsonl
    entries: Vec<RunEvent>,
}

impl EventLog {
    pub fn append(&mut self, event: RunEvent) {
        // 写入 JSONL（append-only）
        // 内存也保留一份用于快速查询
        self.entries.push(event.clone());
        let _ = fs::OpenOptions::new()
            .append(true).create(true).open(&self.path)
            .and_then(|mut f| writeln!(f, "{}", serde_json::to_string(&event).unwrap()));
    }
}
```

- Run 创建时打开/创建 JSONL 文件
- 每个事件 append（best-effort，IO 失败不阻断执行）
- Run 结束后文件保留 → 可 replay、可 fork、可给 reflector 分析
- RunManager 可 `list_runs()` / `replay(run_id)` / `fork(run_id, from_event)`

---

## 九、Run 主循环（伪代码）

```rust
impl Run {
    pub async fn run(mut self) {
        // 等待 Start 命令
        self.wait_for_command(RunCommand::Start).await;
        self.transition(RunState::Running);
        self.emit(RunEvent::RunStarted);

        let result = self.run_loop().await;

        match result {
            Ok(text) => {
                self.transition(RunState::Completed);
                self.emit(RunEvent::RunCompleted { final_text: text });
            }
            Err(RunError::Cancelled) => {
                self.cancel_and_cleanup().await;
                self.emit(RunEvent::RunCancelled { reason: "user".into() });
            }
            Err(RunError::Failed(e)) => {
                self.transition(RunState::Failed);
                self.emit(RunEvent::RunFailed { error: e });
            }
        }
    }

    async fn run_loop(&mut self) -> Result<String, RunError> {
        for turn in 0..max_iterations {
            // ── 检查命令通道（非阻塞）──
            self.poll_commands().await?;

            if self.cancel.is_cancelled() {
                return Err(RunError::Cancelled);
            }

            self.emit(RunEvent::TurnStarted { index: turn });

            // Refresh → Compact → Model → Execute → Observe
            let outcome = self.run_turn(turn).await?;
            match outcome {
                TurnOutcome::Final(text) => return Ok(text),
                TurnOutcome::Continue => {}
            }
        }
        Err(RunError::Failed("max iterations".into()))
    }

    /// 非阻塞检查命令通道，处理 Pause/Steer/Cancel 等
    async fn poll_commands(&mut self) -> Result<(), RunError> {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                RunCommand::Cancel => {
                    self.cancel.cancel();
                    return Err(RunError::Cancelled);
                }
                RunCommand::Pause => {
                    self.transition(RunState::Paused);
                    self.emit(RunEvent::RunPaused);
                    // 阻塞等待 Resume 或 Cancel
                    self.wait_for_resume().await?;
                }
                RunCommand::Steer { message } => {
                    self.context.add(Message::user(message));
                }
                RunCommand::Approve { prompt_id, choice } => {
                    self.resolve_approval(&prompt_id, choice);
                }
                RunCommand::Answer { prompt_id, answer } => {
                    self.resolve_input(&prompt_id, answer);
                }
                _ => {}
            }
        }
        Ok(())
    }
}
```

### 审批阻塞点

当工具需要审批时，**不在 executor 里 await oneshot**，
而是回到 Run 层：状态转 `AwaitingApproval`，emit 事件，
然后在 `poll_commands` 里等 `Approve` 命令：

```rust
// executor 发现需要审批 → 返回特殊结果
enum ToolExecResult {
    Ok(String),
    NeedApproval(ApprovalPrompt),
    Error(String),
}

// Run 层处理
let outcome = match tool_result {
    ToolExecResult::NeedApproval(prompt) => {
        self.transition(RunState::AwaitingApproval);
        self.emit(RunEvent::ApprovalRequired { ... });
        // 阻塞等命令
        let cmd = self.cmd_rx.recv().await;
        match cmd {
            Some(RunCommand::Approve { choice, .. }) => {
                self.transition(RunState::Running);
                // 重新执行工具（已批准）
                continue;
            }
            Some(RunCommand::Cancel) | None => return Err(RunError::Cancelled),
            _ => {}
        }
    }
    // ...
};
```

---

## 十、Drop 语义（最关键的安全网）

```rust
impl Drop for Run {
    fn drop(&mut self) {
        // 1. cancel token（传播到所有 await 点）
        self.cancel.cancel();

        // 2. join_set abort_all（杀子任务）
        self.join_set.abort_all();
        // 注意：drop 内不能 await，所以不 join_next
        // abort_all 会让所有任务在下次 await 时 cancel

        // 3. ProcessSupervisor kill_all（RAII，Drop 内调用）
        //    supervisor.drop() 会 kill 所有子进程
        //    已经在 ProcessSupervisor::drop 中实现

        // 4. emit 终态事件（best-effort）
        //    broadcast sender drop 后接收者会收到 closed

        // 5. 关闭 cmd_rx（drop 自动）
        // 6. flush event_log（best-effort）
        let _ = self.event_log.flush();
    }
}
```

**三层安全网：**
1. **正常路径**：run_loop 完成 → 显式 transition 到终态 → 显式 cleanup
2. **Cancel 路径**：cancel_and_cleanup → 显式 kill_all + abort_all
3. **Drop 兜底**：RAII，即使 1 和 2 都没执行，drop 也保证回收

---

## 十一、分阶段迁移计划

### Phase 1：Brain / Run 拆分（核心，不改前端）

**目标：** Agent 拆成 Brain + Run，保持现有 Tauri/CLI 接口不变。

- [ ] 新建 `core/src/runtime/brain.rs` — 从 Agent 抽出 client/factory/recovery/memory/skills
- [ ] 新建 `core/src/runtime/run.rs` — Run 结构体，拥有 context/cancel/supervisor/join_set
- [ ] 新建 `core/src/runtime/manager.rs` — RunManager（单 brain + runs map）
- [ ] `Agent::run_with_events` 逻辑搬到 `Run::run_loop`
- [ ] Tauri `AppState` 从 `Arc<AsyncMutex<Agent>>` 改为 `Arc<RunManager>`
- [ ] `send_message` 改为 `manager.create_run() + run.start()`
- [ ] 现有事件类型保持兼容（`AgentEvent` → 内部转 `RunEvent`）

**验收：** 现有功能不回归，Tauri/CLI 能正常对话。

### Phase 2：ProcessSupervisor（零泄漏）

**目标：** 所有子进程纳入进程组管理，Cancel 时杀干净。

- [ ] 新建 `core/src/runtime/supervisor.rs` — ProcessSupervisor
- [ ] bash 工具改用 supervisor.spawn_bash（process_group(0)）
- [ ] MCP StdioTransport 改用 supervisor + graceful shutdown
- [ ] Run::cancel_and_cleanup 调用 supervisor.kill_all
- [ ] ProcessSupervisor::drop 实现 kill_all 兜底
- [ ] 添加 `libc` 依赖（Unix process group）

**验收：** 跑 `cargo build` 时 Cancel，`ps aux` 无残留进程。

### Phase 3：状态机 + 命令通道

**目标：** RunState FSM + RunCommand 通道替代全局审批。

- [ ] `RunState` 枚举 + transition 守卫
- [ ] `RunCommand` mpsc 通道
- [ ] 审批从 `global_pending_approvals()` 改为 per-Run 命令
- [ ] `poll_commands` 在 turn boundary 处理 Pause/Steer/Cancel/Approve
- [ ] Tauri 新增 `pause_run` / `resume_run` / `steer_run` 命令
- [ ] 前端增加 Pause/Resume/Steer UI

**验收：** 能暂停/恢复/steer，审批绑定到正确 Run。

### Phase 4：JoinSet 统一子任务

**目标：** 消除所有 detached spawn，Cancel 时全部 abort。

- [ ] background pool 改用 run.join_set
- [ ] memory consolidation 改用 run.join_set
- [ ] 并行 subagent 改用 run.join_set
- [ ] Run::cancel_and_cleanup 调用 join_set.abort_all
- [ ] Run::drop 调用 abort_all 兜底

**验收：** Cancel 后 `tokio::runtime` 无悬挂任务。

### Phase 5：EventLog 持久化

**目标：** 事件 append-only 持久化，可 replay/fork。

- [ ] EventLog 结构 + JSONL 写入
- [ ] Run 创建时打开 log，每个事件 append
- [ ] RunManager: `list_runs` / `replay(run_id)` / `fork(run_id)`
- [ ] 前端 Trace/Replay 面板（可选，后续）

**验收：** 关 App 重开后能 replay 上次 Run。

### Phase 6：清理废弃代码

- [ ] 删除 `global_pending_approvals()`
- [ ] 删除 `Agent` 结构体（Brain + Run 完全替代）
- [ ] 删除 `pending_approvals_override`
- [ ] 清理 `AgentState`（被 `RunState` 替代）
- [ ] 更新 `lib.rs` public API

---

## 十二、依赖变更

```toml
# core/Cargo.toml 新增
[dependencies]
libc = "0.2"   # Unix process group (killpg, process_group)
```

无其他新依赖。broadcast/mpsc/JoinSet/CancellationToken 都已在 tokio "full" 中。

---

## 十三、风险与对策

| 风险 | 对策 |
|------|------|
| Tool trait 签名变更（加 supervisor） | 用 context 注入而非改签名，或提供 `Tool::execute_v2` 默认委托 |
| Run drop 时不能 await | supervisor 用同步 kill（`killpg` + `child.start_kill`），不 await wait |
| broadcast backlog 爆满 | 用 bounded channel 或溢出策略丢弃旧事件 |
| 并发 Run 改同一文件 | Phase 2 暂不支持并发 Run（单活跃 Run），worktree 隔离后续做 |
| Windows 无 process_group | `#[cfg(unix)]` 分支，Windows 用 Job Object（后续） |

---

## 总结

这个方案的核心是三点：

1. **Run 是独立执行空间** — 拥有自己的 context、cancel、supervisor、join_set、event_log，
   互不干扰，可随时 Cancel。

2. **ProcessSupervisor 保证进程零泄漏** — 进程组 kill + RAII drop 兜底，
   不依赖 `kill_on_drop` 的假安全，Cancel/Drop 时杀整个进程树。

3. **三层安全网** — 正常完成 / Cancel 清理 / Drop 兜底，
   任何路径都不留僵尸进程、悬挂任务、泄漏通道。
