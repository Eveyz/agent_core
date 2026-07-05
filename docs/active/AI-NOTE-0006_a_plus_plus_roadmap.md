# AI-NOTE-0006: A++ Code Quality Improvement Roadmap

```yaml
---
id: AI-NOTE-0006
type: AI-NOTE
title: A++ Code Quality Improvement Roadmap — Core Crate (Excluding TUI)
status: Draft
author: AI Agent
created: 2026-07-05
updated: 2026-07-05
related: [AI-NOTE-0004, AI-NOTE-0005, PLAN-0005, PLAN-0008]
tags: [code-quality, hardening, async, concurrency, testing, architecture, A++]
---
```

## Context

当前 `core` crate 代码质量评级为 **B+**（详见 `agent_core_full_audit.md`）。架构设计卓越（7-segment context engine、5-stage compression pipeline、layered permission system、Brain→RunManager→Run execution model），但在实现层面存在并发安全、async 正确性、错误处理、测试覆盖、代码组织等方面的技术债务。

本文档是**从 B+ 到 A++** 的详细改进路线图，**仅覆盖 `core` crate**（不含 `cli/` TUI 部分和 `app/src-tauri/` 桌面应用部分）。每一项改进均包含：

- **问题描述** — 当前状态与风险
- **目标状态** — A++ 标准下的期望
- **具体方案** — 可执行的修改建议（含代码示例）
- **影响范围** — 涉及的文件与模块
- **预估工作量** — S/M/L
- **验收标准** — 如何确认改进完成

---

## 改进总览

| 阶段 | 主题 | 改进项数 | 目标评级 |
|------|------|---------|---------|
| Phase 0 | 关键正确性 Bug 修复 | 5 | B+ → A- |
| Phase 1 | 并发安全与 Async 正确性 | 6 | A- → A |
| Phase 2 | 错误处理与健壮性 | 5 | A → A+ |
| Phase 3 | 测试覆盖与质量保证 | 4 | A+ → A+ |
| Phase 4 | 架构与代码组织优化 | 5 | A+ → A++ |
| Phase 5 | 性能优化与资源管理 | 4 | A++ 巩固 |

---

## Phase 0: 关键正确性 Bug 修复（P0 — 阻塞级）

### 0.1 修复 subagent 工具的进程全局 CWD 竞态

**问题：** `tools/subagent.rs:652-657` 使用 `std::env::set_current_dir()` 修改进程全局 CWD。如果两个 subagent 并发执行（通过 `SubagentSpawnAllTool` 的并行 spawn），它们会竞态修改同一个进程级 CWD，导致文件操作在错误的目录下执行。

**当前代码：**
```rust
// tools/subagent.rs:652-657
fn set_cwd_guard(target: &Path) -> CwdGuard {
    let orig = std::env::current_dir().unwrap_or_default();
    std::env::set_current_dir(target).ok();  // ← 进程全局！
    CwdGuard { orig }
}
```

**目标状态：** 不修改进程全局 CWD，通过参数传递工作目录。

**具体方案：**

1. 在 `SubagentConfig` 中添加 `working_dir: PathBuf` 字段
2. 在 `Subagent` 执行时将 `working_dir` 传递给内部 Agent 的 `Run`
3. `Run` 已经有 `working_dir` 字段（用于 bash 工具的 `default_working_dir`），扩展到影响所有需要 CWD 的操作
4. 删除 `set_cwd_guard` / `CwdGuard` 相关代码

```rust
// 修改后的 SubagentConfig
pub struct SubagentConfig {
    // ... existing fields ...
    pub working_dir: PathBuf,  // 新增
}

// spawn_single 中不再 set_current_dir
let subagent = Subagent::new(config.with_working_dir(workspace_root.clone()));
// 内部 Agent/Run 使用 working_dir 而非 env CWD
```

**影响范围：** `tools/subagent.rs`, `subagent/mod.rs`, `runtime/run.rs`（确保 `working_dir` 传递到所有文件操作）
**工作量：** M
**验收标准：** 两个 subagent 并发执行，各自在不同的 worktree 中运行 `bash` 命令 `pwd`，结果正确指向各自的工作目录

---

### 0.2 修复 Brain 中 `std::sync::Mutex` 的 poison panic 风险

**问题：** `runtime/brain.rs:292,298` 使用 `std::sync::Mutex` 并在 `.lock().unwrap()` 上调用。如果任何代码在持有该锁时 panic，后续所有锁获取都会 panic（poison error），导致 agent 彻底崩溃。其余代码库统一使用 `parking_lot::Mutex`（无 poison 机制），这里是不一致的。

**当前代码：**
```rust
// runtime/brain.rs
current_mode: std::sync::Mutex<AgentMode>,

// line 292
fn current_mode(&self) -> AgentMode {
    *self.current_mode.lock().unwrap()  // ← poison panic
}

// line 298
fn set_mode(&self, mode: AgentMode) {
    *self.current_mode.lock().unwrap() = mode  // ← poison panic
}
```

**目标状态：** 统一使用 `parking_lot::Mutex`，消除 poison 风险。

**具体方案：**
```rust
// brain.rs
use parking_lot::Mutex;

pub struct Brain {
    // ...
    current_mode: Mutex<AgentMode>,  // parking_lot::Mutex
}

fn current_mode(&self) -> AgentMode {
    *self.current_mode.lock()  // 无 .unwrap()，无 poison
}

fn set_mode(&self, mode: AgentMode) {
    *self.current_mode.lock() = mode
}
```

**影响范围：** `runtime/brain.rs`
**工作量：** S
**验收标准：** `cargo build` 通过，`grep -rn "std::sync::Mutex" core/src/runtime/` 无结果

---

### 0.3 修复 `audit.rs:151` 的 UTF-8 字符边界 panic

**问题：** `permission/audit.rs:151` 的 `truncate()` 函数使用 `&s[..max_len]` 切片字符串。如果 `max_len`（500）不在 UTF-8 字符边界上，会直接 panic。对于包含多字节字符的审计日志内容（如中文命令参数），这是一个确定性 panic。

