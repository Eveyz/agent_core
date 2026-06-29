# 2026-06-29: Analysis of Mode Switching and Cache Hit Strategies

**Date**: 2026-06-29
**Subject**: Architectural Review of Mode Switching (Run vs Session) and its Cache Hit Implications
**Author**: Antigravity AI Agent

## 1. Run 与 Session 的关系
在我们的架构中，**Run 和 Session 是两个不同层级的概念**。
- **Session** 是用户视角下的完整对话上下文。
- **Run** 是后端视角下的**单次执行任务**（即处理用户一次 Input 的状态机）。
当我们在中途切换 Mode，然后发送新消息时，后端会调用 `RunManager::create_run` 开启一个新的 Run，但它会将当前的 `session_id` 和完整的 `history: Vec<Message>`（包括所有之前的对话）传给新的 Run。
**结论**：你在同一个 Session 下，**所有的历史对话和 Context 都会被完美继承**，不会有任何丢失。

## 2. 切换 Mode 对 Cache Hit 的影响
**确实有影响，但这是预期内的最佳行为。**
正如你所知，Cache Hit 依赖于 `System Prompt + 历史对话前缀` 的完全一致。
当切换 Mode 时（比如从 Ask 切换到 Build）：
1. 你的 `Principles` 变了（获得了写权限）。
2. 你的 `Tool Catalog` 变了（注入了 `bash`, `write_file` 等重型工具）。
因为这两个 Segment 被定义为 `Stability::Stable`，所以 System Prompt 发生了变化。这会导致**切换 Mode 后的第一轮对话必然发生 Cache Miss**（大模型需要重新计算 System Prompt 和历史记录）。
**但是，从第二轮开始**，只要你继续停留在当前 Mode，System Prompt 就会再次固定，之后的每一轮又会完美触发 Cache Hit。

## 3. 我们需要规避这个 Cache Miss 吗？（探讨 Router Agent）
你有两个很好的直觉：试图规避，或者引入 Router Agent。我们来详细对比这几种方案：

### 方案 A：强行规避 Cache Miss（将 Mode 设为 Dynamic）
- **做法**：如果我们把 `Principles` 和 `Tool Catalog` 改为 `Stability::Dynamic`，把它们挂在每轮最后一条 User Message 后面。System Prompt 永远不变，理论上永不 Miss。
- **代价**：这是一场灾难。因为 `Tool Catalog` 包含大量的 token，如果放在末尾，LLM 每轮都需要把这几千个 token 作为 "新内容" (Cache Miss 部分) 重新计算。这反而会让每轮对话都变慢。**放弃。**

### 方案 B：引入 Router Agent 架构
- **做法**：前端永远只和一个无状态的 Router Agent 聊天。Router Agent 评估意图，然后唤醒下属的 `Plan Subagent` 或 `Build Subagent` 来干活。
- **优点**：每个 Subagent 有自己独立的上下文和固定的 System Prompt，在各自的线程里享有 100% Cache Hit。
- **缺点**：上下文碎片化严重。用户在主界面的连续对话，会被切分到不同 Subagent 的内存中。当 Build Agent 需要用到 Plan Agent 的调研结果时，需要大量的信息传递。对于普通的日常编程流，这种架构太重了。

### 方案 C：当前的 "显式状态机" 架构（One-Time Miss）
- **做法**：用户在同一个主线对话中显式/隐式切换 Mode。切换时承受 **一次（仅仅一次）** Cache Miss 的代价。
- **优点**：极简、上下文连贯、收益最高。绝大多数情况下，用户会在 Plan 模式下待很久（全部 Hit），然后转入 Build 模式写代码（第一次 Miss，之后全部 Hit）。这几十毫秒到几秒的单次 Miss 代价，换来了极其顺畅的用户体验和极低的系统复杂度。

## 4. 结论与建议
我们**不需要**为了规避这一次 Cache Miss 而去修改架构或引入复杂的 Router Agent。
目前这种 "跨 Mode 切换时产生单次 Miss，Mode 内连续对话完美 Hit" 的表现，是业界在 Prompt Caching 机制下的**最优解 (Global Optimum)**。你的最初设计（将 Tool Catalog 放在 Stable 的 System Prompt 中）是非常有远见且正确的。
