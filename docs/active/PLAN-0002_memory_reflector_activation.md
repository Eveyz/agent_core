# PLAN-0002: Memory & Reflector 激活计划

```yaml
---
id: PLAN-0002
type: PLAN
title: Memory & Reflector 激活计划
status: Draft
author: zniverse
created: 2026-06-25
updated: 2026-06-25
reviewers: []
related: [ADR-0001, PLAN-0001]
supersedes: ~
superseded_by: ~
tags: [memory, reflector, trace, activation]
---
```

## Objective

将已有但完全休眠的 Memory 系统（~2000 行）和 Reflector 系统（~1000 行）真正激活，接入 Tauri 应用的 Runtime 路径。

## Background

### 现状诊断

**Memory 系统：**
- 7 个模块（core/recall/archival/salience/consolidation/embedding/storage）+ 3 个 tool 文件，完整实现
- `with_memory(true)` **从未被任何代码调用**
- CLI TUI 硬编码 `.with_memory(false)`
- Tauri 应用的 Brain 只在 `~/.agverse/config.toml` 有 `[memory]` 段时才启用——默认不启用
- `maybe_consolidate`（去重压缩）**只存在于 Legacy Agent 路径**，Runtime/Run 路径完全没有
- 8 个 memory tools（core_memory_*, conversation_search*, archival_memory_*）注册逻辑正确但条件永远不满足

**Reflector 系统：**
- 3 个模块（mod/digester/suggestion）+ TraceCollector，完整实现
- `ComprehensiveAgentBuilder` 从未在生产代码中使用
- `with_trace()` 仅在 1 个测试中被调用
- `with_recovery()` 从未被调用
- 两条 trace 管线格式不兼容：
  - `TraceCollector` 写 `{"ts":"...","event":<AgentEvent>}`
  - `EventLog` 写 `{"seq":N,"event_id":"...","run_id":"...","event":<RunEvent>}`
- `Reflector::load_trace()` 只能解析前者，吃不了后者的数据
- Digester 有 3 条规则（连续 tool 报错、tool 循环、频繁权限拒绝），检测有意义但 skill body 是模板

### 为什么现在做

- Memory 是 Agent 保持跨会话上下文的核心能力，不用等于没有长期记忆
- Reflector 是自我改进机制的基础，不用等于不会从错误中学习
- 代码都写好了，缺的只是接线——这是投入产出比最高的改进

## Scope

### In Scope

- Memory 在 Runtime 路径（Brain/Run）默认启用
- `maybe_consolidate` 补到 Run 路径
- Memory tools 默认注册到所有 Run
- Reflector 适配 EventLog 格式，能读 Run 的事件日志
- Reflector 在 Run 结束后自动运行（可选开关）
- Tauri bridge 无需改动（memory 走 tool 通道，reflector 走后台任务）
- 前端无改动（memory 对前端透明，reflector 对前端不可见）

### Out of Scope

- Memory 持久化 UI（前端不展示 memory 内容，后续可加）
- Reflector suggestion 的前端展示（后续可加 approval 流程）
- Digester 规则增强（现有 3 条规则够用，后续迭代）
- Salience scorer 的参数调优（先跑起来再说）
- Legacy Agent 路径的同步（CLI 路径已有，不优先）

## Design

### Part A: Memory 激活

#### A1. Brain 默认启用 Memory

**文件:** `core/src/runtime/brain.rs`

当前 `build_memory` 只在 `config.memory` 为 `Some` 时返回 `Some`。改为：即使 config 没有 `[memory]` 段，也用默认值启用。

```rust
fn build_memory(config: &Config) -> Result<Option<Arc<Mutex<MemoryManager>>>> {
    let mem_config = config.memory.as_ref();
    let db_path = mem_config
        .map(|m| m.db_path.as_str())
        .unwrap_or("~/.agent_core/memory.db");
    let embedding_model = mem_config
        .map(|m| m.embedding_model.as_str())
        .unwrap_or("BAAI/bge-small-en-v1.5");
    let block_max_chars = mem_config
        .map(|m| m.default_block_max_chars)
        .unwrap_or(2000);

    let m = MemoryManager::new(db_path, embedding_model, block_max_chars)?;
    Ok(Some(Arc::new(Mutex::new(m))))
}
```

