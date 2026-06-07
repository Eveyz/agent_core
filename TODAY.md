# agent_core — 2026-06-07 开发总结

今天完成了 12 个 Phase（Phase 30 ~ CLI 补齐），193 测试全过，cargo check 零警告。

---

## Phase 30: 权限系统升级

**6 层规则引擎 + 用户审批流 + 审计日志**

```
Yolo 模式 → 黑名单(Deny) → 白名单(Allow) → 配置规则 → 内置规则 → 默认 Ask
```

| 文件 | 说明 |
|------|------|
| `src/permission/types.rs` **新建** | PermissionMode, DangerLevel, ApprovalScope, WhitelistEntry, AuditEntry, ApprovalPrompt, ApprovalChoice |
| `src/permission/whitelist.rs` **新建** | 持久化白名单管理器（查询、过期清理、config.toml 读写） |
| `src/permission/audit.rs` **新建** | JSONL 审计日志 + 统计 |
| `src/permission/mod.rs` **重写** | 6 层规则引擎，审批 prompt 含完整信息 |
| `src/permission/rules.rs` | 每个工具标注 DangerLevel（ReadOnly/ReadWrite/Network/System/Destructive） |
| `src/agent.rs` | oneshot channel 审批流程，Ask 时阻塞等待用户响应 |

审批选项：AllowOnce / AllowSession / AllowFor(5m) / AllowPersistent / Deny / DenyPersistent

---

## Phase 31-34: 上下文工程全面升级

### 31: 七段式上下文装配

重写 `src/context.rs` 为 ContextEngine：

```
Segment 1: IDENTITY          (stable, 200 tokens)
Segment 2: PRINCIPLES        (stable, 300 tokens)
Segment 3: ENVIRONMENT       (per-turn, 150 tokens)
Segment 4: TOOL CATALOG      (on-register, 动态)
Segment 5: ACTIVE MEMORY     (query-driven, 500 tokens)
Segment 6: LOADED SKILLS     (on-demand, 400 tokens)
Segment 7: EXECUTION PLAN    (per-turn, 300 tokens)
```

每段独立 token 预算、稳定性标记（Stable/SemiStable/Dynamic）、刷新策略。

### 32: 五段式压缩链路

`src/compressor.rs` **新建**：

```
snipCompact(截断) → dedupCompact(去重) → chunkCompact(合并) → summaryCompact(LLM摘要) → gradientCompact(梯度)
```

越老的轮次压缩越激进，最近 3 轮保留原文，11+ 轮仅保留引用。

### 33: 长会话自动压缩

Agent::maybe_llm_compact() — token > 95% 时自动调 LLM 摘要替换旧轮次。

### 34: Prefix KV Cache 感知

CacheHint 结构体：stable_prefix_tokens + full/partial/none 策略，为本地模型 KV cache 复用做准备。

---

## Phase 35-37: Memory 系统升级

### 35: 时间衰减显著性算法

`src/memory/salience.rs` **新建** — Ebbinghaus 遗忘曲线替换线性衰减：

```
之前: score = 0.6·semantic + 0.2·(1/(1+hours)) + 0.2·0.5
现在: score = 0.55·semantic + 0.25·e^(-t/(S×半衰期)) + 0.20·importance
```

| 特性 | 说明 |
|------|------|
| memory_strength | 访问时自增，越用越强 |
| 半衰期 | 可配置（默认 168h = 1周） |
| decay_modifier | 高重要性记忆衰减慢 3 倍 |
| auto_rate_importance | 启发式评分（决策词 +0.08、文件路径 +0.03 等） |
| MemoryCategory | 5 类分类器（Conversation/Decision/Code/Preference/Trivia） |

### 36: 自动显著性评分

store() 默认使用 auto_rate_importance，无需手动传 importance。

### 37: 记忆增强 & 主动遗忘

- bump_strength() — 检索后自动增强（S_new = S_old × 1.05 + 0.15，上限 5.0）
- prune_cold_memories() — 清除低分 + 低重要性 + 过时记忆
- promote_to_archival() — 高分旧记忆晋级归档库

---

## Phase 39: Session 管理器

`src/session.rs` **新建** — 完整的 session 管理：

```
sessions 表:       id, title, summary, start_time, end_time, message_count,
                   cwd, model_used, tags, archived, parent_session_id, session_type
session_messages:  session_id, msg_index, role, content, tool_calls(JSON), tool_call_id, name
```

| 命令 | 说明 |
|------|------|
| `/sessions` | 表格化历史列表 |
| `/session save` | 保存当前对话（自动生成标题） |
| `/session resume <id>` | 恢复对话到 context |
| `/session delete <id>` | 永久删除 |
| `/session rename <id> <t>` | 重命名 |
| `/session archive <id>` | 归档（软删除） |
| `/session search <kw>` | 按标题/摘要搜索 |

---

## Phase 40: Subagent 自动路由 + Session 隔离 + 并发

### 修复致命 Bug

`subagent_spawn` 之前创建 `ToolRegistry::new()`（**空的！**），子代理调用任何工具都失败。现在用 `ToolRegistry::from_names()` 工厂正确构建。

### 智能路由

```
task_execute 内部:
  ├─ 并行就绪任务 ≥ 1    → subagent（可并发）
  ├─ goal > 120 chars     → subagent
  ├─ ≥2 工具关键词        → subagent
  └─ 短 + 无工具词        → 内联执行
```

### 并发执行

新增 `subagent_spawn_all` 工具 — 多个子代理并发跑，`futures::join_all` 收集结果。

