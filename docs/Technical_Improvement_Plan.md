# agent_core 技术改进方案

> 基于 `Independent_Review_2026-06-18.md` 的诊断,给出可落地、分阶段的技术方案。
> 原则:**接线优先于重构,旁路优先于侵入,护栏优先于自动化。**

---

## 0. 设计原则与分层判断

本方案与 HarnessX 报告的根本分歧在于**执行路径**:报告主张"先把 loop 重写成
Processor Pipeline 再谈其余",本方案主张"先把已造好但未接线的模块焊上、
用旁路机制拿到可观测性,再做架构演进"。理由:前者一次性回归面大、收益滞后;
后者每一步都可独立验证、风险局部化。

### 关键分层判断(决定接线设计,不可重复造轮子)

现有容错能力分布在**两层**,RecoveryEngine 必须只覆盖 client 层够不到的部分:

| 失败类型 | 已由谁处理 | RecoveryEngine 该做什么 |
|---|---|---|
| 单次 HTTP 429 / 5xx / 网络错误 | `client/mod.rs:133` `send_with_retry` + 指数退避 | 不处理(已覆盖) |
| 模型熔断 / 主模型彻底失败 | `client/mod.rs:208` fallback chain + `resilience.rs` CircuitBreaker | 不处理(已覆盖) |
| **context too long(整轮失败后)** | **无人处理** | 触发 `maybe_llm_compact` 后重试该轮 |
| **truncation / max_tokens 不足** | **无人处理** | `set_max_tokens` 提升上限后重试 |
| **连续整轮失败仍可救** | **无人处理** | 循环级 retry(带尝试上限),再放弃 |

所以 RecoveryEngine 的接线价值 = **循环级恢复**,而非 HTTP 级重试。
`determine_strategy` 已能区分这些 case(`error_recovery/mod.rs:92`),只需把
其 `RecoveryAction` 输出接回 loop。

---

## 1. 阶段总览

| 阶段 | 名称 | 侵入度 | 依赖 | 交付物 |
|---|---|---|---|---|
| A | 恢复引擎接线 | 中(改 run_loop 两处错误出口) | 无 | 整轮失败不再直接放弃 |
| B | Hook 边界激活 | 低(补 fire 调用 + 扩枚举) | 无 | 死代码激活 + model 级钩子 |
| C | 上下文处理器列表化 | 低(单槽位→列表) | 无 | 轻量 Processor 接缝 |
| D | 执行轨迹落盘 | 极低(纯旁路) | 无 | `.traces/<id>.jsonl` |
| E | 循环分段与边界钩子 | 中(loop 内联分段) | A,B | Stage 枚举 + 边界钩子统一 |
| F | 离线反思框架 | 低(新模块,挂现有调度) | D | Trace→建议,强制人工批准 |

A–D 互相独立、可并行推进;E 依赖 A/B 的钩子就位;F 依赖 D 的 trace。
建议顺序:**A → B → D → C → E → F**(先拿健壮性与可观测性)。

---

## Phase A — 恢复引擎接线

**目标**:把 `RecoveryEngine` 接进 `run_loop`,覆盖 client 层够不到的循环级恢复。

### A.1 数据结构

`run_loop` 现有两个错误出口直接 `return Ok("...error...")`(`agent/mod.rs:455` 与
`:469`)。改为先交 RecoveryEngine 决策。

新增字段到 `Agent`:

```rust
// agent/mod.rs — Agent struct
recovery: RecoveryEngine,
recovery_ctx: RecoveryContext,
```

`AgentBuilder` 加 `with_recovery(engine: RecoveryEngine)`,默认 `RecoveryEngine::default()`。
`build()` 里初始化 `recovery_ctx = RecoveryContext::new(&model_config.model_id, model_config.max_context_tokens)`。

### A.2 接线点:统一封装一次"带恢复的模型调用"

不要在两处错误出口各写一遍恢复逻辑。把 `stream + collect_stream` 包成一个内部方法,
返回 `Result<(String, Vec<ToolCall>), RecoveryAction>` 或在内部消化重试:

```rust
// agent/mod.rs — 新私有方法
async fn model_turn(
    &mut self,
    on_event: &(impl Fn(AgentEvent) + Send + Sync),
) -> Result<ModelTurnOutcome, String> {
    const MAX_RECOVERY_ATTEMPTS: u32 = 3;
    for _ in 0..MAX_RECOVERY_ATTEMPTS {
        let messages = self.build_messages();      // 抽出 refresh+trim+compact+transform
        let tools = self.registry.tool_definitions();
        let stream = self.client.chat_completion_stream(&messages, &tools).await
            .map_err(|e| { self.recovery_ctx.record_error(&format!("{e}")); format!("{e}") })?;
        match self.collect_stream(stream, on_event).await {
            Ok(r) => { self.recovery_ctx.record_success(); return Ok(ModelTurnOutcome::Done(r)); }
            Err(e) => {
                if self.cancel_token.is_cancelled() {
                    return Err("aborted".into());
                }
                self.recovery_ctx.record_error(&format!("{e}"));
                match self.recovery.determine_strategy(&self.recovery_ctx) {
                    RecoveryAction::CompactContext { target_ratio } => {
                        on_event(AgentEvent::Error("context too long; compacting".into()));
                        self.force_compact(target_ratio).await;
                        continue; // 重试该轮
                    }
                    RecoveryAction::EscalateTokens { new_max_tokens } => {
                        self.client.set_max_tokens(new_max_tokens);
                        on_event(AgentEvent::Error(
                            format!("escalating max_tokens to {new_max_tokens}")));
                        continue;
                    }
                    RecoveryAction::Retry { delay_ms } => {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        continue;
                    }
                    RecoveryAction::SwitchModel { model } => {
                        // 注意:client 层已有 fallback chain,此处只在显式配置时触发
                        if self.config.get_model(&model).is_some() {
                            let _ = self.switch_model(&model);
                            on_event(AgentEvent::Error(format!("switched to fallback {model}")));
                            continue;
                        }
                        return Err(format!("no recovery; last error: {e}"));
                    }
                    RecoveryAction::Fail => {
                        return Err(format!("unrecoverable: {e}"));
                    }
                }
            }
        }
    }
    Err("exhausted recovery attempts".into())
}
```

`run_loop` 里两处错误出口替换为:

```rust
let outcome = match self.model_turn(on_event).await {
    Ok(ModelTurnOutcome::Done(r)) => r,
    Err(e) => {
        on_event(AgentEvent::Error(e.clone()));
        return Ok(format!("I encountered an error: {e}. Please try again."));
    }
};
```

### A.3 辅助方法

- `build_messages(&self) -> Vec<Message>`:抽出 `run_loop:435-447` 的 refresh+trim+compact+transform。
- `force_compact(&mut self, target_ratio: f64).await`:强制对 ~`1-target_ratio` 比例的旧轮做 LLM 摘要(复用 `context.prepare_summary`/`apply_summary`,即现有 `maybe_llm_compact` 的参数化版本)。

### A.4 验证

- 单测:注入一个总是返回 "context length exceeded" 的假 client,断言 `model_turn` 触发一次 compact 后重试成功。
- 不破坏现有 `run_loop` 行为:无错误时 `record_success` 路径与原逻辑等价。
- `cargo test -p agent_core` 全绿。

### A.5 边界(避免重复造轮子)

- **不**在 RecoveryEngine 里做 HTTP 429 重试——那是 `send_with_retry` 的职责。
- `SwitchModel` 仅在 `config.toml` 显式配了 `fallback_model` 之外的命名 fallback 时有意义;若 client fallback chain 已覆盖,该分支保持幂等。
- 设置 `MAX_RECOVERY_ATTEMPTS` 上限,防止无限重试。

---

## Phase B — Hook 边界激活

**目标**:让 `HookRegistry` 真正贯穿 loop,补 model 级钩子。

### B.1 激活死代码

`fire_session_start` / `fire_session_end` 当前无调用点。在 `run_with_events` 里补:

```rust
// agent/mod.rs — run_with_events
on_event(AgentEvent::AgentStart);
self.hook_registry.fire_session_start(self.id());   // 新增
let result = self.run_loop(&on_event).await;
self.hook_registry.fire_session_end(self.id());     // 新增
```

