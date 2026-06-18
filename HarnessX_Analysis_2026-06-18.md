# HarnessX 与 agent_core 源码级对比分析报告 (2026-06-18)

在深入阅读了 `core/src/agent/mod.rs`、`core/src/hooks/mod.rs` 以及 `core/src/permission/mod.rs` 的实际代码后，我对 `agent_core` 的架构有了更透彻的理解。

结合《HarnessX》论文的核心思想，以下是基于**实际代码级别**的现状分析与深度重构建议：

---

## 1. 源码现状剖析 (Static Harness 的真实形态)

虽然文档中描绘了一个高度模块化的框架，但源码揭示了目前的 `agent_core` 在系统设计上仍处于 **“强耦合的静态脚手架（Monolithic Static Harness）”** 阶段，距离 HarnessX 的可组合性（Composition）还有一段距离：

### 1.1 核心循环（Agent Loop）严重硬编码
在 `core/src/agent/mod.rs` 中，`run_loop` 方法长达上百行，整个 ReAct 的生命周期被直接硬编码在了循环里：
- `refresh_context_segments()` 强绑定了环境变量读取、Tool Catalog 构建和 Core Memory 读取。
- 阶段 4 的 LLM 压缩 (`maybe_llm_compact`) 直接插入在 `run_loop` 中。
- 模型流式生成 (`chat_completion_stream`) 和工具执行 (`ToolOrchestrator`) 也是串行写死的。
- **与 HarnessX 差距**：HarnessX 主张将这些行为全部抽象为 `Processor`，Agent 本身只是一个空的管道（Pipeline）。

### 1.2 Hook 系统并未贯穿全局
查阅 `core/src/hooks/mod.rs` 发现，目前仅定义了四个事件：`PreToolUse`、`PostToolUse`、`SessionStart`、`SessionEnd`。
- **与 HarnessX 差距**：论文中定义了 8 个关键 Hook（`task_start`, `step_start`, `before_model`, `after_model`, `before_tool`, `after_tool`, `step_end`, `task_end`）。目前的 Hook 过于关注 Tool，而无法在模型推理前后（如 `before_model`）或每一步开始时注入干预逻辑。

### 1.3 绝佳的 Observability（可观测性）底子
代码中大量使用了 `on_event(AgentEvent::...)`，涵盖了 `TurnStart`、`MessageUpdate`、`ToolExecutionEnd` 等详尽的事件。
- **巨大的潜力**：这正是 HarnessX 中 AEGIS 引擎最需要的 **Trace 记录源**！我们只需要写一个监听这些事件的消费者，就能零成本地构建出结构化的轨迹存储库。

---

## 2. 基于源码落地的改进方向与架构重构 (The AEGIS Path)

要想让 `agent_core` 从一个好用的框架，蜕变为一个能够**自我进化**的 Agent Harness Foundry，我们需要进行以下底层重构：

### 方向一：将 Monolithic Loop 重构为 Processor Pipeline (Composition)

我们要把 `agent.rs` 中的硬编码逻辑全部抽离为独立的组件（Processors）。

1. **补全 Hook 系统**：在 `HookEvent` 枚举中新增 `StepStart`, `BeforeModel`, `AfterModel`, `StepEnd` 等事件。
2. **定义 Processor Trait**：让每个逻辑变成一个 Processor。例如：
   - `EnvironmentProcessor`: 监听 `StepStart`，负责将 CWD 和系统信息更新到 Context。
   - `LLMCompactProcessor`: 监听 `BeforeModel`，检查 Token 数量并执行 `maybe_llm_compact`。
   - `PermissionProcessor`: 监听 `PreToolUse`，执行现在的 6 层权限校验。
3. **改造 AgentBuilder**：Agent 内部不再包含具体的业务逻辑，而是维护一个挂载在各个 Hook 上的 Processor 列表。
   - **好处**：这完全实现了 HarnessX 的组件化理念（Substitutable entity）。未来 Meta-Agent 要改变当前 Agent 的行为，不需要重写 Rust 代码，只需要修改挂载的 Processor。

### 方向二：实现 Tracer 组件沉淀执行轨迹 (Observability)

AEGIS 的自我进化（Adaptation）完全依赖于执行历史的复盘。

1. **开发 `TraceCollector`**：作为传递给 `Agent::run_with_events` 的回调闭包。
2. **数据持久化**：将捕获的 `AgentEvent` 流转换为 JSONL 格式的 Trace 树，保存在类似 `.agent_core_history/traces/<task_id>.jsonl` 中。
3. 必须确保包含完整的 `Context` 快照、工具调用的原始参数、工具返回结果以及最终的任务验证评分（Verifier Score）。

### 方向三：开发离线 AEGIS Meta-Agent (Adaptation)

目前 `agent_core` 的配置（`config.toml`）和技能（`SKILL.md`）都是手写的。

我们可以利用现有的 `Subagent` 或新开一个 CLI 模式 `/evolve`，作为一个后台的 Meta-Agent（即 HarnessX 里的 AEGIS）：
1. **Digester 阶段**：让它读取上一步生成的 Trace 文件，分析诸如“连续发生工具权限拒绝（Permission Denied）”、“因上下文过长导致幻觉”等具体问题。
2. **Evolver 阶段**：让它自动修改环境配置。例如：
   - 如果发现某个网站抓取老是失败，自动修改 `config.toml`，把 `mcp.servers.filesystem` 换成更好的 MCP 爬虫服务器。
   - 如果发现由于某些逻辑导致循环调用（Looping），自动写一个新的 `SKILL.md` 并标记高优先级，通过 `SkillManager` 注入以规避循环。
   - 或者调整 `config.toml` 中 `permissions.mode`，动态放宽/收紧权限。

---

## 3. 下一步的行动路线总结

从代码层面来看，你们已经造出了一台马力强劲的跑车（Static Harness），现在的目标是给它装上**自动驾驶（AEGIS Evolution）**。

建议执行步骤：
1. **Phase 46：重构 Agent Loop (Processor 抽象化)**
   将 `core/src/agent/mod.rs` 解耦，扩充 `HookEvent` 并将现有的状态刷新、上下文压缩拆分为独立的 `Processor` 结构。
2. **Phase 47：实现 Trace 日志落盘**
   利用现有的 `AgentEvent` 系统，实现高保真轨迹（Trajectory）记录引擎。
3. **Phase 48：构建离线反思代理 (Offline Reflection Agent)**
   这是 AEGIS 的雏形。写一个简单的逻辑：跑完任务后，如果失败，自动启动一个新的 Agent 实例读取本次的 Trace，并自动生成/修改一个 `SKILL.md`，然后重试任务，直到任务成功。

这种基于 Trace 自动修改自身配置的机制，将是 `agent_core` 架构的最终极形态！