`Brain::from_config` 改为始终调用 `build_memory`，`memory` 字段始终为 `Some`。

#### A2. Run 路径补 consolidation

**文件:** `core/src/runtime/run.rs`

在 `run_turn` 中，assistant 给出最终答案后（`TurnOutcome::Final` 分支），加 consolidation：

```rust
// Store in memory
if let Some(ref mem) = self.brain.memory {
    let m = mem.lock();
    let _ = m.store_conversation("assistant", &text);
}

// Consolidate (spawn background, non-blocking)
if let Some(ref mem) = self.brain.memory.clone() {
    tokio::spawn(async move {
        let result = mem.lock().consolidate();
        if let Ok(report) = result {
            if report.deduped_recall > 0 || report.deduped_archival > 0 {
                tracing::info!(
                    deduped_recall = report.deduped_recall,
                    deduped_archival = report.deduped_archival,
                    "memory consolidated"
                );
            }
        }
    });
}
```

用 `self.join_set.spawn(...)` 而非裸 `tokio::spawn`，确保 cancel 时能 abort。

#### A3. Segment 5 (Active Memory) 每轮刷新

已有（TODO T1-T8 已完成时加的），确认 `refresh_context_segments` 中有：

```rust
// Segment 5: ACTIVE MEMORY
if let Some(ref mem) = self.brain.memory {
    let m = mem.lock();
    let core_str = m.core().to_context_string();
    if !core_str.is_empty() {
        self.context.set_active_memory(&core_str);
    }
}
```

#### A4. CLI 路径同步

**文件:** `cli/src/main.rs`

TUI 模式 line 258: `.with_memory(false)` → `.with_memory(true)`

非 TUI 模式: 去掉交互式询问，默认 `true`。

### Part B: Reflector 激活

#### B1. Reflector 适配 EventLog 格式

**文件:** `core/src/reflector/mod.rs`

当前 `load_trace` 期望 `TraceRecord { ts, event: AgentEvent }`。Runtime 的 `EventLog` 写的是 `Envelope { seq, event_id, run_id, turn_id, event: RunEvent }`。

方案：新增一个 `load_event_log` 方法，直接读 `Envelope` JSONL，将 `RunEvent` 转成 Digester 能消费的内部表示。

```rust
/// Load a Runtime EventLog (Envelope JSONL) and convert to digester input.
pub fn load_event_log(path: &Path) -> Result<Vec<TraceRecord>> {
    let content = std::fs::read_to_string(path)?;
    let mut records = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() { continue; }
        let env: serde_json::Value = serde_json::from_str(line)?;
        // Envelope has: seq, event_id, run_id, turn_id, event (flattened RunEvent)
        // Convert to TraceRecord format the digester expects
        let event_type = env.get("event").and_then(|v| v.as_str()).unwrap_or("");
        // Map RunEvent variants to the AgentEvent-shaped data the digester checks
        let record = convert_envelope_to_trace_record(&env, event_type);
        if let Some(r) = record {
            records.push(r);
        }
    }
    Ok(records)
}
```

#### B2. Digester 适配 RunEvent

**文件:** `core/src/reflector/digester.rs`

Digester 当前检查 `AgentEvent::ToolExecutionEnd` 和 `AgentEvent::Error`。需要适配 `RunEvent::ToolEnded` 和 `RunEvent::Error`。

方案：让 Digester 接受一个统一的中间表示，而不是直接耦合 AgentEvent 或 RunEvent：

```rust
/// Normalized event the digester operates on.
/// Decoupled from both AgentEvent and RunEvent so it works with either trace source.
#[derive(Debug, Clone)]
pub struct DigestEvent {
    pub kind: DigestEventKind,
    pub tool_name: Option<String>,
    pub is_error: bool,
    pub message: Option<String>,
    pub turn_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum DigestEventKind {
    TurnStart,
    TurnEnd,
    ToolEnd,
    Error,
}
```