### B.2 扩充 HookEvent

```rust
// hooks/mod.rs
pub enum HookEvent {
    SessionStart { session_id: String },
    SessionEnd { session_id: String },
    TurnStart { turn_index: usize },        // 新
    TurnEnd { turn_index: usize },          // 新
    BeforeModel { messages: Vec<Value> },   // 新(传序列化快照,避免借用)
    AfterModel { text: String, tool_calls: usize }, // 新
    PreToolUse { tool_name: String, input: Value },   // 既有
    PostToolUse { tool_name: String, input: Value, output: String }, // 既有
}
```

`HookAction` 增 `SkipModel(String)`(BeforeModel 可注入"直接用此文本作答")、
`Continue`。新增 `HookRegistry::fire_before_model` / `fire_after_model` / `fire_turn_start` / `fire_turn_end`,
签名与现有 `fire_pre_tool_use` 风格一致。

### B.3 在 loop 边界 fire

- `run_loop` 已有 `TurnStart`/`TurnEnd` 的 AgentEvent——在同位置 fire 对应 hook。
- BeforeModel:在 `model_turn` 内、`chat_completion_stream` 之前 fire。
- AfterModel:`collect_stream` 成功后、写回 context 之前 fire。

### B.4 验证

- 单测:注册一个 BeforeModel hook 返回 `SkipModel("preset")`,断言跳过实际 LLM 调用、直接用 preset。
- 现有 `LoggingHook` 扩展为全事件打印,人工跑一次确认 8 个钩子都被触发。

### B.5 边界

- BeforeModel 传 `Vec<Value>`(序列化)而非 `&[Message]`,避免 hook 持有 agent 借用、也便于未来跨进程 hook。
- `SkipModel` 是强干预,默认 hook 不使用;仅作为可观测/测试注入点。

---

## Phase C — 上下文处理器列表化

**目标**:把 `transform_context: Option<TransformContextFn>` 升级为有序命名列表,
成为轻量级 "BeforeModel Processor",无需新建 trait。

### C.1 数据结构

```rust
// agent/mod.rs
pub struct ContextProcessor {
    pub name: String,
    pub transform: Box<dyn Fn(Vec<Message>) -> Vec<Message> + Send + Sync>,
}

pub type TransformContextFn = Box<dyn Fn(Vec<Message>) -> Vec<Message> + Send + Sync>; // 保留兼容

// Agent 字段
context_processors: Vec<ContextProcessor>,
```

`AgentBuilder::with_transform_context` 保留(内部转为单元素列表),新增:

```rust
pub fn with_context_processor(mut self, name: &str, f: impl Fn(Vec<Message>) -> Vec<Message> + Send + Sync + 'static) -> Self
```

### C.2 接线

`build_messages` 里(Phase A 抽出的方法)顺序应用:

```rust
let mut messages = self.context.messages();
for p in &self.context_processors {
    messages = (p.transform)(messages);
}
```

### C.3 验证

- 单测:注册两个 processor(一个截断、一个加前缀),断言顺序生效。
- 兼容:`with_transform_context` 的旧调用行为不变。

### C.4 边界

- **不**引入新 trait、不改 `run_loop` 控制流——这是"命名列表"而非"Processor 框架"。
- 真正的 Processor 框架留到 Phase E,在 Stage 边界统一抽象。

---

## Phase D — 执行轨迹落盘(纯旁路)

**目标**:把 `AgentEvent` 流落盘为高保真 trace,**不改 run_loop**。

### D.1 复用 AuditLog 范式

`permission/audit.rs` 已证明 JSONL append-only 范式可行。新建 `core/src/trace/`:

```rust
// core/src/trace/mod.rs
pub struct TraceCollector {
    file: std::fs::File,
    task_id: String,
}
impl TraceCollector {
    pub fn new(dir: &str, task_id: &str) -> Result<Self>; // dir 默认 .agent_core_history/traces
    pub fn record(&mut self, event: &AgentEvent);          // 序列化为 JSONL 一行
    pub fn flush(&mut self);
}
```

