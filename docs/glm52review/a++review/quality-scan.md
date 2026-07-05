I'll run a comprehensive code quality scan. Let me start with multiple parallel scans to gather the raw data.
Now let me run the unwrap/expect scans and identify test module line ranges, plus std::fs:: + async fn overlap, and long function detection.
Now let me run the detailed unwrap/expect filtering using Python, and detect long functions.
Let me investigate the reflector test module detection issue and re-run the long function scan with optimization.
I see the issue — there are test modules with non-standard names (e.g., `mod guard_tests`). Let me fix the test detection and re-run.
The awk found wider ranges but has accuracy issues. Let me write a corrected Python script that properly matches `#[cfg(test)]` + any `mod` name.
I found a bug in my test-range detection — when a file has multiple test modules, only the first is detected. Let me fix this and re-run the full scan.
I found a regex bug in the eprintln!/println!/dbg! detection. The `\b` word boundary after `!` fails to match. Let me fix and verify.
All scans are complete and verified. Here is the comprehensive code quality report:

---

# Code Quality Scan Report — `core/src/`

**Scope:** 101 Rust files, 35,984 total lines of code. Test modules (56 detected `#[cfg(test)] mod` blocks) excluded from all counts.

---

## (1) `unwrap()` calls NOT in test modules — **25 total**

| # | Location | Code |
|---|----------|------|
| 1 | `agent/mod.rs:831` | `self.config.get_model(&self.current_model_name).unwrap()` |
| 2 | `bin/test_meta.rs:5` | `Storage::new("...").unwrap()` |
| 3 | `bin/test_meta.rs:9` | `sm.list(true).unwrap()` |
| 4 | `memory/block.rs:96` | `self.blocks.get_mut(id).unwrap().content = new_content` |
| 5 | `memory/block.rs:97` | `self.blocks.get_mut(id).unwrap().updated_at = now` |
| 6 | `memory/block.rs:125` | `self.blocks.get_mut(id).unwrap().content = replaced` |
| 7 | `memory/block.rs:126` | `self.blocks.get_mut(id).unwrap().updated_at = now` |
| 8 | `memory/embedding.rs:34` | `guard.as_mut().unwrap()` |
| 9 | `memory/mod.rs:400` | `self.bm25.as_ref().unwrap()` |
| 10 | `memory/mod.rs:401` | `self.hnsw.as_ref().unwrap()` |
| 11 | `memory/mod.rs:503` | `self.bm25.as_ref().unwrap()` |
| 12 | `memory/mod.rs:504` | `self.hnsw.as_ref().unwrap()` |
| 13 | `runtime/brain.rs:292` | `*self.current_mode.lock().unwrap()` |
| 14 | `runtime/brain.rs:298` | `*self.current_mode.lock().unwrap() = mode` |
| 15 | `session.rs:193` | `let id = session_id.unwrap()` |
| 16 | `session.rs:339` | `v.as_array().unwrap().is_empty()` |
| 17 | `skills/manifest.rs:188` | `line.strip_prefix("- ").unwrap().trim()` |
| 18 | `tools/grep.rs:84` | `glob::Pattern::new(ext_filter).unwrap_or(glob::Pattern::new("*").unwrap())` |
| 19 | `tools/webfetch.rs:142` | `self.last_access.lock().unwrap()` |
| 20 | `tools/webfetch.rs:162` | `self.last_access.lock().unwrap()` |
| 21 | `workflow/planner.rs:56` | `.unwrap()` |
| 22 | `workflow/planner.rs:60` | `.unwrap()` |
| 23 | `worktree/mod.rs:48` | `worktree_path.to_str().unwrap()` |
| 24 | `worktree/mod.rs:60` | `worktree_path.to_str().unwrap()` |
| 25 | `worktree/mod.rs:91` | `record.path.to_str().unwrap()` |

---

## (2) `expect()` calls NOT in test modules — **5 total**