**当前代码：**
```rust
// permission/audit.rs:151
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    format!("{}...<truncated>", &s[..max_len])  // ← panic on non-char-boundary
}
```

**目标状态：** 安全的字符边界截断。

**具体方案：**
```rust
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let safe_end = floor_char_boundary(s, max_len);
    format!("{}...<truncated>", &s[..safe_end])
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}
```

> 注意：`floor_char_boundary` 在 `read_file.rs:22` 和 `subagent.rs:519` 中已有重复实现。Phase 4 会将其提取到共享 util 模块。

**影响范围：** `permission/audit.rs`
**工作量：** S
**验收标准：** 包含中文字符的审计日志写入不 panic；新增单元测试覆盖多字节字符截断

---

### 0.4 修复 Whitelist `Once` 作用域条目永远无法生效的 bug

**问题：** `permission/whitelist.rs:65` 的 `query()` 方法在查询前调用 `purge_expired()`，而 `purge_expired()` 会无条件移除所有 `Once` 作用域的条目（`whitelist.rs:94-111`）。这意味着 `Once` 条目在被查询到之前就被清除了——这个功能完全不可用。

**当前流程：**
```
query() → purge_expired() [移除 Once 条目] → 匹配 [Once 条目已不存在] → 永远 miss
```

**目标状态：** `Once` 条目允许匹配一次，匹配后移除。

**具体方案：**

1. `purge_expired` 不移除 `Once` 条目���它们没有过期时间）
2. `query()` 匹配到 `Once` 条目后，立即从列表中移除
3. `types.rs` 的 `is_valid()` 对 `Once` 返回 `true`

```rust
// whitelist.rs - purge_expired 只移除过期的 Duration 条目
fn purge_expired(&mut self) {
    let now = Utc::now();
    self.entries.retain(|e| match &e.scope {
        ApprovalScope::Once => true,  // 不在这里移除
        ApprovalScope::Session(_) => true,  // 会话结束才移除
        ApprovalScope::Duration(expiry) => expiry > &now,
        ApprovalScope::Persistent => true,
    });
}

// query() 匹配到 Once 条目后移除
pub fn query(&mut self, tool: &str, input: &Value) -> Option<WhitelistEntry> {
    self.purge_expired();
    if let Some(idx) = self.entries.iter().position(|e| /* matches */) {
        let entry = self.entries[idx].clone();
        if matches!(entry.scope, ApprovalScope::Once) {
            self.entries.remove(idx);  // 消费 Once 条目
        }
        Some(entry)
    } else {
        None
    }
}

// types.rs
pub fn is_valid(&self) -> bool {
    match &self.scope {
        ApprovalScope::Once => true,  // ← 修复：返回 true
        ApprovalScope::Session(_) => true,
        ApprovalScope::Duration(expiry) => expiry > &Utc::now(),
        ApprovalScope::Persistent => true,
    }
}
```

**影响范围：** `permission/whitelist.rs`, `permission/types.rs`
**工作量：** M
**验收标准：** 新增测试：添加 `Once` 白名单条目 → 第一次 `check()` 返回 `Allow` → 第二次 `check()` 返回 `Ask`（条目已被消费）

---

### 0.5 修复 `agent/mod.rs:831` 的 model 查找 unwrap

**问题：** `agent/mod.rs:831` 调用 `self.config.get_model(&self.current_model_name).unwrap()`。如果当前使用的模型在运行时从配置中被删除，会直接 panic。

**当前代码：**
```rust
fn current_model_config(&self) -> &ModelConfig {
    self.config.get_model(&self.current_model_name).unwrap()
    // ← panic if model removed at runtime
}
```

**目标状态：** 优雅降级或返回错误。

**具体方案：**
```rust
fn current_model_config(&self) -> anyhow::Result<&ModelConfig> {
    self.config
        .get_model(&self.current_model_name)
        .ok_or_else(|| anyhow::anyhow!(
            "current model '{}' not found in config (may have been removed)",
            self.current_model_name
        ))
}

// 所有调用点改为:
let model_config = self.current_model_config()?;  // 或 .context(...)
```

**影响范围：** `agent/mod.rs`（修改函数签名 + 所有调用点）
**工作量：** S
**验收标准：** 运行时删除当前模型的配置 → 返回错误而非 panic；`grep -n "\.unwrap()" core/src/agent/mod.rs` 不含生产路径 unwrap

---

## Phase 1: 并发安全与 Async 正确性（P1）

### 1.1 消除 async 上下文中的阻塞 `std::fs` I/O

**问题：** 多个工具和 runtime 路径在 `async fn` 中直接使用 `std::fs` 同步 I/O，阻塞 tokio worker 线程。

**受影响位置清单：**

| 文件 | 行号 | 阻塞调用 |
|------|------|---------|
| `runtime/run.rs` | 1325-1370 | `std::fs::read_to_string()` × 4+（refresh_context_segments） |
| `tools/edit.rs` | 49, 65 | `std::fs::read_to_string()`, `std::fs::write()` |
| `tools/read_file.rs` | 97, 109, 138 | `std::fs::metadata()`, `std::fs::File::open()`, sync `BufReader` |
| `tools/write_file.rs` | 55, 60 | `std::fs::create_dir_all()`, `std::fs::write()` |
| `tools/grep.rs` | 92, 108 | `std::fs::read_to_string()`, `std::fs::read_dir()` |
| `tools/glob.rs` | 69 | `std::env::current_dir()` + sync `glob::glob()` iterator |
| `tools/subagent.rs` | 704, 710 | `std::fs::read_to_string()`（persona 文件读取） |
| `permission/audit.rs` | 112, 133 | sync file I/O in `record()` called from async path |
| `permission/whitelist.rs` | 143, 218 | `std::fs::read_to_string()`, `std::fs::write()` in `persist_to_config` |
| `tools/webfetch.rs` | 648 | `readability::extractor::extract()` CPU-bound in async |

**目标状态：** 所有 async 函数中的 I/O 使用 `tokio::fs` 或 `spawn_blocking`。

**具体方案（按优先级分两类）：**

**A. 文件 I/O → `tokio::fs`（或 `spawn_blocking`）：**