`AgentEvent` 已 `Serialize`(types.rs:97),直接 `serde_json::to_string`。

### D.2 接线(旁路,零侵入)

调用方在 `run_with_events` 外层包一层:

```rust
let mut trace = TraceCollector::new(".agent_core_history/traces", &task_id)?;
agent.run_with_events(input, |ev| {
    trace.record(&ev);     // 落盘
    ui.handle(ev);          // 原有 UI 回调
}).await?;
trace.flush();
```

也可在 `Agent` 内部默认挂一个 no-op,只有当 builder 开启时才落盘:
`AgentBuilder::with_trace(dir)`。

### D.3 trace 内容增强

`AgentEvent` 不含 context 快照。在关键边界(每轮开始)补一个**轻量快照事件**,
或在 `TraceCollector::record` 内对 `TurnStart` 追加当前 token 计数(通过额外通道)。
推荐:新增一个 `AgentEvent::Checkpoint { token_count: usize, message_count: usize }`,
在 `refresh_context_segments` 后 emit,trace 记录,UI 可忽略。

### D.4 验证

- 跑一次简单任务,检查 `.agent_core_history/traces/<id>.jsonl` 每行可 `serde_json::from_str` 回放。
- 用一个小脚本把 jsonl 还原成可读时间线,确认包含:每轮 tool 调用入参、返回、错误、token 计数。

### D.5 边界

- 纯旁路:trace 失败**不得**影响 agent 运行(`record` 内部吞错 + warn)。
- 大输出工具结果需截断(AuditLog 已有 `truncate(.., 500)` 范式,复用)。

---

## Phase E — 循环分段与边界钩子统一

**目标**:把 `run_loop` 内联序列显式命名 Stage,边界钩子统一收口。
这是 HarnessX "Processor Pipeline" 精神的**渐进式**实现,而非一次性重写。

### E.1 Stage 枚举

```rust
enum Stage { Refresh, Compact, Model, Dispatch, Execute, Observe }
```

把 `run_loop:432-560` 的线性步骤对齐到 Stage,每个 Stage 进出 fire 钩子。
**不**把 Stage 变成独立结构体/外部 trait——保持内联,只引入命名与边界。

### E.2 边界钩子收口

Phase B 的 BeforeModel/AfterModel 等钩子,在 E 阶段统一到 Stage 边界:

```
Refresh   → fire(TurnStart) + refresh_context_segments
Compact   → trim_to_fit + maybe_llm_compact
Model     → fire(BeforeModel) + model_turn + fire(AfterModel)
Dispatch  → tool_calls 非空判断
Execute   → ToolOrchestrator
Observe   → 写回 context + fire(TurnEnd)
```

### E.3 (可选)Stage 跳过/注入

若需要更强的可组合性,允许 hook 返回 `SkipStage` / `InjectMessages`。
**默认不开放**,仅当 Phase F 确有需要时再加,避免过早抽象。

### E.4 验证

- 行为等价性:不开任何 hook 时,`run_loop` 输出与重构前逐字一致(用同一 input 跑回归)。
- 钩子覆盖:注册全事件 LoggingHook,确认每个 Stage 边界都有日志。

### E.5 边界

- **不**把 Stage 抽成 `dyn Processor` trait 列表——那会引入生命周期/借用复杂度且收益有限。
- 保持 `run_loop` 单方法可读;Stage 是"注释 + 钩子挂点",不是"运行时插件系统"。

---

## Phase F — 离线反思框架(带护栏)

**目标**:读 Phase D 的 trace,产出改进**建议**,挂在现有 `background`/`cron`/`tasks` 设施上。

### F.1 架构(复用现有调度)

```
trace jsonl ──► Reflector(新, 一个 Subagent 实例)
                  │  prompt: 分析 trace,找循环/权限拒绝/上下文爆炸
                  ▼
              Suggestion { kind, target, diff, rationale }
                  │
                  ▼
              审批通道(复用 ApprovalRequired 事件)
                  │  人工 Allow / Deny
                  ▼
              落盘:仅"建议类"变更自动应用;安全类必须人工
```