| # | Location | Code |
|---|----------|------|
| 1 | `agent/mod.rs:477` | `.expect("RunManager not initialized")` |
| 2 | `client/mod.rs:68` | `.expect("failed to build http client")` |
| 3 | `memory/hnsw.rs:76` | `.expect("HNSW fallback lock poisoned")` |
| 4 | `memory/hnsw.rs:83` | `.expect("HNSW lock poisoned")` |
| 5 | `memory/hnsw.rs:96` | `.expect("HNSW fallback lock poisoned")` |

---

## (3) `eprintln!`/`println!`/`dbg!` calls NOT in test modules — **29 total**

| # | Location | Macro |
|---|----------|-------|
| 1 | `agent/executor.rs:71` | `eprintln!` |
| 2 | `agent/executor.rs:91` | `eprintln!` |
| 3 | `agent/executor.rs:124` | `eprintln!` |
| 4 | `agent/executor.rs:251` | `eprintln!` |
| 5 | `bin/test_meta.rs:11` | `println!` |
| 6 | `bin/test_meta.rs:16` | `println!` |
| 7 | `bin/test_meta.rs:19` | `println!` |
| 8 | `bin/test_meta.rs:20` | `println!` |
| 9 | `bin/test_meta.rs:21` | `println!` |
| 10 | `config.rs:606` | `eprintln!` |
| 11 | `hooks/mod.rs:215` | `eprintln!` |
| 12 | `hooks/mod.rs:226` | `eprintln!` |
| 13 | `hooks/mod.rs:229` | `eprintln!` |
| 14 | `hooks/mod.rs:232` | `eprintln!` |
| 15 | `hooks/mod.rs:235` | `eprintln!` |
| 16 | `hooks/mod.rs:238` | `eprintln!` |
| 17 | `hooks/mod.rs:241` | `eprintln!` |
| 18 | `hooks/mod.rs:247` | `eprintln!` |
| 19 | `mcp/mod.rs:188` | `eprintln!` |
| 20 | `mcp/mod.rs:192` | `eprintln!` |
| 21 | `mcp/mod.rs:323` | `eprintln!` |
| 22 | `permission/audit.rs:59` | `eprintln!` |
| 23 | `permission/audit.rs:65` | `eprintln!` |
| 24 | `permission/whitelist.rs:40` | `eprintln!` |
| 25 | `runtime/run.rs:651` | `eprintln!` |
| 26 | `runtime/run.rs:665` | `eprintln!` |
| 27 | `runtime/run.rs:674` | `eprintln!` |
| 28 | `trace/mod.rs:64` | `eprintln!` |
| 29 | `trace/mod.rs:73` | `eprintln!` |

**Breakdown:** 25 `eprintln!`, 4 `println!` (all in `bin/test_meta.rs`), 0 `dbg!`

---

## (4) `std::fs::` usage in files containing `async fn` (blocking I/O in async context) — **24 total** across **8 files**

| # | Location | Call |
|---|----------|------|
| 1 | `memory/reflection.rs:187` | `std::fs::read_to_string` |
| 2 | `memory/reflection.rs:191` | `std::fs::write` |
| 3 | `reflector/mod.rs:70` | `std::fs::read_to_string` |
| 4 | `reflector/mod.rs:97` | `std::fs::read_to_string` |
| 5 | `reflector/mod.rs:220` | `std::fs::create_dir_all` |
| 6 | `reflector/mod.rs:254` | `std::fs::write` |
| 7 | `runtime/run.rs:1325` | `std::fs::read_to_string` |
| 8 | `runtime/run.rs:1331` | `std::fs::read_to_string` |
| 9 | `runtime/run.rs:1346` | `std::fs::read_to_string` |
| 10 | `runtime/run.rs:1354` | `std::fs::read_to_string` |
| 11 | `runtime/run.rs:1361` | `std::fs::read_dir` |
| 12 | `runtime/run.rs:1370` | `std::fs::read_to_string` |
| 13 | `runtime/run.rs:1543` | `std::fs::create_dir_all` |
| 14 | `runtime/run.rs:1582` | `std::fs::write` |
| 15 | `tools/edit.rs:49` | `std::fs::read_to_string` |
| 16 | `tools/edit.rs:65` | `std::fs::write` |
| 17 | `tools/grep.rs:92` | `std::fs::read_to_string` |
| 18 | `tools/grep.rs:108` | `std::fs::read_dir` |
| 19 | `tools/read_file.rs:97` | `std::fs::metadata` |
| 20 | `tools/read_file.rs:109` | `std::fs::File::open` |
| 21 | `tools/subagent.rs:704` | `std::fs::read_to_string` |
| 22 | `tools/subagent.rs:710` | `std::fs::read_to_string` |
| 23 | `tools/write_file.rs:55` | `std::fs::create_dir_all` |
| 24 | `tools/write_file.rs:60` | `std::fs::write` |

