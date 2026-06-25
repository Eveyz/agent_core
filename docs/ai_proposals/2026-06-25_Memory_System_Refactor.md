# 2026-06-25 Memory System Refactor & Analysis

## 1. 背景与问题 (Background & Issue)
原先的 Agent 记忆系统采用“基于 Session 的日志转储”模式，会在任务结束时，将原始对话日志（例如 `User: 我是谁` -> `Assistant: 你是...`）按时间戳保存至 `~/.agverse/memories/session_{timestamp}.md`，或是在最近的代码迭代中，无差别追加到全局 `~/.agverse/agverse.md` 的 `# 自动记忆 (Auto Memories)` 区域。

**问题表现：**
1. **文件极度冗杂**：随着会话增加，全局文件堆积了大量无价值的闲聊记录，不仅扰乱人类阅读，更浪费了 LLM 的 Context 窗口。
2. **缺乏智能分类**：像 `CLAUDE.md` 这样的全局最佳实践，要求内容高度凝练并按模块分类（如 `User Preferences`, `Coding Conventions`, `Architecture Decisions`）。传统的字符串追加逻辑无法实现智能分类。

## 2. 根因分析 (Root Cause Analysis)
Rust 运行时的生命周期钩子 (`cancel_and_cleanup` 等) 是同步环境，无法低成本地发起一次 LLM 请求来深度总结和分类会话。
试图用底层硬编码（Hardcode）的方式去完成“知识整理”是不现实的。真正的“全局记忆系统”应当由 Agent **自主感知**并在必要时**主动维护**，而不是由底层运行时无脑倾倒日志。

## 3. 架构演进与改动 (Implementation Changes)
为实现“真正的 Claude Code 模式”，我们对系统的记忆轨做出了如下改造：

### 3.1 废弃无脑追加日志，引入“主动更新”模式
- 修改了 `agent_core/core/src/runtime/run.rs` 中的 `write_session_memory` 方法。
- 该方法现在**仅承担模板初始化职责**：当发现不存在全局记忆文件时，才会生成一个标准模板文件到 `~/.agverse/agverse.md`。
- **停止了**所有原始会话记录 (raw logs) 往文件尾部追加的行为，避免了全局指令文件被污染。

### 3.2 注入系统级文档维护规则
在初始化的 `agverse.md` 模板中，我们通过系统指令明确赋予了后续 Agent 维护该文件的职责：
> "If new user preferences, architectural rules, or global instructions are discovered during the conversation, use file editing tools (like replace_file_content or edit) to intelligently update this `~/.agverse/agverse.md` file and classify the new information into the correct section above."
- 这一改动实现了“授人以渔”。以后的 Agent 会像维护代码一样，主动去重、分类、修改自己的全局记忆配置。

### 3.3 移除旧的路径依赖
- 彻底删除了原先加载和维护零散 session 文件的 `load_recent_memories` 逻辑。
- 在 `core/src/paths.rs` 中清理了已废弃的 `get_memories_dir()`。

## 4. 当前版本的双轨记忆模型总结 (Current Dual-Track Memory Model)

经过改造后，Agent 现在的长期记忆分为两条界限分明、互不干扰的轨道：

| 维度 | 第一轨：全局文档记忆 (Global Document Memory) | 第二轨：向量语义记忆 (Vector Semantic Memory) |
|---|---|---|
| **存储介质** | Markdown 文件 (`~/.agverse/agverse.md`) | SQLite 数据库 (`~/.agverse/memory.db`) |
| **内容属性** | 高度凝练的配置、规范、偏好、项目架构总览。 | 碎片化、细节化的每一次对话原句和相关上下文。 |
| **触发方式** | Agent 根据对话进展，**主动调用工具** (Edit/Write) 智能写入。 | 底层 `MemoryManager` 在消息流转时**自动**存入。 |
| **加载方式** | 全量加载：随 `build_system_prompt` 作为 `Global Project` 指令 100% 注入 Context。 | 语义检索加载：每次用户发起新 Query，自动根据余弦相似度捞取 Top 3 最相关历史片段注入 Context。 |
| **数据分类** | 极度清晰（人工与 Agent 共同维护的分块结构）。 | 无需分类（依赖 Embedding 模型和 `SalienceScorer` 打分进行模糊匹配）。 |

## 5. 后续开发建议 (Next Steps)
- 确保系统 Prompt 中继续鼓励 Agent 使用 File Edit 工具处理 `agverse.md`。
- 其他 Agent 可根据此报告完全信赖 `agverse.md` 内的数据纯度，不再担心里面掺杂无意义的闲聊内容。