### Session 隔离

子代理执行后自动保存独立 session（`session_type = "subagent"`），`/sessions` 显示 `[sub]` 标签。

### Prompt 升级

重写 PRINCIPLES segment 为 **Task Decomposition Protocol**（Step 0-3 决策树），明确 Trivial/Simple/Complex 分类。

---

## Phase 41: Skill 系统升级

### 之前 → 现在

| 维度 | 之前 | 现在 |
|------|------|------|
| 触发方式 | 纯手动 skill_load | **自动触发**：每轮匹配 triggers/read_when |
| 工具 | 2 个 | 4 个（list/load/deactivate/reload） |
| 热加载 | 无 | skill_reload 扫描目录 + 恢复活跃 |
| Prompt 集成 | 无 | Segment 6 自动注入 catalog |
| 格式 | name/description/triggers | +version/read_when/requires/provides_tools/priority |

### 新的 SKILL.md 格式

```yaml
---
name: rust-refactoring
description: Rust refactoring patterns
version: "2.0"
triggers: [rust, refactor, ownership]
priority: 10
---
# Content...
```

---

## Phase 42: MCP Client

### 之前是纯 Stub（什么都不做）

```rust
pub async fn call_tool(&self, name: &str, _args: Value) -> Result<String> {
    Ok(format!("[MCP] Called tool '{}': (stub)", name))
}
```

### 现在是完整实现

```
config.toml → McpClientManager
  ├─ StdioTransport::spawn("npx", ["-y", "@modelcontextprotocol/server-filesystem"])
  ├─ → initialize (JSON-RPC 2.0)
  ├─ → initialized (notification)
  ├─ → tools/list ← [read_file, write_file, ...]
  └─ McpToolDef × N registered into ToolRegistry as mcp__<server>__<tool>
```

| 文件 | 说明 |
|------|------|
| `mcp/protocol.rs` **新建** | JSON-RPC 2.0 完整类型体系 |
| `mcp/transport.rs` **新建** | StdioTransport 行分隔 JSON 读写 |
| `mcp/tool.rs` **新建** | McpTool 实现 Tool trait |
| `mcp/mod.rs` **重写** | McpClientManager 完整握手 + 工具发现 |

config.toml 配置：

```toml
[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/dir"]
```

---

## Phase 43: SSE MCP Transport

`mcp/sse.rs` **新建** — 支持远程 HTTP MCP 服务器：

```
GET /sse → 发现 POST endpoint → POST /message 发送 JSON-RPC
```

`McpServerConfig` 新增 `transport` / `url` 字段，`Transport` enum 分发 stdio/sse。

---

## Phase 44: Agent 集成测试

`tests/integration.rs` **新建** — 11 个集成测试覆盖 Agent 全生命周期：

| 测试 | 说明 |
|------|------|
| test_agent_builds_with_defaults | 构建验证 |
| test_agent_builder_sets_all_options | 所有选项设置 |
| test_agent_builder_defaults | 默认值验证 |
| test_agent_with_permission_policy | 权限策略集成 |
| test_agent_with_skill_manager | Skill 集成 |
| test_agent_abort_flag | Abort 信号 |
| test_agent_context_initialized | 上下文初始化 |
| test_clears_context | 清空上下文 |
| test_agent_tool_registry | 工具注册 |
| test_agent_from_config_file | 加载真实 config.toml |
| test_permission_policy_modes | 四种权限模式 |

AgentBuilder 新增 `with_config()` / `with_permission_policy()`。

---

## Phase 45: 文档和示例

- `README.md` — 架构图、特性表、配置模板、CLI 命令完整参考
- `examples/README.md` — 快速开始、Skill/MCP/Session 配置示例

---

## CLI 全面补齐

### 新增命令

```
/mcp                      ← MCP 服务器 + 工具列表
/memory search <query>    ← 搜索对话记忆
/memory stats             ← 记忆统计
/perm mode <mode>         ← 运行时切换权限模式
/skill deactivate <name>  ← 停用 skill
/skill reload             ← 热重载 skill 目录
/skill active             ← 显示活跃 skill
/context                  ← 消息历史 + KV cache + token
```

### 修复/增强

| 问题 | 修复 |
|------|------|
| MCP 从未连接 | 启动时自动 connect_all() |
| /quit 不保存 | 退出时自动保存 session |
| /skill 只能 load | 新增 deactivate/reload/active |
| /perm 只能 test | 新增 mode 子命令 |
| /memory 只能看 core | 新增 search/stats |
| /abort 无法流中打断 | tokio::select! 并发监听 stdin |
| /help 缺高级用法 | 补全所有章节 |

### 完整命令矩阵

```
常规:    /help /status /quit /exit
模型:    /models /model /temp /max-tokens /tokens
上下文:  /context /clear
控制:    /abort /state /tool-mode /steer /follow-up
记忆:    /memory /memory search /memory stats
权限:    /permission /perm test /perm mode /hooks
MCP:     /mcp
任务:    /todo /tasks
会话:    /sessions /session save/resume/delete/rename/archive/search
技能:    /skills /skill <name> /skill active/deactivate/reload
```

---

## 统计

| 指标 | 数值 |
|------|------|
| 完成 Phase | 30 ~ 45（实际 12 个） |
| 单元测试 | 182 |
| 集成测试 | 11 |
| **总计** | **193** |
| cargo check 警告 | 0 |
| cargo check 错误 | 0 |
| 新建文件 | ~20 |
| 重写文件 | ~15 |