**Worst offenders:** `runtime/run.rs` (8 instances), `tools/` directory (10 instances across 5 files)

---

## (5) `std::env::set_current_dir` / `set_var` calls NOT in test modules — **2 total**

| # | Location | Code |
|---|----------|------|
| 1 | `tools/subagent.rs:647` | `let _ = std::env::set_current_dir(orig)` — cleanup/restore in `Drop`-like context |
| 2 | `tools/subagent.rs:655` | `std::env::set_current_dir(target)?` — changes global CWD (process-wide side effect) |

*(Note: `config.rs:848,868` have `set_var` calls but are inside test modules, excluded.)*

---

## (6) `TODO`/`FIXME`/`HACK`/`XXX`/`todo!()`/`unimplemented!()` NOT in test modules — **2 total**

| # | Location | Type | Content |
|---|----------|------|---------|
| 1 | `cron/manager.rs:15` | `unimplemented!()` | `CronScheduler::new()` — will panic at runtime |
| 2 | `runtime/run.rs:678` | `TODO` comment | `// TODO: implement input request mechanism (future phase)` |

---

## (7) `unsafe` blocks NOT in test modules — **1 real** (1 comment false-positive)

| # | Location | Type | Details |
|---|----------|------|---------|
| 1 | `permission/mod.rs:877` | **Comment only** | `// Any shell metacharacters indicate potentially complex/unsafe commands.` — not a real `unsafe` block |
| 2 | `runtime/supervisor.rs:246` | **Real `unsafe` block** | `unsafe { libc::killpg(pgid, libc::SIGKILL); }` — process group kill in supervisor cleanup. Has `// SAFETY:` doc comment. |

**Actual `unsafe` blocks: 1**

---

## (8) Files over 500 lines — **25 files** (out of 101 total)

| # | Lines | File |
|---|-------|------|
| 1 | 1726 | `runtime/run.rs` |
| 2 | 1384 | `context.rs` |
| 3 | 1194 | `permission/mod.rs` |
| 4 | 1121 | `session.rs` |
| 5 | 1089 | `agent/mod.rs` |
| 6 | 1014 | `tasks/mod.rs` |
| 7 | 998 | `config.rs` |
| 8 | 865 | `tools/webfetch.rs` |
| 9 | 838 | `tools/subagent.rs` |
| 10 | 741 | `memory/mod.rs` |
| 11 | 732 | `memory/recall.rs` |
| 12 | 732 | `compressor.rs` |
| 13 | 712 | `workflow/definition.rs` |
| 14 | 690 | `runtime/manager.rs` |
| 15 | 647 | `skills/mod.rs` |
| 16 | 622 | `runtime/event.rs` |
| 17 | 619 | `memory/salience.rs` |
| 18 | 608 | `subagent/mod.rs` |
| 19 | 581 | `types.rs` |
| 20 | 567 | `workflow/executor.rs` |
| 21 | 565 | `workflow/context.rs` |
| 22 | 541 | `agent_registry/skill_drafts.rs` |
| 23 | 539 | `permission/types.rs` |
| 24 | 522 | `reflector/mod.rs` |
| 25 | 520 | `runtime/brain.rs` |

**Top 3 largest:** `runtime/run.rs` (1726), `context.rs` (1384), `permission/mod.rs` (1194)

---

## (9) Functions over 100 lines (body length) — **24 functions**

