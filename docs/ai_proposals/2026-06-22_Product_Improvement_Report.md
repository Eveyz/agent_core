# 产品改进建议报告

## 项目定位判断

当前项目是一个基于 **Tauri + React + Rust + agent_core** 的桌面版 AI Agent 客户端。它已经具备本地项目管理、会话管理、模型配置、工具调用、权限审批、MCP、Memory、Subagent 等底层能力。

从产品角度看，它更像一个“能跑的 Agent 容器”，但还没有完全成为“用户每天离不开的开发工作台”。后续重点不应只放在代码质量，而应聚焦在：

- 任务闭环
- 用户信任
- 可控执行
- 可视化反馈
- 项目上下文理解
- Git/PR 工作流
- 多 Agent 协作
- 长任务和后台运行

目标是从“聊天式代码助手”升级为“可信赖的 AI 开发工作台”。

---

## 1. 从聊天窗口升级为任务工作台

当前核心体验仍是：用户发消息 → Agent 执行 → 聊天流展示。

但开发者真正关心的是：

- Agent 现在做到哪一步了？
- 改了哪些文件？
- 有没有跑测试？
- 哪些地方有风险？
- 最终能否提交或合并？

建议引入 **Task Workspace / Run 面板**。每次用户发起任务后，自动创建一个任务对象，包含：

- 目标
- 当前阶段
- TODO 计划
- 已读文件
- 已修改文件
- 已运行命令
- 测试结果
- 风险点
- 最终 Diff
- 是否准备提交

这样可以弱化聊天本身，强化任务执行状态和交付物。

优先级：**最高**

---

## 2. 增加 Diff Review 和 Apply/Revert 流程

AI Agent 产品要让用户放心，必须有明确的变更审查入口。

建议增加 **Changes 面板**：

- 文件级 diff
- 每个 hunk 可接受/拒绝
- 一键全部应用
- 一键回滚本轮修改
- 查看 Agent 修改理由
- 展示修改前后测试状态
- 自动生成 commit message
- 一键创建 commit

这会显著提升用户信任，让 Agent 从“自动改代码的黑箱”变成“可审查、可回滚、可交付的助手”。

优先级：**最高**

---

## 3. 把 Subagent 做成显性产品能力

项目底层已经有 subagent 能力，但目前更像隐藏工具，用户未必感知得到。

建议产品化成 **并行专家模式**：

- Architect：分析架构和方案
- Implementer：负责实现
- Tester：补测试并运行验证
- Reviewer：审查变更
- Debugger：分析失败日志
- Researcher：查资料和外部文档

UI 上可以显示多个 Agent 卡片：

- 当前正在做什么
- 关键结论
- 使用了哪些工具
- 与其他 Agent 的关系
- 最终合并意见

这会让产品从普通聊天助手变成“AI 开发团队”。

优先级：**高**

---

## 4. 增加 Ask / Plan / Agent / Review / Debug 模式

当前用户发出一条消息后，Agent 的行为边界不够明确：是否会改文件？是否会运行命令？是否会触发高风险操作？

建议增加模式切换：

### Ask

- 只回答问题
- 不修改文件
- 不运行危险命令

### Plan

- 分析项目
- 提出计划
- 可读取文件
- 不修改代码
- 需用户确认后进入执行

### Agent

- 可编辑文件
- 可运行命令
- 按权限策略审批

### Review

- 专门审查当前 diff、commit 或 PR

### Debug

- 专门处理报错、日志、测试失败

清晰的模式能降低用户焦虑，也能提升产品专业感。

优先级：**高**

---

## 5. 增加 Project Context / Project Brief 面板

当前项目管理主要是目录和会话，还缺少“这个项目是什么”的持续上下文层。

建议为每个项目生成并维护一个 **Project Brief**：

- 技术栈识别
- 启动命令
- 测试命令
- lint 命令
- build 命令
- 包管理器
- 主要目录结构
- 代码风格
- 常用约定
- 环境变量说明
- 当前 git 分支
- 最近任务
- 重要文件索引

第一次添加项目时，Agent 自动扫描并生成 Project Brief。之后每次任务默认带上该上下文，减少用户重复说明，提高 Agent 的项目理解能力。

优先级：**高**

---

## 6. 做“需求到实现”的结构化流程

侧边栏中已有 `New requirement` 入口，但目前尚未形成完整产品能力。

建议将其升级为 **Requirement Workflow**：

1. Clarify：Agent 反问需求缺口
2. Spec：生成简短需求说明
3. Design：生成实现方案
4. Tasks：拆分任务
5. Implement：逐步实现
6. Verify：测试和检查
7. Review：输出改动摘要和风险

这适合中大型需求，可以让产品区别于普通聊天式 Agent。

优先级：**高**

---

## 7. 增加可恢复的长任务和后台执行

桌面 Agent 应支持更强的长任务体验。

建议支持：

- 任务运行中用户可切换会话/项目
- 每个 Run 独立保存状态
- 关闭 App 后可恢复
- 失败后可从某一步继续
- 支持“完成后通知我”
- 支持任务队列
- 支持定时任务

这类能力是 Devin、Codex Cloud Tasks、Cursor Background Agents 等产品的重要方向。

优先级：**中高**

---

## 8. 增加 GitHub / PR 工作流

如果面向开发者，GitHub/PR 是非常重要的工作闭环。

建议支持：

- 选择 GitHub Issue，让 Agent 实现
- 自动创建分支
- 实现后生成 commit
- 自动创建 PR
- 自动生成 PR 描述
- PR Review 模式
- CI 失败后自动分析和修复
- 根据 review comment 继续修改代码

这样产品可以从“本地工具”进入真实团队研发流程。

