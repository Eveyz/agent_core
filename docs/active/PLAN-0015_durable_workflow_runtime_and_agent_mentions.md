# PLAN-0015: Durable Workflow Runtime & `@CustomAgent` Inline Workflows

```yaml
---
id: PLAN-0015
type: PLAN
title: Durable Workflow Runtime & @CustomAgent Inline Workflows
status: Draft
author: agent_core (AI-generated, requires human review)
created: 2026-07-22
updated: 2026-07-22
reviewers: [zniverse]
related: [PLAN-0007, PLAN-0009, RFC-0002]
supersedes: PLAN-0009
superseded_by: ~
tags: [workflow, runtime, custom-agent, multi-agent, durability, orchestration]
---
```

## Objective

建立一个通用、可恢复、可持续演进的 Durable Workflow Runtime，并把输入框中的 `@CustomAgent` 作为它的第一个 Inline Workflow Authoring Adapter。

最终形成统一产品模型：

- `@一个 Agent` 是单 Activity 的 Inline Workflow。
- `@多个 Agent` 是带依赖、并行和汇合的 Inline Workflow。
- 当前 Workflow 画布发布为不可变 Revision 后，由同一个 Runtime 执行。
- 未来 Cron、外部 Trigger、Human Approval、Timer、Child Workflow 和 Agent Experience 都复用同一运行内核。

最重要的兼容目标：没有结构化 `@Agent` mention 时，现有 `RunManager -> Run` 路径在模型上下文、Tool Catalog、权限、事件、数据库和行为上保持不变。

## Background

项目已经具备 Custom Agent Definition、管理 UI、Workflow 画布和静态 DAG Executor，但当前 Workflow 实现更接近功能原型，还不是可恢复的通用运行内核：

- [`WorkflowExecutor`](../../core/src/workflow/executor.rs) 按拓扑 stage 执行，存在不必要的全局屏障。
- 节点 Context 和 skipped state 主要在内存，App 重启后不能从持久化事实继续推进。
- HumanApproval 当前自动通过，还没有 Durable Signal 语义。
- Workflow 运行时读取可变 Agent Definition，运行期间编辑 Agent 可能改变后续节点。
- 当前 `workflow_runs` 与可变 Workflow Definition 强关联，Definition 生命周期可能破坏历史运行记录。
- Tauri `run_workflow` 同步等待执行完成，Executor 内部分配真实 Run ID，取消路径只能先使用 placeholder ID。
- App 启动时把 running Workflow 统一标记为 interrupted，而不是 reconcile 和恢复。
- standalone Custom Agent 与 Workflow Agent Node 重复构建 model、skills、tools、permission、memory 和 history。

现有 Agent Runtime 已经是一个独立 Module，负责单个 Agent Run 的模型循环、Tool 执行、Steering、Approval、Cancellation 和资源清理。新 Workflow Runtime 不应替换它，而应位于其上层，编排粗粒度 Activity。

本计划在接受后取代 `PLAN-0009` 中关于 Stage Executor、内存 WorkflowContext、`WorkflowExecutor -> Subagent` 硬绑定、可变 Definition 直接执行、`trusted => Yolo` 和自动 Agent Memory 的运行时设计。已经完成的 Agent CRUD、Registry、画布和可复用执行能力继续保留。

## Architecture Decisions

### 1. 两个 Runtime，各自拥有清晰职责

```text
Agent Runtime
  └─ 一个 Agent Run 内的模型推理、Tool Loop、Context、Steering 和进程清理

Workflow Runtime
  └─ 多个 Activity 的依赖、调度、Attempt、History、Signal 和恢复

CustomAgentActivityAdapter
  └─ 在 ActivityAdapter Seam 调用统一 CustomAgentRunner
```

目标调用关系：