| # | Lines | Location | Function |
|---|-------|----------|----------|
| 1 | 346 | `memory/storage.rs:41` | `fn init_tables()` |
| 2 | 326 | `agent/executor.rs:31` | `async fn execute_tools()` |
| 3 | 297 | `permission/mod.rs:298` | `fn check()` |
| 4 | 284 | `runtime/run.rs:683` | `async fn run_turn()` |
| 5 | 246 | `subagent/mod.rs:165` | `async fn run_with_sender()` |
| 6 | 227 | `runtime/manager.rs:180` | `async fn create_run_with_workdir()` |
| 7 | 206 | `agent/mod.rs:207` | `fn build()` |
| 8 | 164 | `workflow/executor.rs:57` | `async fn execute()` |
| 9 | 162 | `tools/subagent.rs:668` | `async fn spawn_single()` |
| 10 | 161 | `runtime/run.rs:1283` | `fn refresh_context_segments()` |
| 11 | 159 | `runtime/event.rs:308` | `fn from_agent_event()` |
| 12 | 149 | `tools/read_file.rs:73` | `async fn execute()` |
| 13 | 145 | `tasks/mod.rs:514` | `async fn execute()` |
| 14 | 143 | `agent/mod.rs:461` | `async fn run_with_events()` |
| 15 | 141 | `permission/rules.rs:9` | `fn default_rules_with_danger()` |
| 16 | 127 | `skills/manifest.rs:157` | `fn parse_yaml_frontmatter()` |
| 17 | 121 | `tools/webfetch.rs:233` | `async fn execute()` |
| 18 | 119 | `tools/subagent.rs:223` | `async fn execute_with_stream()` |
| 19 | 112 | `runtime/run.rs:357` | `async fn run()` |
| 20 | 111 | `workflow/validate.rs:38` | `fn validate()` |
| 21 | 106 | `client/mod.rs:162` | `async fn send_with_retry()` |
| 22 | 102 | `subagent/mod.rs:417` | `async fn collect_stream()` |
| 23 | 101 | `runtime/run.rs:163` | `fn new()` |
| 24 | 101 | `runtime/run.rs:1034` | `async fn collect_stream()` |

**Note:** 16 of 24 are `async fn`. `runtime/run.rs` alone has 5 functions over 100 lines.

---

## (10) Clippy configuration — **NOT configured** ❌

| Check | Result |
|-------|--------|
| `clippy.toml` / `.clippy.toml` in project | ❌ Not found anywhere |
| `#![deny(clippy::...)]` / `#![warn(clippy::...)]` in `lib.rs` | ❌ Not found |
| Any `#![...]` crate-level lint attributes in `lib.rs` | ❌ Not found |
| Clippy config in `core/Cargo.toml` | ❌ Not found |
| Clippy config in workspace `Cargo.toml` | ❌ Not found |
| `#[allow(clippy::...)]` suppressions (anywhere in code) | 3 instances in `workflow/executor.rs` (lines 231, 337, 403) — all `#[allow(clippy::too_many_arguments)]` |

---

## Summary Table

| # | Category | Count | Severity |
|---|----------|-------|----------|
| 1 | `unwrap()` (non-test) | **25** | High — will panic on `None`/`Err` |
| 2 | `expect()` (non-test) | **5** | Medium — panics with message |
| 3 | `eprintln!`/`println!`/`dbg!` (non-test) | **29** | Medium — should use `tracing` crate |
| 4 | Blocking `std::fs::` in async files | **24** | High — blocks async runtime |
| 5 | `set_current_dir`/`set_var` (non-test) | **2** | High — global side effects |
| 6 | TODO/FIXME/unimplemented!() (non-test) | **2** | Medium — `unimplemented!()` will panic |
| 7 | Real `unsafe` blocks (non-test) | **1** | Low — has SAFETY doc |
| 8 | Files over 500 lines | **25** | Medium — maintainability |
| 9 | Functions over 100 lines | **24** | Medium — maintainability |
| 10 | Clippy configured | **No** | Medium — no lint enforcement |