优先级：**中高**

---

## 9. 引入 Bench / Quality Gate

AI Agent 改代码最大的问题是用户不信。因此需要用客观检查结果建立信任。

建议增加质量门禁面板：

- Typecheck 是否通过
- Test 是否通过
- Lint 是否通过
- Build 是否通过
- 覆盖率变化
- 性能基准变化
- 安全扫描结果
- 是否有未处理 TODO
- 是否改动敏感文件

每个任务完成时，Agent 自动运行项目配置中的验证命令，并给出明确状态。

优先级：**高**

---

## 10. Memory 产品化为团队知识库

项目已有 memory 模块，但产品上不应只是“黑箱记忆”。

建议将 Memory 分为几类：

### User Preferences

- 用户偏好
- 输出风格
- 常用模型
- 不喜欢的做法

### Project Rules

- 项目代码规范
- 架构约束
- 测试要求
- 禁止修改区域

### Decisions

- 架构决策记录
- 关键实现原因

### Snippets / Recipes

- 常用命令
- 部署步骤
- Debug 方法

### Learned Facts

- Agent 从历史任务中学到的事实

同时需要 UI 支持查看、编辑、删除、锁定，避免 Memory 成为不可控黑箱。

优先级：**中高**

---

## 11. MCP Marketplace / Integration Center

项目已有 MCP 支持，建议进一步产品化为集成中心。

可支持：

- GitHub
- Linear
- Jira
- Notion
- Slack
- Supabase
- Postgres
- Browser
- Figma
- Sentry
- Vercel
- Cloudflare
- Docker
- Kubernetes

产品能力包括：

- 一键添加 MCP Server
- 健康检查
- 权限说明
- 工具列表预览
- 测试调用
- 每个项目单独启用/禁用 MCP

不要只让用户填写 JSON 配置，应尽量降低集成门槛。

优先级：**中**

---

## 12. 增加 Terminal / Browser / Preview 三件套

很多 coding agent 的瓶颈是不能完整观察运行结果。

建议加入：

### 内置 Terminal

- 展示 Agent 跑过的命令
- 用户可手动输入命令
- Agent 可引用终端上下文

### App Preview

- 对前端项目自动启动 dev server
- 内置 WebView 预览
- Agent 能读取 console error、network error、screenshot

### Browser Automation

- Agent 可点击页面
- 观察 UI 状态
- 验证交互结果

这会让产品从“代码修改器”变成更完整的开发环境。

优先级：**中高**

---

## 13. 加入 Agent Trace / Replay

项目已有 event log，很适合产品化为 Trace / Replay。

建议支持：

- 时间线展示
- 每一步输入输出
- 工具调用参数
- 工具结果
- token / cost
- 耗时
- 模型切换
- 错误重试
- 从某一步 fork 新会话
- 分享 run report

这有助于调试 Agent、建立信任，也适合团队协作和问题复盘。

优先级：**中**

---

## 14. 成本与模型路由

项目支持多 provider/model，但还可以进一步产品化。

建议支持：

- 每次任务预计成本
- 实际 token / cost
- 按任务类型推荐模型
- 快速 / 便宜 / 最强 三档选择
- 自动模型路由：
  - 简单搜索用小模型
  - 架构设计用强模型
  - 长上下文阅读用长上下文模型
  - 代码审查用专门模型
- 超预算提醒

用户往往不是不会配置模型，而是不知道某个任务该用哪个模型。

优先级：**中**

---

## 15. 重做 Onboarding

目前项目仍有模板痕迹，首次使用体验需要产品化。

建议首次启动向导包括：

1. 选择模型 Provider
2. 填写 API Key 或选择本地模型
3. 添加第一个项目
4. 自动扫描项目
5. 运行一个安全 demo task
6. 展示权限机制
7. 告诉用户如何 Review Diff

这会显著降低新用户流失。

优先级：**高**

---

# 建议路线图

## 第一阶段：让它可信

目标：用户敢让它改代码。

- Diff Review 面板
- Apply / Revert 本轮改动
- Ask / Plan / Agent / Review 模式
- Quality Gate：test / build / lint 状态
- Project Brief 自动生成
- Onboarding

## 第二阶段：让它高效

目标：用户觉得它比自己快。

- Task Workspace
- 结构化 TODO / 阶段状态
- Background Runs
- 内置 Terminal
- 自动测试 / 修复循环
- Git branch / commit 工作流

## 第三阶段：让它专业

目标：能服务真实团队和复杂项目。

- GitHub Issue → Branch → PR
- PR Review / CI 修复
- Memory 知识库 UI
- MCP Integration Center
- Agent Trace / Replay
- 成本统计和模型路由

## 第四阶段：形成差异化

建议重点选择以下两个定位之一。

### 方向 A：本地优先的 Agent 开发工作台

卖点：

- 数据和代码主要在本地
- 权限透明
- Diff 可控
- 支持任意模型
- 支持 MCP
- 可审计、可回放

适合个人开发者、小团队、对隐私敏感团队。

### 方向 B：AI 开发团队 Orchestrator

卖点：

- 多 Agent 并行
- Architect / Implementer / Reviewer / Tester 分工
- 任务看板
- 长任务后台执行
- PR 交付闭环

适合复杂需求和团队研发流程。

---

# 最重要的三个改进

如果只能选择三个功能优先做，建议是：

1. **Diff Review + 一键回滚 / 提交**
2. **Task Workspace：计划、进度、工具、测试、交付物可视化**
3. **Project Brief + Quality Gate：让 Agent 理解项目，并用测试结果证明完成**

这三个完成后，产品体验会从“AI 聊天壳”明显升级为“可信赖的 AI 开发助手”。