```rust
// tools/edit.rs - 修改前
async fn execute(&self, args: Value) -> Result<String> {
    let old_content = std::fs::read_to_string(&resolved_path)?;
    // ...
    std::fs::write(&resolved_path, &new_content)?;
}

// 修改后
async fn execute(&self, args: Value) -> Result<String> {
    let old_content = tokio::fs::read_to_string(&resolved_path).await?;
    // ...
    tokio::fs::write(&resolved_path, &new_content).await?;
}
```

**B. CPU-bound 操作 → `spawn_blocking`：**

```rust
// tools/webfetch.rs - readability 提取是 CPU-bound
let extracted = tokio::task::spawn_blocking(move || {
    readability::extractor::extract(...)
}).await?;
```

**C. `runtime/run.rs` refresh_context_segments：**

```rust
// 读取 agverse.md 等文件 → spawn_blocking
let cwd = self.working_dir.clone();
let files = tokio::task::spawn_blocking(move || {
    let mut results = Vec::new();
    for path in &[".agverse.md", "AGVERSE.md", "agverse.md"] {
        if let Ok(content) = std::fs::read_to_string(cwd.join(path)) {
            results.push(content);
        }
    }
    results
}).await.unwrap_or_default();
```

**影响范围：** 上表所有文件
**工作量：** M（机械性修改，但涉及面广）
**验收标准：** `grep -rn "std::fs::" core/src/tools/ core/src/runtime/` 仅出现在 `spawn_blocking` 闭包或 `#[cfg(test)]` 中

---

### 1.2 消除 `webfetch.rs` 中 `std::sync::Mutex` 的 poison 风险

**问题：** `tools/webfetch.rs:142,162` 使用 `std::sync::Mutex` 并 `.lock().unwrap()`，存在 poison panic 风险。

**具体方案：**
```rust
// 修改前
use std::sync::{Arc, Mutex};

// 修改后
use parking_lot::Mutex;
// self.last_access: Arc<Mutex<HashMap<String, Instant>>>

// .lock().unwrap() → .lock()
let mut map = self.last_access.lock();
```

**影响范围：** `tools/webfetch.rs`
**工作量：** S
**验收标准：** `grep -rn "std::sync::Mutex" core/src/` 无结果（或仅出现在注释/测试中）

---

### 1.3 修复 grep 工具递归搜索的符号链接循环

**问题：** `tools/grep.rs` 的 `search_path` 递归函数通过 `read_dir` → `file_type()` 遍历目录，会跟随符号链接。如果存在符号链接循环（如 `a → b → a`），会导致无限递归和栈溢出。

**具体方案：**
```rust
fn search_path(path: &Path, /* ... */) {
    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        // 跳过符号链接，防止循环
        if file_type.is_symlink() {
            continue;
        }

        if file_type.is_dir() {
            // ... 递归
        } else if file_type.is_file() {
            // ... 搜索
        }
    }
}
```

**影响范围：** `tools/grep.rs`
**工作量：** S
**验收标准：** 在包含符号链接循环的目录中执行 grep 不崩溃

---

### 1.4 将 Memory 的 embedding 计算移出锁

**问题：** `runtime/run.rs:760-761` 中，`store_conversation()` 在持有 `mem.lock()` 时执行 embedding 计算（10-50ms），阻塞所有其他内存操作。`search_conversation_precomputed` 已经采用了 pre-computed embedding 模式，但 `store_conversation` 没有。

**具体方案：**

```rust
// runtime/run.rs - 修改前
{
    let m = mem.lock();
    let _ = m.store_conversation(&msg, &response);
    // embedding 在锁内计算
}

// 修改后：先在锁外计算 embedding，再在锁内存储
let embedding = {
    let m = mem.lock();
    m.embed_single(&content)  // 或者直接调 client.embed
};  // 锁释放

{
    let m = mem.lock();
    let _ = m.store_conversation_precomputed(&msg, &response, embedding.as_ref());
}  // 锁只持有很短时间
```

需要在 `MemoryManager` 中添加 `store_conversation_precomputed` 方法，接受可选的预计算 embedding。

**影响范围：** `runtime/run.rs`, `memory/mod.rs`
**工作量：** M
**验收标准：** `store_conversation` 路径不再在锁内调用 `embed_single`；并发内存操作延迟降低

---

### 1.5 修复 `permission/mod.rs` 中 `is_readonly_command` 不调用 `normalize_command` 的不一致

**问题：** `is_destructive_command` 调用了 `normalize_command`（处理 `${IFS}` 等转义），但 `is_readonly_command` 没有。这意味着 `ls${IFS}-la` 不会被识别为 readonly 命令（虽然安全，但与 destructive 检查不一致）。

**具体方案：**
```rust
pub fn is_readonly_command(cmd: &str) -> bool {
    let normalized = normalize_command(cmd);  // ← 添加
    let lower = normalized.to_lowercase();
    // ... 后续逻辑使用 lower
}
```

**影响范围：** `permission/mod.rs`
**工作量：** S
**验收标准：** `ls${IFS}-la` 被正确识别为 readonly；新增测试覆盖 `${IFS}` 转义场景

---

### 1.6 修复 `permission/mod.rs:910` 的 `..` 误判

**问题：** `is_destructive_command` 中 `arg.contains("..")` 会匹配任何包含 `..` 的参数，包括合法文件名如 `file..txt` 或版本号 `v2..0`。

**具体方案：**
```rust
// 修改前
if arg.contains("..") {
    return true;  // 误判
}

// 修改后：只匹配路径遍历模式
if arg.contains("/../") || arg == ".." || arg.starts_with("../") || arg.ends_with("/..") {
    return true;
}
```

**影响范围：** `permission/mod.rs`
**工作量：** S
**验收标准：** `cat file..txt` 不被误判为 destructive；`cat ../../etc/passwd` 仍被正确检测

---

## Phase 2: 错误处理与健壮性（P1-P2）

### 2.1 消除生产路径中的所有 `eprintln!`，替换为 `tracing`