```text
普通 Prompt
  -> existing RunManager / Run
  -> normal answer

Prompt + structured AgentMention[]
  -> existing RunManager / Run (parent)
  -> run-scoped WorkflowClient
  -> WorkflowRuntime.start(Inline WorkflowSpec)
  -> CustomAgentActivityAdapter
  -> CustomAgentRunner
  -> structured Workflow result
  -> parent Agent synthesizes final answer
```

Module 依赖规则：

- Agent Runtime 只依赖窄的 `WorkflowClient` Interface，不依赖 Workflow Runtime Implementation。
- Workflow Runtime 只依赖 `ActivityAdapter` Interface，不直接依赖或拥有全局 `Brain`。
- Composition root 负责连接 `WorkflowRuntime`、`CustomAgentActivityAdapter` 和 `CustomAgentRunner`。
- Workflow Runtime 不管理主 Prompt、主 Session transcript 或 Agent Tool Loop。
- Agent Runtime 不计算 Workflow Ready Frontier，也不直接更新 Workflow Attempt。

### 2. 三类入口编译成同一种 Program

```text
MentionWorkflowCompiler  -- Inline WorkflowSpec --┐
Legacy/Canvas Compiler   -- Published Revision ---+--> WorkflowProgram
Cron/API/Test Trigger    -- Published Revision ---┘
```

Inline 只表示“不进入用户的 Saved Workflow Catalog”，不表示不持久化。Inline Run 仍保存 Program、RunManifest、History、Attempt、Artifact 和 Handoff，并可在之后执行 “Save as Workflow”。

### 3. Definition、Program、Run 和 History 分离

#### Workflow Definition / Revision

- `WorkflowDefinitionId` 是可复用 Workflow 的逻辑身份。
- Draft 可以编辑；Publish 产生不可变 `WorkflowRevisionId`。
- Canvas 坐标、颜色、尺寸属于 Layout Revision，不进入执行 Hash。
- 删除 Definition 不得级联删除 Revision、Run、History 或 Artifact。

#### Workflow Program

Compiler 产生的规范化执行 IR。它包含节点、数据绑定、控制依赖、结果表达式和执行策略。

#### RunManifest

每次 Run 开始前解析并冻结 Program 的全部依赖闭包：

- Agent Definition Revision 或内容快照
- Agent instructions 和 model config
- Skill、Tool、Activity Adapter 的版本或内容 Hash
- Permission Ceiling、Retry、Timeout、Effect Policy 和 Resource Claims
- Schema Version 和 Compiler Version

恢复只读取 RunManifest，不重新解析 `latest` Agent、Model、Skill 或 Tool。

#### Workflow Run

Program 的一次调用，包含稳定 `run_id`、幂等 `request_id`、输入、RunScope、Parent Correlation、当前状态和最终输出。

建议状态：`Pending`、`Running`、`Waiting`、`Succeeded`、`Failed`、`Cancelled`、`NeedsAttention`。

#### Node Instance / Attempt

- `node_instance_id` 表示逻辑节点的一次物化执行。
- `attempt_id` 表示该 Node Instance 的一次尝试。
- Retry 追加新 Attempt，不覆盖历史 Attempt。
- Attempt 保存输入、结果、Artifact、Checkpoint、Effect Key、Lease/Fencing Token 和终止原因。

#### Workflow History

按 `(run_id, sequence)` 追加的领域事实，是恢复的权威来源。Run View、节点状态、成本统计和 Ready Queue 都是可重建 Projection。

核心 History Event：

```text
RunCreated / RunStarted
ActivityScheduled
AttemptStarted
ActivityCompleted / ActivityFailed / AttemptTimedOut / AttemptOutcomeUnknown
RetryScheduled
SignalAwaited / SignalReceived / SignalConsumed
TimerScheduled / TimerFired
BranchSelected
ChildStarted / ChildCompleted
RunCompleted / RunFailed / RunCancelled
```

Token delta、完整模型 stream 和长 transcript 不进入核心 History；它们保存在 Trace 或 Artifact Store，History 只保存引用和最终 Handoff。

### 4. Workflow Runtime 是深 Module

