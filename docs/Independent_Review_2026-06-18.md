# agent_core 独立评审报告 (2026-06-18)

> 评审人:Codex agent
> 对象:`HarnessX_Analysis_2026-06-18.md` 提出的诊断与重构方案
> 方法:逐文件核验 `core/src/agent/mod.rs`、`agent/executor.rs`、`hooks/mod.rs`、
> `permission/mod.rs`、`context.rs`、`error_recovery/mod.rs`、`comprehensive/mod.rs`

---

## 0. 一句话结论

HarnessX 报告的**方向感是对的**(向可组合、可观测、可自我进化演进),
但对现状的描述存在系统性偏差:**它把代码低估成了"一块铁板",
而实际上代码里已经长出了大量模块化的零件——只是其中相当一部分
还没接进主循环。** 真正的瓶颈不是"缺抽象",而是"接线缺失(unwiring)"。

---

## 1. 报告判断准确的部分

| 报告论断 | 核验结果 |
|---|---|
| `run_loop` 是单方法硬编码序列 | ✅ 准确。`refresh_context_segments → trim_to_fit → maybe_llm_compact → stream → execute_tools` 全部内联在 `core/src/agent/mod.rs:424` 的 `run_loop` 里 |
| Hook 只有 4 个事件、且都围绕 Tool/Session | ✅ 准确。`hooks/mod.rs` 仅 `PreToolUse/PostToolUse/SessionStart/SessionEnd`,无 `BeforeModel/AfterModel/StepStart/StepEnd` |
| `refresh_context_segments` 直接读环境/拼 catalog/读 core memory | ✅ 准确。`mod.rs:651` 内联实现 |
| `maybe_llm_compact` 硬插在 loop 里 | ✅ 准确。`mod.rs:440` 直接调用 |
| `AgentEvent` 流是天然的 trace 源 | ✅ 准确且重要。事件覆盖 `AgentStart/End`、`TurnStart/End`、`MessageUpdate/End`、`ToolExecutionEnd/Update`、`Subagent*`、`ApprovalRequired`、`Error`,粒度足够 |

这部分判断没有问题,可以采信。

---

## 2. 报告误判或遗漏的部分(核心)

这部分是我与 HarnessX 报告**分歧最大**的地方,也是我认为最有价值的修正。

### 2.1 严重低估了 Context Engine 的成熟度

报告把上下文管理等同于"`refresh_context_segments` 那段硬编码",
这是误读。`core/src/context.rs` 已经实现了一套**7 段语义上下文引擎**:

```
IDENTITY / PRINCIPLES / ENVIRONMENT / TOOL CATALOG /
ACTIVE MEMORY / LOADED SKILLS / EXECUTION PLAN
```

每段都有独立的 `RefreshPolicy`(never / on-change / per-turn)、
`Stability` 等级、token 预算,还有 `CacheHint`(KV cache 前缀提示)、
`stable_prefix_token_count` 等本地模型优化。

**含义**:这本身已经是一种"组合化上下文"机制。
HarnessX 主张把行为抽成 Processor——而 context 段正是
"可替换、可插拔的上下文处理器"的雏形。报告完全没看到这一层,
导致它建议的"重构"其实有一半已经在 context.rs 里了。

### 2.2 漏掉了 `transform_context` 这个已存在的"处理器"接缝

`AgentBuilder::with_transform_context` (`mod.rs:144`) 允许注入一个
`Fn(Vec<Message>) -> Vec<Message>`,在发送给 LLM **之前**对消息做变换,
且确实接进了 `run_loop:444`。

这就是 HarnessX 想要的 "BeforeModel Processor" 的等价物——只是
当前只有一个全局槽位、没有列表化。**补成列表 + 命名 = 立刻得到
轻量级 Processor pipeline**,无需推倒重来。

### 2.3 最大的盲点:有一批模块**造好了却没接进 run_loop**

这是我在核验中最意外的发现,报告完全没提及:

