# agent_core 项目长期记忆

## 技术决策

### TUI 方案
- **决定**：基于 ratatui 0.30 继续优化，不引入 TypeScript 前端
- **理由**：Rust 核心库 63 文件、196 测试，引入 TS 需要 IPC 层 + 双重类型 + 两套构建，性价比低
- **已优化**：pulldown-cmark 解析 markdown、syntect 语法高亮、输入快捷键增强
- **如果将来不够**：用 Tauri + WebView 嵌入前端，不拆 IPC 协议

## 架构关键

- TUI 层只是 AgentEvent 流的消费者，不包含业务逻辑
- render.rs 中的所有 TurnBlock 渲染（Thought/Tool/Subagent/Notice/Error）是项目特有的 UI 组件，不能简单地用通用 markdown 替换