**问题：** 代码中存在 20+ 处 `eprintln!` 调用，用于调试输出。这些输出无法控制级别、无法结构化、在热路径上产生 I/O 开销。

**完整清单：**

| 文件 | 行号 | 内容 | 替换为 |
|------|------|------|--------|
| `runtime/run.rs` | 651, 665, 674 | resolve_approval 调试 | `tracing::debug!` |
| `agent/executor.rs` | 71, 91, 124, 251 | tool orchestrator 调试 | `tracing::debug!` |
| `hooks/mod.rs` | 215-241 | LoggingHook 全事件 | `tracing::info!` / `debug!` |
| `mcp/mod.rs` | 188, 192, 323 | MCP 生命周期日志 | `tracing::info!` |
| `permission/whitelist.rs` | 40 | 过期清理 | `tracing::warn!` |
| `permission/audit.rs` | 59, 65 | 审计写入失败 | `tracing::error!` |

**具体方案：**
```rust
// 修改前
eprintln!("[permission] pruned {} expired whitelist entries", expired);

// 修改后
tracing::warn!(expired_count = expired, "pruned expired whitelist entries");
```

**前提条件：** 确保 `core/Cargo.toml` 已包含 `tracing` 依赖，并在 `lib.rs` 或入口处初始化 subscriber。

**影响范围：** 上表所有文件
**工作量：** S（机械性替换）
**验收标准：** `grep -rn "eprintln!" core/src/ | grep -v "#\[cfg(test)\]"` 无结果

---

### 2.2 修复 `edit` 工具的非原子写入

**问题：** `tools/edit.rs:65` 直接使用 `std::fs::write()` 写入目标文件。如果写入过程中断（panic、磁盘满、进程被 kill），文件可能被截断或损坏，原始内容丢失。

**具体方案：**
```rust
// 写入临时文件，然后原子重命名
let tmp_path = resolved_path.with_extension("tmp.edit");
tokio::fs::write(&tmp_path, &new_content).await?;
tokio::fs::rename(&tmp_path, &resolved_path).await?;
// rename 在同一文件系统上是原子的
```

**影响范围：** `tools/edit.rs`
**工作量：** S
**验收标准：** 模拟写入中断后原文件内容完整；新增测试验证临时文件不残留

---

### 2.3 修复 grep 工具无效 glob 过滤器静默回退为 `"*"`

**问题：** `tools/grep.rs:84` 中，如果用户提供的 `include` glob 模式无效，会静默回退到 `"*"`（匹配所有文件），这完全违背了过滤器的意图。

**当前代码：**
```rust
let pattern = glob::Pattern::new(ext_filter)
    .unwrap_or(glob::Pattern::new("*").unwrap());  // ← 静默回退到匹配所有
```

**具体方案：**
```rust
let pattern = glob::Pattern::new(ext_filter)
    .with_context(|| format!("invalid include pattern: {ext_filter}"))?;
```

**影响范围：** `tools/grep.rs`
**工作量：** S
**验收标准：** 无效 glob 模式返回错误而非匹配所有文件

---

### 2.4 消除 bash 工具中被静默忽略的 I/O 错误

**问题：** `tools/bash.rs` 多处使用 `let _ =` 静默忽略读取错误，导致输出丢失且无法诊断。

**受影响位置：**
- `bash.rs:178` — `stdout.read_to_end()` 错误被忽略
- `bash.rs:190` — `stderr.read_to_end()` 错误被忽略
- `bash.rs:170, 263` — `next_line()` 错误静默终止循环
- `bash.rs:221` — `supervisor.kill()` 失败被忽略

**具体方案：**
```rust
// 修改前
while let Ok(Some(line)) = lines.next_line().await {
    // ...
}

// 修改后
loop {
    match lines.next_line().await {
        Ok(Some(line)) => { /* ... */ }
        Ok(None) => break,
        Err(e) => {
            tracing::warn!(error = %e, "stdout read error, partial output captured");
            break;
        }
    }
}
```

对于 cleanup 路径的 `let _ =`：
```rust
if let Err(e) = supervisor.kill(&child_id) {
    tracing::warn!(error = %e, "failed to kill child process during cleanup");
}
```

**影响范围：** `tools/bash.rs`
**工作量：** S
**验收标准：** 所有 `let _ =` I/O 调用替换为带日志的错误处理

---

### 2.5 修复 `permission/whitelist.rs` 的 TOML 操作损坏风险

**问题：** `persist_to_config()`（whitelist.rs:137-220）使用逐行字符串操作来添加/删除 `[[permissions.whitelist]]` TOML 块。这种方式极其脆弱：
- 多行数组值会被错误截断
- 字符串中的 `"` 和 `\` 不会被转义，导致 TOML 格式错误
- 空行在条目中间会提前终止块识别

**具体方案：** 使用 `toml_edit` crate（保留注释和格式的 TOML 编辑器）替代手动字符串操作：

```rust
use toml_edit::Document;