- **`RecoveryEngine`(error_recovery/mod.rs):完全没接线。**
  它实现了 retry / token escalation / fallback model / compact 四种策略,
  还带单测,但 `rg RecoveryEngine core/src/agent` 零命中。
  也就是说:主循环出错时走的是 `run_loop` 里手写的 `return Ok("...error...")`,
  RecoveryEngine 是一具摆设。**这是一个既有高价值资产被晾着。**

- **`SessionStart/SessionEnd` Hook:是死代码。**
  `fire_session_start` / `fire_session_end` 在 `hooks/mod.rs` 里定义了,
  但全仓库没有任何调用点(`rg fire_session_start` 仅命中定义处)。
  所以报告说"Hook 没贯穿全局"还不够——**实际上连已定义的 Session 钩子都没触发过。**

- **`AuditLog`:只覆盖权限决策。**
  它确实接进了 `PermissionPolicy`(JSONL、append-only),
  但只记 permission 决策,不记 agent 轨迹。
  这是个现成的"trace 落盘"范式,可以直接复用扩展,报告没提。

- **`ComprehensiveAgent`(comprehensive/mod.rs)已有 teams / task_board /
  background_pool / cron_scheduler / worktree** 等编排能力的聚合器,
  报告的"离线反思 Agent"完全可以挂在这套调度设施上,而不是从零写。

### 2.4 对"串行写死"的描述不够精确

报告说"流式生成和工具执行串行写死"。更准确的说法是:
**循环级是串行的(必须:模型先出 tool_calls,再执行),
但工具之间是可并行的**——`ToolExecutionMode::Parallel` 是默认值,
`ToolOrchestrator::execute_tools` 真的会并发跑多个工具。
把这一层说成"串行写死"会让读者误判性能设计。

---

## 3. 我的独立诊断:问题是"接线",不是"铁板"

综合核验,我对现状的定义与 HarnessX 报告不同:

**HarnessX 视角**:agent_core = 强耦合静态脚手架 → 需要大重构为 Processor Pipeline。

**我的视角**:agent_core = **模块零件丰富、但主循环只焊了其中一小部分**。
零件库(7段上下文、RecoveryEngine、AuditLog、transform_context、
teams/tasks/background/cron)已经造好,缺的是**把它们接进 `run_loop`
并在边界上 fire 钩子**。

证据:
- 20 项 harness 能力在 PLAN.md 里全标 ✅ Done,但至少 2 项(Recovery、SessionHook)
  实际未接入主路径;
- `run_loop` 本身只有 ~165 行,却手写了 context 刷新、压缩、流式、工具调度,
  而这些能力本可由现有模块提供。

这意味着:**收益最高、风险最低的改进不是"重构成 Pipeline",
而是"先把已造的零件接线,再逐步把边界暴露为钩子"。**

---

## 4. 独立建议的优先级(与报告不同)

报告建议 Phase 46(重构成 Processor)→ 47(Trace)→ 48(AEGIS 自我进化)。
我重排如下,理由是**先收割已造资产,再做新架构**:

### P0(最高 ROI,低风险):接线 + 补边界钩子

1. **接入 RecoveryEngine 到 run_loop**。
   把 `run_loop` 里两处 `return Ok("...error...")` 改为
   先 `RecoveryEngine::determine_strategy`,按 Retry/Compact/Escalate/Fallback 决策,
   而不是直接放弃。这是"零新代码、纯接线",但能显著提升健壮性。

2. **在 `run_with_events` 里 fire `SessionStart`/`SessionEnd`**,
   补 `StepStart/BeforeModel/AfterModel/StepEnd` 到 `HookEvent` 并在 loop 边界触发。
   让已有的 HookRegistry 真正贯穿。

3. **把 `transform_context` 从单槽位升级为有序列表**,
   每个元素命名 → 这就是轻量 Processor,无需新建 trait。

### P1(高价值,中等工作量):Trace 落盘