外部 Interface 保持很小：

```rust
#[async_trait]
pub trait WorkflowRuntime {
    async fn start(&self, request: StartRun) -> Result<StartReceipt>;

    async fn command(
        &self,
        run_id: RunId,
        command: WorkflowCommand,
    ) -> Result<CommandReceipt>;

    async fn observe(&self, query: ObserveRun) -> Result<RunObservation>;
}

pub enum WorkflowSource {
    Inline(WorkflowSpec),
    Published(WorkflowRevisionId),
}

pub enum WorkflowCommand {
    Signal { command_id: String, name: String, payload: Value },
    Pause { command_id: String },
    Resume { command_id: String },
    Cancel { command_id: String, reason: String },
    ResolveUnknownAttempt { command_id: String, resolution: Resolution },
}
```

Interface 约束：

- `start` 必须先持久化再返回；相同 `request_id` 返回同一个 Run。
- `command` 使用 `command_id` 去重。
- `observe` 返回 Snapshot 和 `after_sequence` 之后的 History，可用于等待和断线补洞。
- `execute_to_completion`、Tauri subscription 和 CLI wait 只能是调用方 Adapter。
- Workflow Signal 与主 Agent Steering 是不同语义，不复用同一队列或事件类型。

内部 Activity Seam：

```rust
#[async_trait]
pub trait ActivityAdapter {
    fn descriptor(&self) -> ActivityDescriptor;
    async fn invoke(&self, invocation: ActivityInvocation) -> ActivityOutcome;
    async fn recover(&self, attempt: InterruptedAttempt) -> RecoveryDisposition;
}
```

`CustomAgentActivityAdapter` 是第一个 Adapter。后续 Tool、Remote Worker 等 Adapter 不改变 Workflow Runtime 的外部 Interface。

### 5. Workflow IR

Runtime IR 使用“固定的核心控制语义 + 版本化 Activity Kind”，不把 coder、reviewer、researcher 等业务角色写入 Runtime：

```rust
pub struct WorkflowSpec {
    pub schema_version: u32,
    pub nodes: Vec<NodeSpec>,
    pub result: ValueExpr,
    pub policy: WorkflowPolicy,
}

pub struct NodeSpec {
    pub key: NodeKey,
    pub kind: NodeKind,
    pub inputs: BTreeMap<PortName, ValueExpr>,
    pub after: Vec<NodeKey>,
    pub retry: RetryPolicy,
    pub timeout: TimeoutPolicy,
    pub effect: EffectPolicy,
    pub resources: Vec<ResourceClaim>,
}

pub enum NodeKind {
    Activity(ActivityKindRef), // "custom_agent@1", "tool@1", ...
    Choice,
    WaitSignal,
    Timer,
    ChildWorkflow,
    ForEach,
    Output,
}
```

V1 只开放 `Activity("custom_agent@1")`、静态依赖、并行、汇合和 Output。V1 数据交换使用带 JSON Schema 的 Value、`ArtifactRef` 和稳定的 `agent.handoff@1`，不立即实现完整 nominal type system。

Runtime 不从 Agent 名字、角色或自然语言推断依赖。Compiler 必须产生显式 `after` 和输入绑定。转换使用 `ValueExpr` 或显式 Transform Activity，不在 Edge 中保存无法验证的自由格式逻辑。

### 6. Mention Authoring Adapter

Composer 必须产生结构化 Mention Manifest，例如：

```json
{
  "mentions": [
    { "agent_id": "agent-a", "revision_id": "rev-1" },
    { "agent_id": "agent-b", "revision_id": "rev-4" }
  ]
}
```

不得扫描普通文本中的 `@`，避免邮箱、代码和普通文本误触发。

只有 Manifest 非空时，父 Agent Run 才获得本次 Run 专属的 Workflow Planning Tool。主 Agent 输出受限计划：