挂载点:`ComprehensiveAgent` 已有 `background_pool` / `cron_scheduler` / `task_board`,
Reflector 作为一个 background task 或 cron job 注册,无需新建调度系统。

### F.2 Suggestion 种类与护栏(关键)

| Suggestion kind | 自动落盘? | 理由 |
|---|---|---|
| 新增非破坏性 SKILL.md(规避某循环) | ✅ 自动(幂等、可回滚) | 低风险,有收益 |
| 调整 memory consolidation 阈值 | ⚠️ 人工 | 影响长期记忆 |
| 改 `permissions.mode` / blacklist | ❌ 禁止自动 | 越权风险 |
| 改 `api_key` / `base_url` / `model_id` | ❌ 禁止自动 | 凭据/路由风险 |
| 改 `max_iterations` / token 上限 | ⚠️ 人工 | 影响行为边界 |

**实现**:维护一个 `SAFE_AUTO_APPLY` 白名单(只含 SKILL 追加类);
其余 Suggestion 走 `AgentEvent::ApprovalRequired` 通道,人工确认后才落盘。
`config.toml` 的 `permissions.*` 与 `[models.*]` 字段进入**不可自动修改清单**,
Reflector 即便建议也只产出 diff 展示,不写文件。

### F.3 Digester 规则(示例,从 trace 检测)

- 连续 ≥3 次 `ToolExecutionEnd{is_error:true}` 同名工具 → 建议 SKILL 提示正确用法。
- 同一 tool_call 模式重复 ≥3 轮 → 标记 looping,建议 SKILL 打断或调整。
- `Checkpoint` token 计数持续逼近上限 → 建议 `max_context_tokens` 调整(人工)。
- `ApprovalRequired` 后 `DenyPersistent` 频繁 → 建议收紧默认权限(人工)。

### F.4 验证

- 构造一个会循环的 trace,跑 Reflector,确认产出对应 SKILL 建议且自动落盘。
- 构造一个建议改 `permissions.mode` 的场景,确认**不**自动落盘、只出审批提示。

### F.5 边界

- Reflector 本身是一个 Subagent,受同一套权限约束——它**没有**特权写 config。
- 所有自动落盘的 SKILL 必须带 `generated_by: reflector` 标记 + 时间戳,便于回溯/批量删除。
- 默认关闭,需 `AgentBuilder::with_reflector(...)` 显式开启。

---

## 2. 风险与护栏汇总

- **A**:恢复重试可能放大 token 消耗 → `MAX_RECOVERY_ATTEMPTS` 上限 + 每次 compact 前检查 token 计数。
- **B**:BeforeModel 钩子传序列化快照有拷贝开销 → 仅在注册了 hook 时才序列化。
- **D**:trace 文件膨胀 → 复用 AuditLog 的 `max_entries` trim + 工具结果截断。
- **E**:重构引入行为差异 → 强制等价性回归(无 hook 时逐字一致)。
- **F**:自修改系统是最高风险 → 安全字段不可自动修改清单 + 强制人工批准 + 默认关闭 + 生成物可回溯标记。

## 3. 验证策略总览

每个阶段都应满足:
1. `cargo build -p agent_core` 通过;
2. `cargo test -p agent_core` 新增针对性单测 + 现有全绿;
3. 行为等价性:无新配置时,与改动前输出一致(A/E 尤其);
4. 旁路隔离性:D/F 的失败不影响主路径。

## 4. 落地顺序与里程碑

- **M1(1-2 天)**:Phase A + B —— 健壮性与钩子激活,立即可感。
- **M2(1 天)**:Phase D —— 可观测性,为后续提供数据基础。
- **M3(1 天)**:Phase C —— 处理器列表化,小而稳。
- **M4(2-3 天)**:Phase E —— loop 分段,统一边界钩子。
- **M5(3-5 天)**:Phase F —— 离线反思框架,默认关闭、护栏优先。

M1 完成后即可独立交付价值(不再因单次模型错误放弃整轮、Session 钩子激活),
无需等待后续阶段。这是本方案"接线优先"的核心收益。