`Digester::analyze` 改为接受 `&[DigestEvent]`。`load_trace` 和 `load_event_log` 各自负责转换到这个中间表示。

#### B3. Run 结束后自动触发 Reflector

**文件:** `core/src/runtime/brain.rs` + `core/src/runtime/manager.rs`

Brain 持有 `Reflector`：

```rust
pub struct Brain {
    pub config: Config,
    pub memory: Option<Arc<Mutex<MemoryManager>>>,
    pub skill_manager: Option<Arc<Mutex<SkillManager>>>,
    pub todo_list: Arc<Mutex<TodoList>>,
    pub reflector: Option<Reflector>,    // ← 新增
    current_model_name: String,
}
```

`from_config` 中根据 config 开关决定是否创建 Reflector：

```rust
let reflector = if config.reflector_enabled.unwrap_or(false) {
    let skills_dir = dirs::home_dir()?.join(".agent_core/skills");
    Some(Reflector::new(skills_dir))
} else {
    None
};
```

`RunManager` 在 Run 完成后（`RunEvent::RunCompleted`），如果有 Reflector，spawn 一个后台任务：
1. 读取该 Run 的 EventLog JSONL
2. 调用 `Reflector::load_event_log`
3. 调用 `Reflector::analyze`
4. 对 `AppendSkill` 类型的 suggestion 自动 apply
5. 其他 suggestion 记日志

#### B4. Config 开关

**文件:** `core/src/config.rs`

`Config` 增加：

```rust
#[serde(default)]
pub reflector_enabled: Option<bool>,
```

`config.toml` 示例：
```toml
reflector_enabled = true
```

默认 `false`，需要显式开启。

## Tasks

| ID | Task | 涉及文件 | Status |
|----|------|---------|--------|
| M1 | Brain 默认启用 memory（即使 config 无 `[memory]` 段） | `core/src/runtime/brain.rs` | Todo |
| M2 | Run 路径补 consolidation + 用 join_set | `core/src/runtime/run.rs` | Todo |
| M3 | CLI 路径默认启用 memory | `cli/src/main.rs` | Todo |
| M4 | cargo check + cargo test 验证 memory | — | Todo |
| R1 | Digester 引入 `DigestEvent` 中间表示 | `core/src/reflector/digester.rs` | Todo |
| R2 | Reflector 新增 `load_event_log` 适配 EventLog 格式 | `core/src/reflector/mod.rs` | Todo |
| R3 | Brain 持有 Reflector + config 开关 | `core/src/runtime/brain.rs`, `core/src/config.rs` | Todo |
| R4 | RunManager 在 Run 结束后自动触发 Reflector | `core/src/runtime/manager.rs` | Todo |
| R5 | cargo check + cargo test 验证 reflector | — | Todo |

## Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| Memory 默认启用导致首次启动慢（embedding 模型加载） | Med | Med | fastembed 模型懒加载，首次 store 时才初始化 |
| Memory SQLite 文件权限问题 | Low | Low | 用 `~/.agent_core/memory.db`，目录不存在时自动创建 |
| Reflector 后台任务影响性能 | Low | Low | spawn 在 join_set 上，Run cancel 时 abort |
| Reflector 自动写 SKILL.md 产生垃圾文件 | Med | Med | 限制每个 Run 最多 1 个 suggestion；skill 内容质量后续迭代 |
| Digester 格式适配引入 bug | Low | Med | 保留 `load_trace` 原路径不删，新增 `load_event_log` 并行存在 |

## Success Criteria

- Tauri 应用启动后，Brain.memory 始终为 `Some`
- 对话结束后，`store_conversation` 被调用，SQLite 中有数据
- consolidation 在后台运行，日志可见
- 模型可以在对话中调用 `core_memory_append` / `conversation_search` 等 tool
- 模型在 system prompt 的 Segment 5 中看到 core memory 内容
- `reflector_enabled = true` 时，Run 结束后 Reflector 自动分析 EventLog
- `cargo test` 全部通过
- `cargo check` 无新增 warning

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-25 | zniverse | Created as Draft |
