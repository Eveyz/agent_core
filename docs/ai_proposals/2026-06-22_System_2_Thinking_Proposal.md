# 2026-06-22 System 2 Thinking Proposal

## 背景与目的 (Background & Objective)

目前 `agent_core` 框架主要采用 ReAct (Reason + Act) 模式，这种“系统 1”思考方式在处理单步、简单的代码生成和工具调用时表现良好。然而，当框架面对长链路的复杂编程任务（如大规模重构）或通用非代码任务（如复杂工作流编排、研究报告撰写）时，ReAct 贪心且短视的缺陷会暴露出来，容易陷入死循环或产生破坏性的副作用（如不可逆的 API 调用）。

本提案建议在 `agent_core` 中引入“系统 2 思考”机制（如 Tree of Thoughts, ToT 和 Monte Carlo Tree Search, MCTS），通过“先规划推演，后物理执行”的策略，大幅提升 Agent 的决策质量、容错率以及在通用任务场景下的可靠性。

## 核心机制设计 (Core Mechanisms)

### 1. 思维树与沙盒推演 (Tree of Thoughts & Sandbox Simulation)
* **多分支规划 (Multi-branch Planning)**：面对复杂任务，Agent 应一次性生成多种解决方案树（例如 3 种不同的代码重构策略，或 3 种不同的差旅预订路线）。
* **虚拟执行 (Dry-Run / Simulation)**：在内存沙盒中顺着分支进行推演，而不产生实际的物理副作用。
* **分离的 Actor 和 Critic 角色**：
  * **Actor**：负责展开分支、生成代码或 API 调用序列。
  * **Critic**：负责对推演结果进行打分（评估安全性、可行性、风险度），并将价值分数回传（Backpropagation）。

### 2. 细粒度的人在回路 (Fine-grained Human-in-the-Loop, HITL)
* 放弃单纯的“阻断式”权限管理，走向“干预式”管理。
* **规划可视化与确认**：当 Critic 选出最优方案后，在执行具有物理副作用的操作前（特别是不可逆的提交、支付、发邮件），可以通过 UI/TUI 将决策树展示给用户。
* **动态修正**：用户可以直接修改 Agent 生成的 Plan 树节点，然后让 Agent 继续顺着修正后的节点推演和执行。

### 3. 工具权限与副作用隔离 (Tool Side-Effect Isolation)
* 为了支持 MCTS/ToT，需要对工具进行读写分离的标记：
  * **Read-Only / Pure Tools**：如 `SearchWeb`、`ReadCalendar`、`CargoCheck`。可以在沙盒推演阶段无限次调用。
  * **Mutative / Side-Effect Tools**：如 `WriteFile`、`SendEmail`、`GitCommit`。必须被隔离在推演树之外，只有当最终高分路径确定并（可选）经过用户授权后，才统一作为事务（Transaction）执行。

## 架构演进建议 (Architectural Recommendations for `agent_core`)

1. **抽象思考空间 (Thought Workspace / Scratchpad)**
   * 为每个任务或 Subagent 提供一个临时的状态快照环境，允许它在不污染真实项目目录的情况下执行代码和尝试 API。
2. **利用 Rust 的并发优势**
   * 在执行到关键决策点时，利用现有的 Subagent 和 Task DAG 模块，`spawn` 多个轻量级的并发任务分别去请求 LLM 探索不同分支。
3. **状态快照与回滚 (State Checkpointing & Rollback)**
   * 支持快速将文件系统或状态机回退到上一个决策节点。当某条分支推演失败（如引发过多编译错误）时，能够自动执行类似 `git restore` 的动作，切断分支并尝试其他高分路径。

## 预期收益 (Expected Impact)

* **通用任务可靠性**：使得 `agent_core` 不仅是一个编程助手，更能胜任复杂的工作流自动化和深度研究任务，办事更加稳妥。
* **突破 ReAct 瓶颈**：极大地减少长链路任务中的幻觉累积和死循环，使其向业内顶尖水平（如 Devin 的深度规划能力和 o1 模型的自我修正能力）靠拢。