4. **复用 AuditLog 的 JSONL 范式,写一个 `TraceCollector`**,
   挂在 `run_with_events` 的 `on_event` 闭包上,把 `AgentEvent` 流
   落到 `.agent_core_history/traces/<task_id>.jsonl`。
   关键字段:context 快照摘要、tool 原始入参、tool 返回、错误、token 计数。
   **这一步不需要改 run_loop,纯旁路。**

### P2(架构演进,谨慎):Loop 分段为 Stages

5. **把 `run_loop` 的内部步骤显式命名为 Stage 枚举**
   (`Refresh → Compact → Model → Dispatch → Execute → Observe`),
   每个 Stage 边界 fire 钩子。这是报告 Phase 46 的精神,
   但我的版本是"渐进式内联分段 + 钩子",而非"重写为外部 Processor 列表",
   避免一次性大重构的回归风险。

### P3(远期,需护栏):离线反思

6. **离线反思 Agent 挂在现有 background/cron/task 设施上**,
   读 P1 的 trace,产出 SKILL.md / config 建议。**强烈建议默认只生成"建议",
   人工确认后再落盘**,不要让 AEGIS 直接改 `config.toml` 的权限与模型配置
   (见第 5 节风险)。

---

## 5. 对报告 Phase 48(AEGIS 自我进化)的风险提醒

报告末尾设想:Meta-Agent 直接修改 `config.toml`(放宽/收紧权限)、
自动写 SKILL.md、改 MCP server。这是**自修改系统(self-modifying system)**,
报告几乎没谈安全边界:

- 自动放宽 `permissions.mode` 或 blacklist,可能让 agent 越权执行破坏性操作;
- 自动改模型/base_url/api_key 配置,有把凭据写错或路由到错误端点的风险;
- 自动生成的 SKILL.md 若含错误指令,会被注入到 system prompt 影响所有后续任务。

**建议**:AEGIS 的输出默认走"建议 + diff + 人工批准"通道,
复用现有 `ApprovalRequired` 机制;只有经过验证的、幂等的变更
(如追加一条非破坏性 SKILL)才允许自动落盘。
config.toml 的安全相关字段(permissions / api_key)应进入**不可自动修改白名单**。

---

## 6. 给 HarnessX 报告的打分与采纳建议

| 维度 | 评价 |
|---|---|
| 方向判断(可组合/可观测/可进化) | ✅ 正确,值得追求 |
| 现状描述("强耦合静态脚手架") | ⚠️ 偏负,低估了 context 引擎与已有模块 |
| 对既有资产的盘点 | ❌ 遗漏 RecoveryEngine/AuditLog/transform_context/ComprehensiveAgent |
| 行动排序(先大重构 Pipeline) | ⚠️ 风险高、收益滞后;应先接线后重构 |
| Phase 48 自我进化 | ⚠️ 缺安全护栏,需补"建议-批准"层 |

**采纳建议**:接受其 Hook 扩充、Trace 落盘、Processor 化的**目标**;
但**执行顺序**改为先做我 P0/P1 的接线与旁路 trace(低成本高回报),
再考虑 P2 的 loop 分段;AEGIS 自我进化设为远期且强制人工批准。

---

## 附:核验过的关键代码位置

- 主循环:`core/src/agent/mod.rs:424` `run_loop`
- 上下文 7 段引擎:`core/src/context.rs:1`(模块文档)与 `ContextSegment`
- transform_context 接缝:`core/src/agent/mod.rs:144`、接进 loop 在 `:444`
- RecoveryEngine(未接线):`core/src/error_recovery/mod.rs`、仅 `lib.rs:42` 导出
- Hook 死代码:`core/src/hooks/mod.rs` 的 `fire_session_start/end` 无调用点
- AuditLog(仅权限):`core/src/permission/audit.rs`、接进 `PermissionPolicy:289`
- 工具并行:`core/src/agent/executor.rs` `ToolOrchestrator` + `ToolExecutionMode::Parallel`