fn persist_to_config(config_path: &Path, entry: &WhitelistEntry, action: PersistAction) -> Result<()> {
    let content = std::fs::read_to_string(config_path)?;
    let mut doc: Document = content.parse()
        .context("failed to parse config.toml")?;

    let permissions = doc["permissions"]
        .or_insert(toml_edit::table());

    match action {
        PersistAction::Add => {
            let whitelist = permissions["whitelist"]
                .or_insert(toml_edit::array_of_tables());
            let mut item = toml_edit::Table::new();
            item["tool_pattern"] = toml_edit::value(&entry.pattern.tool_pattern);
            // ... 其他字段
            whitelist.as_array_of_tables_mut().unwrap().push(item);
        }
        PersistAction::Remove => {
            // 按条件过滤移除
        }
    }

    std::fs::write(config_path, doc.to_string())?;
    Ok(())
}
```

**影响范围：** `permission/whitelist.rs`, `core/Cargo.toml`（添加 `toml_edit` 依赖）
**工作量：** M
**验收标准：**
- 包含多行数组值的 config.toml 在持久化后格式完整
- 包含特殊字符的 tool_pattern 正确转义
- 新增单元测试覆盖各种 TOML 边界情况

---

## Phase 3: 测试覆盖与质量保证（P2）

### 3.1 为 `Run` 执行循环添加单元测试

**问题：** `runtime/run.rs`（1726 行）是整个系统最核心的文件，但完全没有单元测试，仅通过 Agent 集成测试间接覆盖。turn loop、命令轮询、context 刷新、compaction 触发等关键路径未被直接测试。

**目标状态：** Run 的核心逻辑有独立单元测试覆盖。

**具体方案：** 提取可测试的子组件并添加测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_completes_on_no_tool_calls() {
        // 配置 mock client 返回无 tool_call 的响应
        // 验证 Run 正确进入 Completed 状态
    }

    #[tokio::test]
    async fn test_run_cancellation_mid_turn() {
        // 在 turn 执行中触发 CancellationToken
        // 验证 Run 正确进入 Cancelled 状态
    }

    #[tokio::test]
    async fn test_context_compaction_triggers_at_threshold() {
        // 填充消息直到超过 auto_compact_threshold
        // 验证 chunked_drop 被触发
    }

    #[tokio::test]
    async fn test_steering_message_injected_at_turn_boundary() {
        // 发送 Steer 命令
        // 验证 steer_entries 在下一个 turn 前被注入
    }

    #[tokio::test]
    async fn test_event_guard_emits_tool_ended_on_drop() {
        // 模拟 tool 执行中途 panic
        // 验证 EventGuard drop 时发出 ToolEnded { is_error: true }
    }
}
```

**影响范围：** `runtime/run.rs`（可能需要提取部分逻辑为可测试函数）
**工作量：** L
**验收标准：** `runtime/run.rs` 测试覆盖率 > 40%（行覆盖）

---

### 3.2 为 `MemoryManager` 添加单元测试

**问题：** `memory/mod.rs`（741+ 行）是关键系统，但完全没有单元测试。BM25、HNSW、RRF 融合、salience 评分、consolidation 等核心逻辑未被直接测试。

**具体方案：**

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_store_and_recall_conversation() {
        // 存储一条对话 → recall 搜索能找到
    }

    #[test]
    fn test_bm25_keyword_retrieval() {
        // 存储多条 → BM25 关键词搜索返回相关结果
    }

    #[test]
    fn test_rrf_fusion_merges_bm25_and_hnsw() {
        // 分别 mock BM25 和 HNSW 结果 → 验证 RRF 融合顺序
    }

    #[test]
    fn test_salience_recency_decay() {
        // 存储不同时间点的记忆 → 验证 recency decay 权重
    }

    #[test]
    fn test_consolidation_dedup() {
        // 存储相似内容 → 触发 consolidation → 验证去重
    }

    #[test]
    fn test_try_lock_memory_timeout() {
        // 持有锁 → try_lock_memory 返回 busy message
    }
}
```

**影响范围：** `memory/mod.rs`
**工作量：** L
**验收标准：** memory 模块测试覆盖率 > 50%

---

### 3.3 为安全关键工具添加单元测试

**问题：** `bash`、`edit`、`write_file`、`grep`、`webfetch`、`subagent` 工具完全没有单元测试。

**优先级排序：**

| 工具 | 优先级 | 关键测试场景 |
|------|--------|------------|
| `bash` | P0 | timeout 行为、exit code 传播、stderr 捕获、进程组 kill |
| `edit` | P1 | 基本替换、多匹配报错、未找到报错、diff 输出格式 |
| `write_file` | P1 | 基本写入、父目录创建、路径重定向 |
| `grep` | P1 | 模式匹配、include 过滤、目录递归、隐藏目录跳过 |
| `webfetch` | P2 | HTTPS 升级、meta 提取、rate limiter、内容截断 |
| `subagent` | P2 | result_strategy 解析、tool summary 构建、workspace root 查找 |

**具体方案（以 bash 为例）：**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bash_echo() {
        let tool = BashTool::new();
        let result = tool.execute(json!({"command": "echo hello"})).await.unwrap();
        assert!(result.contains("hello"));
    }

    #[tokio::test]
    async fn test_bash_timeout() {
        let tool = BashTool::new();
        let result = tool.execute(json!({
            "command": "sleep 10",
            "timeout_secs": 1
        })).await;
        assert!(result.unwrap().contains("timed out") || result.is_err());
    }

    #[tokio::test]
    async fn test_bash_exit_code() {
        let tool = BashTool::new();
        let result = tool.execute(json!({"command": "exit 42"})).await.unwrap();
        assert!(result.contains("42"));
    }
}
```

**影响范围：** `tools/bash.rs`, `tools/edit.rs`, `tools/write_file.rs`, `tools/grep.rs`, `tools/webfetch.rs`, `tools/subagent.rs`
**工作量：** L
**验收标准：** 每个工具至少 5 个单元测试

---

### 3.4 为 `permission/whitelist.rs` 的 `persist_to_config` 添加测试

**问题：** `persist_to_config`（80+ 行）包含复杂的 TOML 字符串操作逻辑，完全没有测试。这是安全相关代码——如果白名单持久化出错，可能导致权限配置损坏或绕过。

**具体方案：**
```rust
#[test]
fn test_persist_add_to_empty_config() {
    // 空配置 → 添加条目 → 验证 TOML 格式正确
}

#[test]
fn test_persist_add_to_existing_whitelist() {
    // 已有白名单条目 → 添加新条目 → 验证两条都在
}

#[test]
fn test_persist_remove_entry() {
    // 有两条 → 移除一条 → 验证只剩一条
}

#[test]
fn test_persist_preserves_other_sections() {
    // 有 [model], [permissions.rules] 等 → 操作 whitelist → 验证其他段不受影响
}

#[test]
fn test_persist_special_chars_in_pattern() {
    // tool_pattern 包含 " 和 \ → 验证正确转义
}
```

**影响范围：** `permission/whitelist.rs`
**工作量：** M
**验收标准：** `persist_to_config` 所有分支被测试覆盖

---

## Phase 4: 架构与代码组织优化（P2-P3）

### 4.1 拆分 `runtime/run.rs`（1726 行）

