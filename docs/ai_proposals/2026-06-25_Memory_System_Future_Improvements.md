# 2026-06-25 探索下一代智能记忆系统：最佳实践与改进方案

## 1. 业界最优秀的 Memory 系统是怎么做的？

目前 AI Agent 领域最顶尖的记忆系统架构主要分为以下几个流派：

### 1.1 MemGPT / Letta 模式 (OS-like Memory Hierarchy)
- **核心理念**：将大模型比作 CPU，把 Context Window 比作内存 (RAM)，把数据库比作硬盘 (Disk)。
- **Core Memory (核心内存)**：分为 `Persona`（AI 自己的角色设定）和 `Human`（用户画像）。这部分容量极小但**永远存在于上下文**中。Agent 必须主动通过 `core_memory_append` / `core_memory_replace` 工具来修改它。
- **Archival Memory (归档存储)**：无限大。Agent 发现超出 Context 时，会主动触发 `archival_memory_search` 进行“分页查询”。

### 1.2 Claude Code / Cursor 模式 (Document-Driven Memory)
- **核心理念**：极度透明，所见即所得。抛弃“黑盒”的向量数据库。
- **机制**：强依赖工作区里的文本文件（如 `CLAUDE.md`, `.cursorrules`, `docs/`）。当系统学到新东西时，强制大模型使用 `edit_file` 将知识转化为 Markdown 记录下来。人类可以直接 review AI 的记忆并进行干预。

### 1.3 GraphRAG / LightRAG (知识图谱驱动)
- **核心理念**：从“语义相似度”升级为“实体关系推理”。
- **机制**：当用户输入一句话时，后台 LLM 提取出实体（Entity）和关系（Relationship）。比如提取出 `[zniverse] --(works_at)--> [Bank]`。这种网状结构在检索复杂代码架构（如“哪个服务调用了用户模块”）时，比传统的向量搜索准确度高出几个量级。

### 1.4 Zep / LangChain 长效记忆 (Background Consolidation)
- **核心理念**：不占用主线程的 Token，将记忆整理异步化。
- **机制**：主 Agent 只负责干活。后台会起一个小模型（如较便宜的 Flash/Haiku），每隔一段时间偷偷把最近的对话提取摘要、识别关键事实（Facts），然后汇总覆盖旧的记忆。

---

## 2. 审视我们 `agent_core` 的现状

目前我们在 `agent_core` 里其实已经拥有了非常豪华的**三轨记忆组合**：
1. **Core Memory 工具库 (`core_memory.rs`)**：支持 `append`/`replace`，存储在 `human` 等 block 中。
2. **Recall/Archival Memory (`recall.rs`)**：基于 SQLite + 向量计算，每次交流自动进行语义检索和“遗忘半衰期”衰减打分。
3. **Global Project Memory (`agverse.md`)**：刚刚优化的全局文档记忆，靠 Agent 主动调用文件工具维护。

**目前存在的痛点（不够智能的地方）：**
- **职责重叠导致 AI 精神分裂**：我们同时提供了 `core_memory_append` 工具和编辑 `agverse.md` 的能力。当了解到“用户喜欢 Rust”时，AI 到底该把这句话放进 `human` block，还是写进 `agverse.md`？
- **自动向量检索往往会召回垃圾信息**：目前我们的底层每次都会拿着用户的 `last_user_message` 去向量库里查最相似的前 3 句话。如果用户说“哈哈”，系统就会从历史里翻出另外 3 句“哈哈”，这对解题毫无帮助，反而浪费 Token。

---

## 3. 我们该如何改进？（Make it smarter）

要让我们的系统达到甚至超越业界最优秀的水平，我建议从以下四个方面进行深度改造：

### 改进一：统一定义记忆的“楚河汉界” (Boundary Alignment)
我们需要在 System Prompt 中为 Agent 提供极为明确的记忆写入准则：
- **`core_memory (Human/Persona)` 工具**：仅用于存储**非常私人且跨项目**的简短属性（如：名字、职业、习惯用语、当前状态）。
- **`agverse.md` (Document)**：仅用于存储**跟当前项目绑定**的技术栈、架构决策、代码规范、TODO 进度等长篇知识。
- 这样 AI 就不会混乱，人类开发者看文件时也能做到心中有数。

### 改进二：引入异步“记忆整理员” (Background Consolidator Subagent)
我们现在的 `MemoryConsolidator` 是基于相似度聚类的硬代码。这不够聪明。
- **做法**：我们可以定义一个专门的 `subagent`（比如叫 `Memory_Archivist`）。
- 每当主 Agent 完成一次复杂任务或积累了超过 20 轮对话，就异步唤醒这个 Archivist，让它在后台阅读近期的冗长对话，提炼出事实（Facts），然后由它去调用更新工具。这样主 Agent 永远保持专注且 Context 不会膨胀。

### 改进三：将 Vector 降级，赋予 Agent “主动回忆”的权力
- 当前系统在底层“强制”把 Vector 召回的 3 条信息塞进 Prompt，这是很不智能的被动投喂。
- **做法**：关闭底层的强制召回，仅将 `recall_memory_search` 作为一个 Tool 暴露给 Agent。只有当 Agent 觉得“这个问题我以前好像解决过，但我忘了具体命令”，由 Agent **主动去搜**。这才是真正在模拟人类的工作流。

### 改进四：从 Text 转为 Entity (构建迷你代码关系图谱)
- 对于复杂的代码仓库（如 `agent_core`），我们可以引入轻量级的 Graph 机制。
- 当 Agent 探索完一个复杂的模块（比如 `run.rs`），不仅仅是存一段文本总结，而是存入：`[run.rs] --(depends_on)--> [paths.rs]`。当未来某个 Bug 出现时，Agent 可以沿着依赖链索骥，这对于解决千丝万缕的代码 Bug 有着降维打击般的效果。

## 结语
我们目前的基建（SQLite、向量检索、Core Memory Blocks、全局文档）已经非常完备。下一步的重点不再是“增加怎么存的代码”，而是“**把主动权还给 LLM，让它像人类一样，有选择性地、异步地去记笔记和翻笔记**”。