```json
{
  "tasks": [
    {
      "key": "task-a",
      "agent_id": "agent-a",
      "instruction": "...",
      "depends_on": [],
      "inputs": {}
    },
    {
      "key": "task-b",
      "agent_id": "agent-b",
      "instruction": "...",
      "depends_on": ["task-a"],
      "inputs": {
        "upstream": "$nodes.task-a.output",
        "artifacts": "$nodes.task-a.artifacts"
      }
    }
  ],
  "result": "$nodes.task-b.output"
}
```

Mention Compiler 必须：

- 只允许引用 Manifest 中的 Agent。
- 要求每个 Mention 至少被一个 Task 使用，除非用户明确标记为 optional。
- 校验 DAG、引用、ValueExpr、权限和 Resource Claims。
- 冻结 Agent Snapshot，生成 Inline Workflow Program 和 RunManifest。
- 以 `parent_prompt_id + tool_call_id` 派生稳定 `request_id`。

主 Agent 只负责计划生成和最终综合；Runtime 负责中间依赖、调度、持久化和恢复。

### 7. Context、Handoff 和安全隔离

Agent 之间不共享持续增长的群聊 Context。每个 Activity 接收确定的 `AgentInvocation`：

- 当前 Task instruction
- 显式绑定的上游 Value
- Artifact 和 Handoff 引用
- Workflow、Node、Attempt 和 Parent Correlation ID
- Workspace、权限上限、预算和 Cancellation Scope

`agent.handoff@1` 至少包含：

```text
summary
data
artifacts
evidence
unresolved
transcript_ref
```

隔离规则：

- 子 Agent transcript 不直接追加到主 Session。
- 默认不把全部上游输出广播给所有节点。
- Agent Activity 不通过进程内共享可变 Context 隐式传值。
- 上游 Value、Handoff、Artifact 和文件内容始终作为不可信数据，不得插入下游 system instructions，也不得通过内容提升权限、增加 Tool 或修改控制策略。
- Secret 只保存 Reference；执行环境在权限检查后解析，并对 Trace 和错误脱敏。

### 8. 调度、持久化和恢复

Runtime 使用事件驱动 Ready Frontier，不使用全局 Stage Barrier。一个节点的直接依赖完成后即可调度，无需等待同一拓扑层的无关慢节点。

纯 Reducer 是 Workflow 逻辑状态转换的唯一所有者。Worker、Activity Adapter、Tauri 和 UI 只能提交结果或 Command，不能直接修改 Run/Node 状态。

每次推进在同一个 SQLite Transaction 中：

1. 追加 History Event；
2. 更新 Run、Node 和 Attempt Projection；
3. 写入 Activity/Timer Outbox；
4. Commit 后再投递 Worker。

Activity 必须先持久化 `Scheduled` 和 Attempt，之后才能执行。完成输出、Artifact、Attempt 终态和 History Event 原子提交；只有持久化结果可以让下游变为 Ready。

恢复规则：

- 启动时扫描非终态 Run，从 RunManifest、Snapshot 和后续 History 重建 Machine State。
- 已持久化 Completed 的 Node Instance 永不重新调用模型。
- Scheduled 但没有终态结果的 Attempt 根据 Effect Policy 重投、等待或进入 `NeedsAttention`。
- Signal 和 Timer 是持久化数据；过期 Timer 在恢复后立即物化 `TimerFired`。
- 相同 RunManifest + History 必须产生相同调度 Decision；Reducer 禁止调用模型、读取当前时间、产生随机数或查询可变 Definition。
- UI event broadcast 不是事实来源；客户端通过 History sequence 补洞。

概念持久化记录：

```text
workflow_definitions / workflow_definition_revisions
workflow_programs / workflow_run_manifests
workflow_runs / workflow_events
workflow_node_instances / workflow_attempts / workflow_activity_tasks
workflow_signals / workflow_timers
workflow_artifacts / workflow_resource_leases
```

确切表名和迁移方式由 Schema ADR 决定。迁移必须 additive；确认新 reader 和恢复逻辑稳定前，不删除旧表或旧 Workflow reader。