**问题：** `Run` 结构体承担了太多职责：turn 循环、命令轮询、context 刷新、工具执行、compaction 策略、审批解析、事件发送。这是单一职责原则的违反。

**目标状态：** 将 `Run` 拆分为多个协作组件。

**具体方案：**

```
runtime/
├── run.rs           # Run 结构体 + 生命周期管理（~400 行）
├── turn_executor.rs # turn 循环 + model_turn + execute_tools（~500 行）
├── context_refresher.rs # refresh_context_segments + segment 构建（~300 行）
├── compact_strategy.rs  # maybe_compact + chunked_drop + force_compact（~300 行）
└── approval_resolver.rs # resolve_approval（已有，可能需要提取）
```

```rust
// run.rs - 只保留核心生命周期
pub struct Run {
    // ... 字段
    turn_executor: TurnExecutor,
    context_refresher: ContextRefresher,
    compact_strategy: CompactStrategy,
}

impl Run {
    pub async fn execute(&mut self) -> Result<()> {
        self.context_refresher.refresh(&mut self.context).await?;
        loop {
            self.turn_executor.run_turn(&mut self.context).await?;
            self.compact_strategy.maybe_compact(&mut self.context)?;
            // ... 命令轮询、状态检查
        }
    }
}
```

**影响范围：** `runtime/run.rs` → 拆分为 4-5 个文件
**工作量：** L
**验收标准：** 每个文件 < 500 行；`cargo test` 全部通过；行为无变化

---

### 4.2 拆分 `permission/mod.rs`（1194 行）

**问题：** `permission/mod.rs` 包含权限策略、命令分析（destructive/readonly 检测）、路径规范化、sandbox 检查等多个独立功能。

**具体方案：**

```
permission/
├── mod.rs           # PermissionPolicy + check() 主逻辑（~400 行）
├── command_analysis.rs # normalize_command, is_destructive_command, is_readonly_command（~300 行）
├── sandbox.rs       # canonicalize_target, sandbox 路径检查（~200 行）
├── rules.rs         # 不变
├── types.rs         # 不变
├── whitelist.rs     # 不变
└── audit.rs         # 不变
```

**影响范围：** `permission/mod.rs` → 拆分
**工作量：** M
**验收标准：** 每个文件 < 500 行；公开 API 不变

---

### 4.3 拆分 `tools/webfetch.rs`（865 行）

**问题：** `webfetch.rs` 混合了 UA 配置、rate limiter、HTTP 客户端、HTML 解析、meta 提取、readability、markdown 转换、错误格式化等多个功能。

**具体方案：**

```
tools/webfetch/
├── mod.rs           # WebFetchTool + execute()（~200 行）
├── ua.rs            # UaProfile, UA_PROFILES, pick_ua_profile（~100 行）
├── rate_limit.rs    # DomainRateLimiter（~80 行）
├── html_parser.rs   # strip_html_tags, extract_meta, extract_readable（~300 行）
└── error_format.rs  # format_http_error, truncate（~100 行）
```

**影响范围：** `tools/webfetch.rs` → 拆分为子模块
**工作量：** M
**验收标准：** 每个文件 < 350 行

---

### 4.4 提取共享 utility 模块，消除代码重复

**问题：** 多个工具函数在 2-3 个文件中重复实现。

| 函数 | 重复位置 |
|------|---------|
| `floor_char_boundary` | `read_file.rs:22`, `subagent.rs:519` |
| `expand_tilde` | `permission/mod.rs:739`, `permission/audit.rs:155` |

**具体方案：**
```rust
// core/src/util.rs（新文件）
pub fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() { return s.len(); }
    while idx > 0 && !s.is_char_boundary(idx) { idx -= 1; }
    idx
}

pub fn expand_tilde(path: &str) -> PathBuf {
    // ...
}
```

在 `lib.rs` 中添加 `pub mod util;`，然后删除各处的重复实现，改为 `use crate::util::floor_char_boundary;`。

**影响范围：** 新建 `core/src/util.rs`，修改 `read_file.rs`, `subagent.rs`, `permission/mod.rs`, `permission/audit.rs`, `lib.rs`
**工作量：** S
**验收标准：** `grep -rn "fn floor_char_boundary" core/src/` 只有一个定义；`grep -rn "fn expand_tilde" core/src/` 只有一个定义

---

### 4.5 清理遗留代码

**问题：** 存在多个已被取代但未删除的遗留代码路径，增加维护负担和混淆。

**遗留代码清单：**

| 代码 | 位置 | 状态 |
|------|------|------|
| `trim_to_fit_legacy` | `context.rs` | 已被 `trim_to_fit` 和 `chunked_drop` 取代 |
| `micro_compact` | `compressor.rs` | 已被 `chunked_drop` 取代（仅作为 last resort） |
| `PromptAssembler` | `prompt.rs` | 已被 `PromptBuilder` 取代 |
| `global_pending_approvals` | `permission/mod.rs:35` | 已被 per-Run `ApprovalResolver` 取代 |
| `default_rules()` (legacy) | `permission/rules.rs:154` | 已被 `default_rules_with_danger()` 取代 |
| `permissive_rules()` (legacy) | `permission/rules.rs` | 同上 |
| `strict_rules()` (legacy) | `permission/rules.rs` | 同上 |
| bash 工具 legacy path | `tools/bash.rs:256-308` | 已被 supervised path 取代 |

**具体方案：**
- 标记为 `#[deprecated]` 并添加 `#[doc(hidden)]`
- 如果确认无外部消费者，直接删除
- 对 bash legacy path：确认 `supervisor` 在所有路径上都为 `Some` 后删除 legacy 分支

**影响范围：** 多个文件
**工作量：** M
**验收标准：** `grep -rn "legacy\|deprecated\|TODO.*remove" core/src/ | grep -v test` 显著减少

---

## Phase 5: 性能优化与资源管理（P2-P3）

### 5.1 缓存 tool catalog 字符串，避免每 turn 重建

**问题：** `runtime/run.rs:1294-1297` 每个 turn 都调用 `set_tool_catalog()`，虽然有内容相等检查跳过 no-op 更新，但 `build_danger_map()` 和 `build_tool_catalog_string()` 仍然每 turn 执行。

