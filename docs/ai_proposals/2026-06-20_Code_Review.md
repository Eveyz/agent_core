# 代码评审 — 2026-06-20

评审范围：`tauri` 分支当前 HEAD（`861f9af feat(core): harness engineering phases A-F` 之后）。
重点放在主战场 **React UI + Tauri 桥接**，兼顾 Rust core。
结论：上一版 `Independent_Review_2026-06-18.md` 里的 P0/P1（恢复引擎、trace collector、staged loop、session hooks）**已实现并合入**，本次只报**当前代码里仍然存在的问题**。

> 整体架构是健康的：core/app/cli 分层干净，agent loop 完成度高，Tauri 用 `state_changed` 事件流推送而非轮询。下面是按优先级排序的待修项。

---

## P0 — 用户每天都感受得到

### P0-1. 工具执行声称"并行"但实际串行，且文档/代码自相矛盾

`core/src/agent/executor.rs:233-255` 的 `ToolExecutionMode::Parallel` 分支里，作者自己留了 TODO：

```text
// TODO: wrap ToolRegistry in Arc to enable true parallel execution
// via JoinSet. Currently sequential due to &self borrow constraints.
```

即所谓"并行模式"和串行分支**逐字相同**，都是 `for ... await`。但
`Independent_Review_2026-06-18.md` §2.4 却断言"工具并行执行已实现"——文档不可信。

**正确解法（采纳）**：不是无脑全并行，也不是无脑串行，而是**按 DAG 调度**。
模型一次返回多个工具调用时，互相不依赖（读写不同资源）的并行跑；有依赖（写同一文件、
后一个读前一个写的文件）的按拓扑序串行。`Tool: Send + Sync` 已满足，
`&ToolRegistry` 本身就是 `Sync` 的，**根本不需要 Arc**——那个 TODO 的前提是错的。

实现要点见本次提交 `core/src/agent/scheduler.rs`。

### P0-2. MarkdownContent 流式渲染 O(n²)

`app/src/components/chat/MarkdownContent.tsx`：assistant 每吐一个 delta，就把**整段累积文本**
重新跑一遍 markdown 解析 + 语法高亮。turn 一长，每个 delta 开销 O(n)，整体 O(n²)，掉帧明显。

**修法**：流式期间用轻量纯文本渲染（plain `<pre>` 或极简组件），`isStreaming=false` 后
再切回完整 MarkdownContent；并对最终解析做 `useMemo(content)`。

---

## P1 — 正确性 / 安全

### P1-1. Tauri 同步命令做阻塞 I/O

`app/src-tauri/src/lib.rs`：`list_directory`、`git_*` 等 `#[tauri::command]` 是同步签名，
内部却是文件系统 / `Command::output`（阻塞）I/O，占用 Tauri 同步命令线程池，并发时 UI 卡顿。

**修法**：这些命令改为 `async`，I/O 放进 `tokio::task::spawn_blocking`。

### P1-2. 审批队列与 agent mutex 共锁

`send_message` 在异步 agent mutex 下跑整个 turn，`approve_tool` 又要走同一把锁唤醒等待中的工具。
长 turn 会阻塞所有 approve/list 操作。

**修法**：审批通道（`global_pending_approvals`）已经独立于 agent handle，
但 Tauri 层的 `approve_tool` 仍需确认不重入 agent 锁。本次确认并加固。

### P1-3. 密钥明文写在 config.toml，且存在两份配置

根 `config.toml` 与 `app/config.toml` 是**重复配置**，明文存放多个 provider 的 api_key。
根目录的已 gitignore，但 `app/config.toml` **未被 gitignore**（只是恰好没被 commit）。
`config.rs` 的 default 路径里还有硬编码 key。

**修法**：
- `app/config.toml` 加入 `.gitignore`；
- config 支持 `api_key_env = "XXX"` 从环境变量读 key（兼容旧的明文 `api_key`）；
- 清理 `config.rs` 里的硬编码 default key。

---

## P2 — 工程质量

### P2-1. useAgentEventListener 事件无批处理

`app/src/hooks/useAgentEventListener.ts`：每个 `state_changed` 事件单独 `dispatch`，
流式时一秒几十次 dispatch = 几十次重渲染。修法：rAF / 微任务攒批 flush。

### P2-2. 消息列表缺稳定 key + 虚拟化

长会话全部在 DOM 里，且没看到 `react-window`/`virtuoso`。补稳定 key + 虚拟化。

### P2-3. dead code 与命名残留

`cargo build` 实锤 8 个 warning：`pending_approvals_override` 字段、`tasks::detect_cycle`
未使用（DAG 调度接入后会被使用）。`.gitignore` 里 `.deepseek`/`.workbuddy`/`.agent_core_history`
命名残留。本次清理。

---

## 评分

| 维度 | 评价 |
|---|---|
| 架构分层 | 很好，core/app/cli 干净分离 |
| Rust core | 良好，agent loop 完成度高；"假并行"是最大瑕疵（本次按 DAG 修正）|
| Tauri 桥接 | 中等，sync 命令阻塞 + 审批/turn 共锁（本次修复）|
| React UI | 中等偏下，流式性能首要瓶颈（本次修复）|
| 安全/配置 | 中等，密钥管理需正规化（本次修复）|