### 9. 权限、资源和副作用

Activity 有效权限：

```text
Caller / Trigger Permission Ceiling
  INTERSECT Workflow requested permission
  INTERSECT Agent capability declaration
  INTERSECT Activity requested permission
```

Workflow 不得因为 `trusted` 自动提升到 Yolo。Cron 等无人值守 Trigger 必须绑定明确 Execution Profile。

DAG dependency 和 Resource Claim 分开：

- Edge 表示数据或控制依赖。
- Resource Claim 表示 Workspace、路径、端口、设备或远端账户的冲突。
- Lease 使用 fencing token，防止过期 Worker 提交结果。
- V1 对 WorkspaceWrite 使用保守独占 Lease；未来可用独立 worktree 和显式 Merge Activity。

副作用语义：

- Runtime 只承诺 at-least-once dispatch，不声称任意副作用 exactly-once。
- Pure/ReadOnly Activity 可以自动 crash retry。
- WorkspaceWrite V1 默认不盲目 crash retry。
- 外部系统支持 idempotency key 时，所有 Attempt 复用同一个逻辑 `effect_key`。
- 远端可能成功、本地未记录时写入 `AttemptOutcomeUnknown`，Run 进入 `NeedsAttention` 等待核对。
- 在 effectful Tool 全部接入 Effect Journal 前，包含写操作的 Agent Activity 使用保守恢复策略。

### 10. Parent Agent 恢复边界

Workflow 可恢复不代表当前 Agent Run 能在任意模型或 Tool 调用中点透明恢复。

V1 保存：

```text
parent_session_id
parent_prompt_id
parent_tool_call_id
invocation_id
continuation_key
```

如果父 Run 仍存活，Workflow 完成后解除等待并继续综合。如果父 Run 已丢失，Workflow 结果进入 `completed_unattached`，由 UI 或下一次主 Agent Run 使用同一个 `continuation_key` 创建一次综合 Turn。

Workflow 启动、结果附着和最终 Assistant Message 写入都必须有幂等 ID，避免重启后重复启动、重复附着或重复回复。

V1 不承诺父 Agent 在进程崩溃后的无感续写，但必须保证 Workflow 工作和结果不丢失、不重复执行已完成节点。长期方案需要单独 ADR，在以下方向中选择：

1. 为 Agent Runtime 增加 Durable Tool-boundary suspension/checkpoint；或
2. 仅对带 Mention 的请求，把 Planner、Agent Activities 和 Final Synthesis 全部放入顶层 Workflow。

### 11. Agent Experience 的未来接入点

Workflow Runtime 不直接学习或修改 Agent Memory，只记录可审计 Provenance：Agent Snapshot Hash、Task、绑定输入、Handoff、Artifact、Transcript Reference、Attempt、错误、人工反馈和最终 Workflow Outcome。

未来独立 `ExperienceProjector` 消费已完成且经过评价的 Activity，生成候选经验；经过去重、质量评估和用户策略后再写入 Agent Memory。Experience 逻辑不得进入 Workflow Reducer。

## Scope

### In Scope

- 通用 `WorkflowRuntime.start / command / observe` Interface
- Inline 和 Published Workflow Source
- 不可变 Program、RunManifest、Run、Node Instance、Attempt、History 和 Artifact
- 静态 DAG、事件驱动 Ready Frontier、并行、汇合和显式 Value Binding
- `CustomAgentRunner` 和 `CustomAgentActivityAdapter`
- 结构化 Agent Mention、受限 Plan Compiler 和父子 Correlation
- 幂等启动、Cancel、App 重启 reconcile 和 event replay
- 保守的 WorkspaceWrite、Permission Intersection 和 `NeedsAttention`
- Legacy Workflow Compiler，保留当前画布 UI
- Inline Run “Save as Workflow” 的数据路径
- Feature Flag、additive migration 和回滚策略

### Out of Scope