**具体方案：**
```rust
// 在 Run 中缓存 catalog 字符串和指纹
struct Run {
    // ...
    tool_catalog_cache: Option<(String, String)>,  // (fingerprint, catalog_string)
}

fn refresh_tool_catalog(&mut self) {
    let fingerprint = self.tools.registry_fingerprint();  // 基于工具名+描述的 hash
    if let Some((cached_fp, _)) = &self.tool_catalog_cache {
        if *cached_fp == fingerprint {
            return;  // 无变化，跳过
        }
    }
    let catalog = self.tools.build_catalog_string();
    self.context.set_tool_catalog(&catalog);
    self.tool_catalog_cache = Some((fingerprint, catalog));
}
```

**影响范围：** `runtime/run.rs`, `tools/mod.rs`（添加 `registry_fingerprint`）
**工作量：** S
**验收标准：** 工具注册不变时，`build_tool_catalog_string` 不被调用

---

### 5.2 添加内存索引大小限制与淘汰机制

**问题：** BM25（tantivy）和 HNSW（instant-distance）索引随对话历史无限增长，长时间运行会 OOM。

**具体方案：**

```rust
pub struct MemoryManager {
    // ...
    max_index_entries: usize,  // 可配置，默认 10000
}

// 当索引超过限制时，淘汰最旧的条目
fn enforce_index_limit(&mut self) {
    if self.conversation_count > self.max_index_entries {
        let to_remove = self.conversation_count - self.max_index_entries;
        // 从 BM25 和 HNSW 中移除最旧的 to_remove 条
        self.bm25_index.evict_oldest(to_remove);
        self.hnsw_index.evict_oldest(to_remove);
        tracing::info!(evicted = to_remove, "evicted old memory entries");
    }
}
```

**影响范围：** `memory/mod.rs`, `memory/bm25.rs`, `memory/hnsw.rs`
**工作量：** M
**验收标准：** 长时间运行（> max_index_entries 条对话）内存使用稳定

---

### 5.3 实现 `RecoveryAction::SwitchModel`

**问题：** `runtime/run.rs:1203-1209` 中，`SwitchModel` recovery action 是一个 no-op，直接返回 `GiveUp`。当 loop 级需要切换模型时（如当前模型持续返回无效输出），无法自动切换。

**当前代码：**
```rust
RecoveryAction::SwitchModel { model } => {
    // Model switching at runtime is complex — for now, just give up
    self.emit(AgentEvent::Error { message: format!("Model switch to {} not implemented", model) });
    return Err(RunError::Failed("model switch not implemented".into()));
}
```

**具体方案：**
```rust
RecoveryAction::SwitchModel { model } => {
    tracing::info!(new_model = %model, "switching model mid-run");
    
    // 从 config 中查找新模型配置
    let model_config = self.brain.config.get_model(&model)
        .ok_or_else(|| RunError::Failed(format!("model '{}' not in config", model)))?;
    
    // 重建 client
    self.client = OpenAIClient::new(model_config.clone(), self.brain.config.clone());
    self.current_model = model.clone();
    
    // 不需要重置 context — conversation history 保留
    self.emit(AgentEvent::ModelSwitched { new_model: model });
    
    // 继续重试
    continue;
}
```

**影响范围：** `runtime/run.rs`
**工作量：** M
**验收标准：** 配置 fallback model → 主模型持续失败 → 自动切换到 fallback model 继续

---

### 5.4 添加事件日志轮转机制

**问题：** 事件日志（JSONL）无限增长，无清理机制。长时间运行会消耗大量磁盘空间。

**具体方案：**
```rust
pub struct EventLog {
    // ...
    max_file_size: u64,  // 默认 100MB
    max_files: usize,    // 默认保留 5 个轮转文件
}

fn rotate_if_needed(&self) {
    if let Ok(metadata) = std::fs::metadata(&self.path) {
        if metadata.len() > self.max_file_size {
            // rename current → .1, .1 → .2, etc.
            for i in (1..self.max_files).rev() {
                let from = self.path.with_extension(format!("jsonl.{}", i));
                let to = self.path.with_extension(format!("jsonl.{}", i + 1));
                let _ = std::fs::rename(&from, &to);
            }
            let _ = std::fs::rename(&self.path, self.path.with_extension("jsonl.1"));
        }
    }
}
```

**影响范围：** `runtime/event_log.rs`
**工作量：** S
**验收标准：** 事件日志超过 max_file_size 时自动轮转；旧文件不超过 max_files 个

---

## 附加改进项

### A.1 统一错误类型（可选，面向库消费者）

**问题：** 代码库统一使用 `anyhow::Result`，这对应用代码是合适的，但作为库（`agent_core` 被其他 crate 引用），调用者无法 match 具体错误类型。

**评估：** 当前 `cli/` 和 `app/` 是唯一消费者，且都是应用层。如果未来作为公开库发布，应考虑为公共 API 添加 `thiserror` 错误类型。

**建议：** 暂不实施，但标记为 future work。当有第三方消费者时再引入。

---

### A.2 使 `system_prefix_budget` 可配置

**问题：** `context.rs` 中 `system_prefix_budget = max_tokens * 0.08`（8%），对于 128K context 仅 ~10K tokens。如果工具目录增长（MCP 工具等），可能被截断。

**具体方案：**
```rust
// config.toml
[context]
system_prefix_budget_ratio = 0.15  # 15%，默认 0.08

// context.rs
let system_prefix_budget = (max_tokens as f64 * config.context.system_prefix_budget_ratio) as usize;
```

**影响范围：** `config.rs`, `context.rs`
**工作量：** S
**验收标准：** 可通过 config.toml 调整 system_prefix_budget

---

### A.3 修复 `permission/types.rs:478` 的 glob-to-regex 转义不完整

**问题：** `glob_match` 将 glob 模式转为 regex 时只转义了 `.`，其他 regex 元字符（`+`, `(`, `)`, `[`, `]`, `{`, `}`, `^`, `$`, `\`）会被当作 regex 解析，导致匹配静默失败。

**具体方案：**
```rust
let regex_pattern: String = pattern.chars().map(|c| match c {
    '*' => ".*".to_string(),
    '?' => ".".to_string(),
    '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '\\' | '|' => format!("\\{c}"),
    _ => c.to_string(),
}).collect();
```

**影响范围：** `permission/types.rs`
**工作量：** S
**验收标准：** 包含 `(` 的 tool_pattern 正确匹配；新增测试覆盖各种 regex 元字符

---

### A.4 修复 `permission/rules.rs:52-70` 的 `starts_with` 前缀匹配问题

**问题：** readonly 命令列表使用 `starts_with` 匹配，`"ls"` 会匹配 `"lsxyz"`，这不是一个合法的 `ls` 命令。

**具体方案：** 使用 word-boundary 匹配：
```rust
fn matches_command(cmd: &str, pattern: &str) -> bool {
    // 确保 pattern 后面是空格或字符串结尾
    cmd == pattern || cmd.starts_with(&format!("{} ", pattern))
}
```

**影响范围：** `permission/rules.rs` 或 `permission/types.rs`（`matches_command` 所在位置）
**工作量：** S
**验收标准：** `lsxyz` 不被匹配为 `ls`；`ls -la` 仍被正确匹配

---

## 实施顺序与依赖关系

```
Phase 0（关键 Bug 修复）
  │
  ├── 0.1 subagent CWD竞态 ──────────────┐
  ├── 0.2 Brain Mutex poison ────────────│
  ├── 0.3 audit.rs UTF-8 panic ──────────│── 无依赖，可并行
  ├── 0.4 Whitelist Once bug ────────────│
  └── 0.5 model lookup unwrap ───────────┘
  │
Phase 1（并发与 Async）
  │
  ├── 1.1 阻塞 I/O（依赖 0.1 的 working_dir 传递）
  ├── 1.2 webfetch Mutex（无依赖）
  ├── 1.3 grep 符号链接（无依赖）
  ├── 1.4 embedding 移出锁（无依赖）
  ├── 1.5 normalize_command 一致性（无依赖）
  └── 1.6 `..` 误判（无依赖）
  │
Phase 2（错误处理）
  │
  ├── 2.1 eprintln! → tracing（无依赖）
  ├── 2.2 edit 原子写入（无依赖）
  ├── 2.3 grep glob 回退（无依赖）
  ├── 2.4 bash 静默错误（无依赖）
  └── 2.5 whitelist TOML（无依赖，但建议在 0.4 之后）
  │
Phase 3（测试覆盖）── 可与 Phase 1/2 并行
  │
  ├── 3.1 Run 测试（建议在 4.1 拆分后进行，或同时进行）
  ├── 3.2 MemoryManager 测试
  ├── 3.3 工具测试
  └── 3.4 whitelist 测试（建议在 2.5 之后）
  │
Phase 4（架构优化）── 建议在 Phase 0-2 完成后
  │
  ├── 4.1 拆分 run.rs（建议与 3.1 同步进行）
  ├── 4.2 拆分 permission/mod.rs
  ├── 4.3 拆分 webfetch.rs
  ├── 4.4 提取 util 模块（无依赖）
  └── 4.5 清理遗留代码（无依赖）
  │
Phase 5（性能优化）── 最后进行
  │
  ├── 5.1 缓存 tool catalog
  ├── 5.2 内存索引淘汰
  ├── 5.3 实现 SwitchModel
  └── 5.4 事件日志轮转
```

---

## A++ 验收标准总览

以下是通过 A++ 评级需要满足的量化标准：

| 维度 | B+（当前） | A- | A | A+ | A++ |
|------|-----------|-----|---|----|----|
| 生产路径 `unwrap()` | ~136 | <50 | <20 | <10 | **0** |
| `eprintln!` | 20+ | 10 | 5 | 1 | **0** |
| async 中的阻塞 I/O | 20+ | 10 | 5 | 1 | **0** |
| `std::sync::Mutex`（生产） | 3 | 1 | 0 | 0 | **0** |
| 关键模块测试覆盖 | ~40% | 50% | 60% | 75% | **>80%** |
| `Run` 测试 | 0 | 3 | 8 | 15 | **>20** |
| `MemoryManager` 测试 | 0 | 3 | 8 | 15 | **>20** |
| 最大文件行数 | 1726 | 1200 | 800 | 600 | **<500** |
| 已知 P0 bug | 5 | 2 | 0 | 0 | **0** |
| 已知 P1 bug | 6 | 4 | 2 | 0 | **0** |

### A++ 的定性标准

1. **零 panic 路径** — 生产代码中不存在任何可能导致 panic 的 `unwrap()`/`expect()`/`unreachable!()`/切片越界
2. **全 async 正确** — 没有 async 函数中的阻塞 I/O，CPU-bound 操作使用 `spawn_blocking`
3. **并发安全** — 不存在进程全局可变状态，所有锁使用 `parking_lot`（无 poison），锁内不做耗时操作
4. **错误可观测** — 所有错误路径有 `tracing` 日志，无 `let _ =` 静默忽略
5. **测试完备** — 关键模块（Run、Memory、Permission、Tools）测试覆盖率 > 80%
6. **代码组织** — 无文件超过 500 行，无重复代码，无遗留废弃代码
7. **资源可控** — 内存索引有淘汰机制，日志有轮转，无无限增长路径
8. **优雅降级** — 所有运行时配置缺失/变更返回错误而非 panic

---

## References

- `agent_core_full_audit.md` — 完整审计报告（B+ 评级依据）
- `docs/ai-notes/remaining-core.md` — core 子系统深度分析
- `docs/ai-notes/memory-analysis.md` — memory 子系统深度分析
- `docs/ai-notes/tool-analysis.md` — tools 子系统深度分析
- `AI-NOTE-0004` — Skill Load Truncation Analysis
- `PLAN-0005` — Prompt Cache Hit Rate Optimization
- `PLAN-0008` — Truncation Architecture Redesign

---

*Generated by AI Agent (agent_core)*
*Model: glm-5.2 | Timestamp: 2026-07-05T13:38:00+08:00*