- 任意循环、任意动态图修改或 Agent 自主改写已执行 Graph
- Agent 之间自由群聊或共享可变 Context
- Agent 模型/Tool 调用中点的透明恢复
- 任意外部副作用自动重试或 exactly-once 承诺
- V1 自动写入 Agent 长期记忆
- V1 在 Desktop 完全关闭期间继续主动执行
- V1 完整 nominal type system
- V1 自动 worktree merge 和复杂冲突解决

## Tasks

| ID | Task | Owner | Status | ETA |
|----|------|-------|--------|-----|
| T1 | 起草并审批 Runtime Seam、Run/History、Side-effect Semantics 三份 ADR | Architecture | Todo | TBD |
| T2 | 建立无 Mention parity/golden tests 和 Feature Flag | Core/App | Todo | TBD |
| T3 | 抽取 `CustomAgentRunner`，让 standalone 与旧 Workflow 共用执行路径 | Core | Todo | TBD |
| T4 | 定义 Workflow IR、RunManifest、ID 体系和 additive schema migration | Core | Todo | TBD |
| T5 | 实现 InMemory/SQLite WorkflowStore、ArtifactStore 和 deterministic reducer tests | Core | Todo | TBD |
| T6 | 实现 `start / command / observe`、History/Projection、Outbox 和 Ready Frontier | Core | Todo | TBD |
| T7 | 实现 Attempt、idempotency、Cancel、reconcile、Lease 和保守 Retry | Core | Todo | TBD |
| T8 | 实现 Composer 结构化 Mention Token 和 Mention Manifest | Frontend/App | Todo | TBD |
| T9 | 实现受限 Mention Planner/Compiler、Inline Workflow 和结构化 Handoff | Core/App | Todo | TBD |
| T10 | 实现 Parent Correlation、`completed_unattached` 和幂等 continuation | Core/App | Todo | TBD |
| T11 | 实现 `LegacyWorkflowCompiler`，将当前画布接到新 Runtime | Core/App | Todo | TBD |
| T12 | 将 Tauri Workflow 调用改为稳定 Run ID + async observe，移除 placeholder cancel ID | App | Todo | TBD |
| T13 | 增加 observability、crash failpoints、migration/rollback tests 和渐进发布 | Core/App | Todo | TBD |
| T14 | 后续增加 Signal、真实 Approval、Timer、Child Workflow 和 `OutcomeUnknown` UI | Core/App/Frontend | Todo | TBD |

## Milestones

| Milestone | Description | Target Date |
|-----------|-------------|-------------|
| M0 — Contract Freeze | ADR、术语、IR、History Event Catalog、兼容不变量和 V1 非目标通过评审 | TBD |
| M1 — No-behavior Refactor | `CustomAgentRunner` 抽取完成，standalone/legacy parity tests 通过 | TBD |
| M2 — Durable Kernel | 静态 DAG、History、Attempt、Outbox、幂等和重启 reconcile 通过 fake activity crash tests | TBD |
| M3 — Mention V1 | 单/多 Agent Mention、并行/依赖、Handoff、Cancel 和主 Agent 综合可用 | TBD |
| M4 — End-to-end Recovery | 父 Run 丢失后的 continuation 单次恢复、event replay 和保守副作用处理可用 | TBD |
| M5 — Saved Workflow Migration | 当前画布通过 Legacy Compiler 使用新 Runtime，旧 UI 无需先重写 | TBD |
| M6 — Durable Control Flow | Signal、Approval、Timer、Child Workflow、Lease/Fencing 和 `NeedsAttention` UI 可用 | TBD |

## Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| 新 Tool 或指令污染所有普通 Prompt | High | High | Mention Manifest 为空时不注入任何 Workflow 能力；golden test assembled context/tool schema |
| 构建成半套 Temporal，V1 复杂度失控 | High | High | V1 限定静态 DAG + Custom Agent Activity；高级控制节点分阶段开放 |
| Crash retry 重复产生写入或外部副作用 | High | High | Effect Policy、稳定 effect key、保守 retry、`OutcomeUnknown/NeedsAttention` |
| Workflow 恢复但父 Agent Turn 丢失 | High | High | Parent Correlation、continuation key、`completed_unattached` 和幂等最终消息 |
| 上游 Agent 输出形成 Prompt Injection | High | Med | Handoff 始终是不可信 data，不插入 system instructions，不允许内容提权 |
| 多 Agent 同时修改 Workspace | High | High | Resource Claim、exclusive lease/fencing；后续 worktree + Merge Activity |
| 运行中编辑 Agent/Skill 改变结果 | High | Med | RunManifest 固定全部依赖版本和内容 Hash |
| Definition 删除破坏审计记录 | High | Med | Revision/Run/History 独立生命周期，禁止 cascade delete 历史 |
| UI event 丢失导致状态错误 | Med | High | SQLite History 为权威来源，broadcast 仅通知，sequence replay 补洞 |
| 新旧 Workflow migration 无法回滚 | High | Med | additive migration、Feature Flag、旧 reader 保留、新 Run 禁止降级执行 |
| History 和 Artifact 无限增长 | Med | High | 将 retention/quota/GC/加密写入后续 Storage ADR |

## Success Criteria

- 没有结构化 Mention 的 Prompt 不创建 Workflow 数据，不增加 system prompt、Tool Schema、异步任务或数据库访问。
- 邮箱、代码和普通文本中的 `@` 不触发 Workflow。
- 单 Mention 创建一个可观察、可取消、可恢复的单 Activity Inline Run。
- 多 Mention 可以表达任意角色的静态 DAG；独立节点并行，依赖节点只在上游结果持久化后启动。
- 下游只收到显式绑定的 Value、Handoff 和 Artifact；完整上游 transcript 不进入下游 Context。
- 运行期间修改 Agent/Workflow/Skill/Tool 不影响已开始 Run。
- 相同 `request_id` 不产生重复 Run；相同 `command_id` 不重复推进状态。
- 节点完成后强制退出 App，恢复时不重新调用该节点。
- ReadOnly Attempt 可以按策略重试；不确定的 WorkspaceWrite/外部副作用进入 `NeedsAttention`。
- UI 断开重连后可以通过 History sequence 恢复完整 Run View。
- 父 Run 在等待时丢失，子 Workflow 仍可完成，并且最终结果只附着和综合一次。
- 父 Run Cancel 按策略传播到 Workflow 和未完成 Activity，已提交 History 保留。
- Workflow 或 Agent 不能超过 Caller/Trigger Permission Ceiling。
- 删除 Saved Workflow 后，历史 Revision、Run、History 和 Artifact 仍可审计。
- 当前画布在不先重写 UI 的情况下通过 Legacy Compiler 使用新 Runtime。
- 关闭 Feature Flag 后，普通 Agent Runtime 与本功能接入前行为一致。

## Deferred Decisions

- Parent Agent 使用 Durable Tool-boundary checkpoint，还是 Mention-only 顶层 Workflow
- Artifact retention、quota、encryption 和 garbage collection
- WorkspaceWrite 使用全局 Lease、路径级 Lease还是默认 worktree 隔离
- 后台 daemon 和跨进程 Worker Lease 模型
- Node/Port Schema 的版本兼容与迁移策略
- Secret Reference 的解析位置和审计脱敏规范
- Experience 候选的评价、采纳、撤销和遗忘策略
- 动态 `GraphPatchProposal`、ForEach、Continue-as-New 的具体 IR

这些决策不得阻塞静态 DAG V1，但 V1 Interface 和持久化模型不能排除它们。

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-07-22 | agent_core | Created as Draft from the `@CustomAgent` / Workflow Runtime architecture discussion |

---
*Generated by AI Agent (agent_core)*

*Model: GPT-5 | Timestamp: 2026-07-22T14:21:26+08:00*
