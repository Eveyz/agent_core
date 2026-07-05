I have a clear map of the codebase. Let me read all the tool files in parallel batches.
Let me read the remaining tool files in parallel.
Now let me read the remaining tool files and the permission system.
Now let me read the permission system files.
Now let me read the client files and the remaining parts I haven't seen.
Let me read the remaining sections and check for the CWD mutation bug and remaining content.
Let me search for unwrap/expect patterns and the redirect_if_artifact function.
Let me look at the ProcessSupervisor and check how the CWD guard interacts with concurrent subagent runs.
Now let me look at the archival_memory and core_memory tools to round out the analysis.
Let me check a few remaining details for completeness.
I now have a comprehensive picture of the entire system. Let me compile the structured report.

---

# Deep-Dive Analysis: Tools & Permission System

## Table of Contents
1. [Subagent CWD Mutation Bug (Critical)](#1-subagent-cwd-mutation-bug)
2. [Tool-by-Tool Analysis](#2-tool-by-tool-analysis)
3. [Permission System Analysis](#3-permission-system-analysis)
4. [Client System Analysis](#4-client-system-analysis)
5. [Cross-Cutting Findings](#5-cross-cutting-findings)

---

## 1. Subagent CWD Mutation Bug (Critical)

**Files:** `core/src/tools/subagent.rs:642-657, 810-816`

This is the most severe bug in the codebase. The subagent system mutates **process-global state** (the OS-level working directory) from within an async context, causing data races across concurrent subagents and corrupting the parent agent's CWD.

### The Bug Mechanism

```rust
// subagent.rs:642-657
struct CwdGuard(Option<std::path::PathBuf>);

impl Drop for CwdGuard {
    fn drop(&mut self) {
        if let Some(ref orig) = self.0 {
            let _ = std::env::set_current_dir(orig);  // ← restores CWD
        }
    }
}

fn set_cwd_guard(target: &std::path::Path) -> anyhow::Result<CwdGuard> {
    let original = std::env::current_dir().ok();
    std::env::set_current_dir(target)?;  // ← MUTATES PROCESS-GLOBAL CWD
    Ok(CwdGuard(original))
}
```

At `subagent.rs:810-816`, `spawn_single` acquires this guard before running the subagent:
```rust
let _cwd_guard = match set_cwd_guard(&workspace_root) {
    Ok(g) => Some(g),
    Err(e) => { ... None }
};
let result = subagent.run_with_sender(task, event_sender).await?;  // ← long-running async
// _cwd_guard dropped here
```

### Why This Is Catastrophic

**Race Condition #1 — Concurrent subagents (subagent.rs:264-306):**
The `SubagentSpawnAllTool` spawns multiple subagents concurrently via `tokio::task::JoinSet` (line 264). Each calls `spawn_single` → `set_cwd_guard`. Since `std::env::set_current_dir` mutates **process-global** state and tokio tasks run concurrently on the same thread pool:

- Subagent A sets CWD to `/workspace` at T=0
- Subagent B sets CWD to `/workspace` at T=1 (overwrites A's "original" capture)
- Subagent A finishes, drops its guard, restores CWD to `/workspace` (B's "original", not the real original)
- Subagent B finishes, restores CWD to `/workspace` (correct, but only by luck)
- If subagents targeted *different* workspace roots, the restores would cross-corrupt

**Race Condition #2 — Parent agent tools:**
While any subagent is running (which can take minutes), the parent agent's tools that read `std::env::current_dir()` will see the *subagent's* CWD, not the parent's. Affected call sites:
- `glob.rs:69` — `std::env::current_dir()` for path relativization
- `permission/mod.rs:764` — `canonicalize_target` uses `current_dir()` for relative path sandboxing
- `runtime/run.rs:209, 1286, 1317` — Run path resolution
- `agent/mod.rs:285` — Agent CWD capture
- `skills/mod.rs:97` — Skill directory scanning

**Race Condition #3 — Panic during restore:**
If the subagent panics, `CwdGuard::drop` fires, but `set_current_dir(orig)` itself can fail silently (`let _ = ...`). If `original` was `None` (because `current_dir()` failed at capture time), no restore happens at all.

**Race Condition #4 — `find_workspace_root` TOCTOU (subagent.rs:620-637):**
```rust
fn find_workspace_root(start: &std::path::Path) -> std::path::PathBuf {
    loop {
        if current.join("Cargo.toml").exists() {  // ← TOCTOU: file could be deleted
            return current;
        }
```
The workspace root is determined by walking up to find `Cargo.toml`. This is fundamentally wrong for non-Rust projects (Python, JS, Go) — they have no `Cargo.toml`, so it falls back to `start` (line 634), which is the already-mutated CWD.

### A++ Recommendation

**Eliminate process-global CWD mutation entirely.** Pass the working directory explicitly through the tool execution context:

```rust
// 1. Add working_dir to the Tool trait or a ToolContext
pub struct ToolContext {
    pub working_dir: PathBuf,
    pub session_id: Option<String>,
    pub event_sender: Option<EventSender>,
}

// 2. Thread it through execute_with_stream
async fn execute_with_stream(&self, args: Value, ctx: &ToolContext) -> Result<String>;

// 3. Each tool uses ctx.working_dir instead of std::env::current_dir()
//    - bash: already takes working_dir param → just default to ctx.working_dir
//    - glob: use ctx.working_dir as base instead of current_dir()
//    - grep: use ctx.working_dir as base
//    - read_file: resolve relative paths against ctx.working_dir
//    - permission canonicalize_target: use ctx.working_dir
```

This is the correct architecture. `std::env::set_current_dir` is **never safe** in a multi-threaded async runtime.

---

## 2. Tool-by-Tool Analysis

### 2.1 `bash.rs` — Bash Tool

#### (1) unwrap()/expect() in production paths
- **None in production paths.** All unwraps are in the legacy `wait_fut` which uses `unwrap_or(-1)` (line 293-294), which is acceptable.

#### (2) Security vulnerabilities

**CRITICAL — Command injection is by design but the permission layer's detection is bypassable:**
- `bash.rs:243-251` — Spawns `sh -c <command>`. The command string is passed verbatim. This is the intended design (the LLM writes bash), but it means the permission system's `is_destructive_command` / `is_readonly_command` checks are the *only* defense.
- `permission/mod.rs:934-964` — `is_destructive_command` explicitly admits it "cannot defeat arbitrary shell quoting/`$()` substitution." This means:
  - `eval "rm -rf /"` — **NOT detected as destructive** (no `rm` token at the expected position)
  - `bash -c "rm -rf /"` — **NOT detected**
  - `printf "rm -rf /" | sh` — **NOT detected** (pipe splits on `|`, but `sh` isn't in the destructive list)
  - `python -c "import os; os.system('rm -rf /')"` — **NOT detected**
  - `$HOME/evil_script.sh` — **NOT detected**
  - Variable expansion: `CMD="rm"; $CMD -rf /` — **NOT detected**

**MEDIUM — `working_dir` is not sandbox-validated in the bash tool itself:**
- `bash.rs:99-103` — `working_dir` defaults to `.` or `self.default_working_dir`. The permission system's `check_path` only validates the `path` parameter, not `working_dir`. An LLM could pass `working_dir: "/etc"` and the bash command runs there, bypassing sandbox.
- The permission `check()` signature has `path: Option<&str>` but the bash tool's `working_dir` is never passed as `path` to the permission check.

**LOW — Timeout can be set to arbitrarily large values:**
- `bash.rs:104` — `timeout_secs` is `args["timeout_secs"].as_u64().unwrap_or(60)`. No upper bound. An LLM could set `timeout_secs: 999999` to hold a process for ~11 days.

#### (3) Blocking I/O in async functions
- `bash.rs:142-161` — `sup.lock()` uses `parking_lot::Mutex` which is a **blocking lock** held in an async context. Three separate lock acquisitions happen (spawn, take_stdout, take_stderr), each blocking the executor thread. If another task holds the lock (e.g., kill_all during cancel), this blocks the worker thread.

#### (4) Error handling that returns confusing messages to LLM
- `bash.rs:111` — `.context("command timed out")?` — The LLM sees "command timed out" but doesn't know *which* command or how long it ran. Should include the command and timeout value.
- `bash.rs:151, 159` — `"child disappeared after spawn"` — This is a race condition message that gives the LLM no actionable information.

#### (5) Missing input validation
- **No validation on `working_dir` existence** — If the directory doesn't exist, `sh -c` fails with a confusing "No such file or directory" error.
- **No upper bound on `timeout_secs`** — Should clamp to a reasonable maximum (e.g., 600s).

#### (6) Race conditions
- `bash.rs:142-221` — The supervised path acquires the lock 4 times (spawn, take_stdout, take_stderr, kill). Between `spawn` and `take_stdout`, another task could call `kill_all()`, causing `get_child` to return `None` → "child disappeared after spawn" error.
- `bash.rs:198-214` — The `wait_fut` polls `try_exit_code()` every 50ms with lock acquisition. This is a busy-wait that holds the lock 20x/second.

#### (7) Missing test coverage
- **No tests at all** for `bash.rs`. No test for:
  - Command execution success/failure
  - Timeout behavior
  - Working directory handling
  - Process group kill on cancel
  - Stdout/stderr streaming

#### (8) A++ Recommendations
1. **Pass `working_dir` to permission `check()` as the `path` parameter** so sandbox validation covers it.
2. **Clamp `timeout_secs`**: `let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(60).min(600);`
3. **Hold the supervisor lock once** for spawn + take_stdout + take_stderr to eliminate the race:
   ```rust
   let (child_id, stdout_handle, stderr_handle) = {
       let mut sup = sup.lock();
       let id = sup.spawn_bash(command, working_dir)?;
       let child = sup.get_child(&id).ok_or_else(...)?;
       (id, child.take_stdout(), child.take_stderr())
   };
   ```
4. **Replace busy-wait** with `child.wait().await` instead of polling `try_exit_code()` every 50ms.
5. **Add comprehensive tests** using `tokio::test`.

---

### 2.2 `edit.rs` — Edit Tool

#### (1) unwrap()/expect() in production paths
- **`edit.rs:69`** — `old_content.find(old_string).unwrap_or(0)` — If `old_string` is empty (which passes the `count > 1` check since empty string matches infinitely, but `matches().count()` returns `old_content.len()+1`), this returns 0. However, empty `old_string` should be rejected explicitly.

#### (2) Security vulnerabilities
**MEDIUM — No path traversal protection:**
- `edit.rs:42-46` — `file_path` is used directly (or through `redirect_if_artifact` which only redirects specific filenames). There's no sandbox check in the tool itself. The permission system checks `path`, but `edit.rs` doesn't pass the resolved path to any validation.
- An LLM can edit `../../etc/cron.d/evil` if permission allows.

**LOW — No atomic write:**
- `edit.rs:65` — `std::fs::write(&resolved_path, &new_content)` is not atomic. A crash mid-write leaves a corrupted file. Should write to a temp file and rename.

#### (3) Blocking I/O in async functions
- **`edit.rs:49`** — `std::fs::read_to_string` — **blocking file I/O in async function**
- **`edit.rs:65`** — `std::fs::write` — **blocking file I/O in async function**
- These block the tokio worker thread. Should use `tokio::fs::read_to_string` and `tokio::fs::write`.

#### (4) Error handling
- `edit.rs:50` — Error message includes the path and OS error, which is good.
- `edit.rs:54` — `"old_string not found in '{}'"` — Good, includes path. But doesn't show *what* was searched for, which would help the LLM debug.
- `edit.rs:57-61` — Ambiguous match error includes count and path, which is good.

#### (5) Missing input validation
- **No empty `old_string` check** — An empty `old_string` matches `count = old_content.len() + 1` (typically > 1), so it bails with "found N times". But this is confusing; should explicitly reject empty `old_string`.
- **No file size check** — Reads entire file into memory. A 1GB file would OOM. Should check metadata like `read_file.rs` does.
- **No binary file detection** — Unlike `read_file.rs`, edit doesn't check for NUL bytes. Editing a binary file would produce garbage.

#### (6) Race conditions
- **TOCTOU between read and write** (edit.rs:49-65) — Between `read_to_string` and `write`, another process could modify the file. The edit would silently overwrite the concurrent change. Should use file locking or compare-and-swap.

#### (7) Missing test coverage
- **No tests at all** for `edit.rs`. No test for:
  - Successful edit
  - old_string not found
  - Ambiguous match
  - Empty old_string
  - Binary file rejection
  - Diff generation correctness

#### (8) A++ Recommendations
1. **Use `tokio::fs`** for all I/O operations.
2. **Add empty `old_string` validation**: `if old_string.is_empty() { bail!("old_string must not be empty"); }`
3. **Add file size guard** (reuse `MAX_FILE_SIZE_BYTES` from read_file).
4. **Add binary detection** (NUL byte probe like read_file).
5. **Atomic write** via temp file + rename.
6. **Add tests.**

---

### 2.3 `read_file.rs` — Read File Tool

#### (1) unwrap()/expect() in production paths
- **`read_file.rs:124`** — `reader.read(&mut probe).unwrap_or(0)` — This silently swallows read errors during the binary probe. If the read fails (e.g., permission denied on the first 8KB), it proceeds with `n=0` and the file appears empty, then bails with "File is empty" instead of the real error.

#### (2) Security vulnerabilities
**LOW — Path traversal:** Same as edit — no tool-level sandbox check. Relies on permission layer.

#### (3) Blocking I/O in async functions
- **`read_file.rs:97`** — `std::fs::metadata` — blocking
- **`read_file.rs:109`** — `std::fs::File::open` — blocking
- **`read_file.rs:124`** — `reader.read` — blocking
- **`read_file.rs:138-140`** — `reader.read_line` — blocking
- **`read_file.rs:130-132`** — `reader.seek` — blocking

All of these are synchronous std::io operations in an async function. For small files this is tolerable, but for files near the 1MB limit, this blocks the worker thread for the full read duration.

#### (4) Error handling
- Generally excellent. Error messages include path, line numbers, and actionable suggestions.
- `read_file.rs:126` — "File appears to be binary (contains NUL bytes)" — clear and actionable.
- `read_file.rs:100-104` — Includes byte count and limit, plus suggests offset/limit. Good.

#### (5) Missing input validation
- **No symlink resolution** — A symlink could point outside the sandbox. The permission layer's `canonicalize_target` handles this for permission checks, but `read_file` follows symlinks without any check.
- `offset` and `limit` are validated (lines 81-86). Good.

#### (6) Race conditions
- **TOCTOU between metadata check and open** — `metadata()` at line 97 could see a 1KB file, but by `File::open` at line 109, it could have grown to 1GB. The 1MB size check is ineffective against this.

#### (7) Missing test coverage
- Good coverage: 8 tests covering line numbers, offset/limit, continuation hints, binary rejection, invalid UTF-8, oversize files, empty files, zero offset.
- **Missing:** No test for the char-cap truncation path (pathological single line), no test for symlink following.

#### (8) A++ Recommendations
1. **Use `tokio::fs`** for async I/O.
2. **Fix the `unwrap_or(0)`** on the binary probe to propagate the error.
3. **Add symlink resolution** check (resolve symlink, verify target is within sandbox).

---

### 2.4 `write_file.rs` — Write File Tool

#### (1) unwrap()/expect() — None.

#### (2) Security vulnerabilities
**MEDIUM — No sandbox check in tool:** Same as edit — relies on permission layer.
**MEDIUM — `create_dir_all` can create arbitrary directories:** `write_file.rs:53-57` — An LLM could write to `/tmp/../../etc/cron.d/evil` and `create_dir_all` would create the path. The permission check happens *before* the tool runs, but if permission is granted (e.g., Yolo mode or whitelist), there's no additional guard.

#### (3) Blocking I/O in async functions
- **`write_file.rs:55`** — `std::fs::create_dir_all` — blocking
- **`write_file.rs:60`** — `std::fs::write` — blocking

#### (4) Error handling
- Good — includes path in error messages.

#### (5) Missing input validation
- **No content size limit** — An LLM could write a multi-GB string. Should cap content size.
- **No path validation** — No check for path traversal (`..`).

#### (6) Race conditions
- None significant (single write operation).

#### (7) Missing test coverage
- **No tests at all.** No test for:
  - Successful write
  - Parent directory creation
  - Overwrite existing file
  - Permission errors

#### (8) A++ Recommendations
1. **Use `tokio::fs`.**
2. **Add content size limit** (e.g., 10MB).
3. **Add path traversal validation** (reject `..` components or canonicalize and check sandbox).
4. **Add tests.**

---

### 2.5 `grep.rs` — Grep Tool

#### (1) unwrap()/expect() in production paths
- **`grep.rs:84`** — `glob::Pattern::new(ext_filter).unwrap_or(glob::Pattern::new("*").unwrap())` — The inner `unwrap()` on `glob::Pattern::new("*")` is safe (static valid pattern), but the `unwrap_or` silently falls back to `*` when the user provides an invalid pattern. This is confusing — if the LLM passes `include: "[invalid"`, it silently matches all files instead of erroring.

#### (2) Security vulnerabilities
**MEDIUM — Unbounded directory traversal:**
- `grep.rs:70-131` — `search_path` recursively walks directories. The only pruning is skipping `.`, `target`, and `node_modules` (line 121). But:
  - No symlink following protection — a symlink loop could cause infinite recursion.
  - No depth limit — could traverse `/` if `path: "/"` is given.
  - No total file count limit — could open thousands of files.
  - `std::fs::read_to_string` (line 92) reads entire files into memory — a 1GB file would OOM.

**LOW — Regex DoS (ReDoS):** The user-provided regex pattern (`grep.rs:50`) is compiled and applied to every line of every file. A catastrophic backtracking pattern (e.g., `(a+)+$`) could hang the executor.

#### (3) Blocking I/O in async functions
- **`grep.rs:92`** — `std::fs::read_to_string` — blocking, reads entire file
- **`grep.rs:108`** — `std::fs::read_dir` — blocking
- **`grep.rs:115-116`** — `entry?` and `entry.file_type()?` — blocking

All synchronous in an async function. A grep over a large codebase blocks the worker thread for the entire duration.

#### (4) Error handling
- `grep.rs:63` — "No matches found." — Good, clear.
- `grep.rs:109` — Error includes path. Good.

#### (5) Missing input validation
- **No `path` existence check** — If path doesn't exist, `is_file()` and `is_dir()` both return false, and the function silently returns no results. Should error.
- **No `max_results` upper bound** — Could be set to millions.
- **No file size limit** before `read_to_string`.

#### (6) Race conditions
- **TOCTOU between `is_file()` and `read_to_string`** — File could be deleted between check and read.

#### (7) Missing test coverage
- **No tests at all.** No test for:
  - Basic search
  - Include filter
  - Directory traversal
  - Max results
  - Invalid regex
  - Non-existent path

#### (8) A++ Recommendations
1. **Use `tokio::fs`** and `walkdir` crate (with symlink loop detection).
2. **Add depth/file-count limits** to prevent runaway traversal.
3. **Add file size check** before `read_to_string`.
4. **Use `regex::RegexBuilder` with `size_limit`** to prevent ReDoS.
5. **Error on invalid `include` pattern** instead of silent fallback.
6. **Error on non-existent `path`**.
7. **Add tests.**

---

### 2.6 `glob.rs` — Glob Tool

#### (1) unwrap()/expect() in production paths
- **`glob.rs:69`** — `std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))` — Safe fallback, but uses process-global CWD which is corrupted by the subagent CWD bug.

#### (2) Security vulnerabilities
**LOW — No sandbox check:** Results are not filtered against sandbox boundaries.

#### (3) Blocking I/O in async functions
- **`glob.rs:56`** — `glob::glob(&full_pattern_str)` returns an iterator that performs blocking filesystem I/O when iterated.
- **`glob.rs:61-80`** — `for entry in paths` — blocking iteration in async context.

#### (4) Error handling
- `glob.rs:58` — "invalid glob pattern: {e}" — Clear.
- `glob.rs:83` — "No matching files found." — Clear.

#### (5) Missing input validation
- **No `max_results` upper bound** — Could be set to millions.
- **No path existence check** for `base_path`.

#### (6) Race conditions
- None significant.

#### (7) Missing test coverage
- 2 tests: basic matching and no-match. Good for basic cases.
- **Missing:** No test for absolute pattern, max_results, invalid pattern, symlink handling.

#### (8) A++ Recommendations
1. **Use `tokio::task::spawn_blocking`** for the glob iteration since it's inherently synchronous.
2. **Add upper bound on `max_results`** (e.g., 1000).
3. **Add more tests.**

---

### 2.7 `webfetch.rs` — WebFetch Tool

#### (1) unwrap()/expect() in production paths
- **`webfetch.rs:142`** — `self.last_access.lock().unwrap()` — **`std::sync::Mutex` in async context.** If the mutex is poisoned (panic during lock hold), this panics the entire task. Should use `parking_lot::Mutex` or handle the poison.
- **`webfetch.rs:162`** — Same issue, second lock acquisition.

#### (2) Security vulnerabilities
**MEDIUM — SSRF (Server-Side Request Forgery):**
- `webfetch.rs:233-264` — The URL is not validated against internal IP ranges. An LLM (or a malicious webpage via redirect) could fetch:
  - `http://169.254.169.254/latest/meta-data/` (AWS metadata)
  - `http://localhost:8080/admin` (internal services)
  - `http://10.0.0.1/internal-api`
  - `http://[::1]:9090/metrics`
  The `upgrade_to_https` (line 239) only upgrades `http://` to `https://`, which doesn't block internal IPs.

**MEDIUM — Cookie jar persistence across sessions:**
- `webfetch.rs:184` — `cookie_store: Arc<Jar>` — The cookie jar is shared across all webfetch calls for the lifetime of the tool. Cookies from one site could leak to another via redirect chains. There's no cookie domain isolation per-request.

**LOW — robots.txt check uses `*` user-agent (line 462):** This is the most conservative choice, but it means the tool respects robots.txt for all bots, which could block legitimate access.

#### (3) Blocking I/O in async functions
- **`webfetch.rs:647-648`** — `readability::extractor::extract(&mut cursor, &url_parsed)` — This is a **synchronous CPU-intensive** HTML parsing operation running in an async context. For large HTML documents, this blocks the worker thread.
- **`webfetch.rs:622`** — `htmd::convert(&article_html)` — Same, synchronous CPU-intensive conversion.
- **`webfetch.rs:142, 162`** — `std::sync::Mutex` lock — blocking lock in async context.

#### (4) Error handling
- Excellent. `format_http_error` (lines 742-799) provides categorized, actionable error messages with suggestions for timeout, connect, redirect, body, DNS, and certificate errors.
- `format_http_status` (lines 801-865) provides explanations and suggestions per HTTP status code.
- These are A+ quality — among the best error messages for LLMs in the codebase.

#### (5) Missing input validation
- **No URL scheme validation** — Only `http://` and `https://` should be allowed. `file:///etc/passwd` would be passed to reqwest (though reqwest may reject it).
- **No response size limit** — `response.text().await` (line 291) reads the entire response body into memory. A 10GB response would OOM.
- **No redirect-to-internal-IP protection** — The redirect policy allows 8 redirects (line 257), which could redirect to internal services.

#### (6) Race conditions
- **Rate limiter race** (webfetch.rs:138-167) — Between the `required_delay` check (line 141-154) and the `insert` (line 161-164), another concurrent request could insert a newer timestamp, causing the delay calculation to be stale. This is a minor TOCTOU but could result in slightly faster-than-intended requests.

#### (7) Missing test coverage
- **No tests at all** for webfetch.rs (despite being 865 lines). No test for:
  - URL upgrading
  - Rate limiting
  - Robots.txt checking
  - Meta extraction
  - Content extraction
  - Error formatting
  - Image content type detection

#### (8) A++ Recommendations
1. **Add SSRF protection**: Validate resolved IP addresses against private ranges (RFC 1918, 169.254.0.0/16, ::1, fc00::/7) before connecting.
2. **Add response size limit**: Use `response.bytes()` with a streaming size check, bail if > 10MB.
3. **Switch to `parking_lot::Mutex`** for the rate limiter to avoid poisoning panics.
4. **Use `tokio::task::spawn_blocking`** for readability extraction and htmd conversion.
5. **Add URL scheme validation**: reject anything not `http://` or `https://`.
6. **Add comprehensive tests** (mock HTTP server).

---

### 2.8 `tavily_search.rs` — Tavily Search Tool

#### (1) unwrap()/expect() — None.

#### (2) Security vulnerabilities
**LOW — API key in request body:** `tavily_search.rs:60` — The API key is sent in the JSON request body, not as a header. This is the Tavily API design, but the key could leak in logs if the request body is logged.

#### (3) Blocking I/O — None (uses reqwest async).

#### (4) Error handling
- `tavily_search.rs:74` — "Failed to send HTTP request to Tavily" — Good.
- `tavily_search.rs:77` — "Failed to parse Tavily JSON response" — Good.
- `tavily_search.rs:107` — "Failed to extract results from API." — This is returned as `Ok()`, not `Err()`. The LLM sees this as a successful result, which is confusing. Should be an error or include the raw response.

#### (5) Missing input validation
- **No query length limit** — Could send a 1MB query string.
- **No timeout configuration** — Uses the default reqwest client (no timeout). Could hang indefinitely.

#### (6) Race conditions — None.

#### (7) Missing test coverage
- **No tests.**

#### (8) A++ Recommendations
1. **Add timeout to the reqwest client**: `reqwest::Client::builder().timeout(Duration::from_secs(30)).build()`.
2. **Add query length limit** (e.g., 1000 chars).
3. **Return error** when results can't be extracted, not Ok with a confusing message.
4. **Add tests** with mocked HTTP.

---

### 2.9 `subagent.rs` — Subagent Tools

#### (1) unwrap()/expect() in production paths
- **`subagent.rs:380`** — `pending_calls.pop_front().unwrap_or_else(...)` — Safe (provides fallback).

#### (2) Security vulnerabilities
**CRITICAL — CWD mutation bug** (detailed in §1 above).

**MEDIUM — No tool name validation against available tools:**
- `subagent.rs:736-749` — Tool names from args are used directly. While line 774 filters against `available_tools`, a typo'd tool name is silently dropped, and the subagent gets `read_file` as a fallback (line 778). This is confusing for the LLM.

**LOW — `max_iterations` has no upper bound:**
- `subagent.rs:733` — `args["max_iterations"].as_u64().unwrap_or(parent_max_iterations as u64) as usize` — Could be set to billions.

#### (3) Blocking I/O in async functions
- **`subagent.rs:704, 710`** — `std::fs::read_to_string` for persona files — blocking.
- **`subagent.rs:624, 653, 693`** — `std::env::current_dir()` — blocking (but fast).
- **`subagent.rs:629`** — `current.join("Cargo.toml").exists()` — blocking, and called in a loop.

#### (4) Error handling
- `subagent.rs:679, 682` — "missing 'id'" / "missing 'task'" — Good.
- `subagent.rs:337` — `"ERROR: {}"` — Good, includes error.
- `subagent.rs:340` — `"JOIN ERROR: {}"` — Good, distinguishes join failures.

#### (5) Missing input validation
- **No `id` uniqueness check** for concurrent subagents — Two subagents with the same ID would have conflicting session saves.
- **No `task` length limit.**
- **No `max_iterations` upper bound.**

#### (6) Race conditions
- **CWD mutation** (detailed in §1).
- **`session_mgr.lock()` in concurrent subagents** (subagent.rs:297-300) — Each concurrent subagent locks the session manager to save results. This serializes saves but is correct.
- **`find_workspace_root` TOCTOU** — Cargo.toml could be deleted/created between check and use.

#### (7) Missing test coverage
- **No tests at all** for subagent.rs (despite being 838 lines). No test for:
  - Single subagent spawn
  - Concurrent subagent spawn
  - Tool filtering
  - Result strategy formatting
  - Tool summary building
  - CWD guard behavior

#### (8) A++ Recommendations
1. **Fix the CWD bug** (see §1).
2. **Add `id` uniqueness validation** for concurrent batches.
3. **Add `max_iterations` upper bound** (e.g., `min(max_iterations, 1000)`).
4. **Use `tokio::fs`** for persona file reads.
5. **Add tests** for all spawn paths.

---

### 2.10 `todo.rs`, `skill.rs`, `memory.rs` tools

#### Blocking I/O
- `skill.rs:46, 114, 173, 217` — `self.manager.lock()` uses `parking_lot::Mutex` (blocking) in async context. Since skill operations are fast, this is tolerable.
- `todo.rs:60, 91, 146` — Same pattern.

#### Race conditions
- **`skill.rs:114-128`** — `SkillLoadTool` holds the lock across `find_by_name`, `load_skill_context`, and `activate`. If `load_skill_context` does file I/O (blocking), this holds the lock for the entire duration, blocking all other skill operations.

#### Missing test coverage
- `todo.rs` — No tests.
- `skill.rs` — No tests.
- `archival_memory.rs` — No tests.
- `core_memory.rs` — No tests.
- `recall_memory.rs` — No tests.

---

## 3. Permission System Analysis

### 3.1 `permission/mod.rs`

#### (1) unwrap()/expect() in production paths
- **`mod.rs:764`** — `std::env::current_dir().unwrap_or_default()` — Safe fallback, but returns empty PathBuf if CWD is unreadable, which would produce incorrect sandbox checks.
- **`mod.rs:675`** — `serde_json::from_str(_tool_input_json).unwrap_or_default()` — Silently swallows JSON parse errors and produces an empty `Value::Null`. The approval prompt then shows no tool input, which is misleading.

#### (2) Security vulnerabilities

**CRITICAL — `is_destructive_command` is bypassable (mod.rs:934-964):**
As detailed in §2.1, the command inspection cannot handle:
- `eval "rm -rf /"`
- `bash -c "rm -rf /"`
- `python -c "import os; os.system('rm -rf /')"`
- `printf 'rm -rf /' | sh`
- Variable expansion: `CMD=rm; $CMD -rf /`
- Subshell: `$(echo rm) -rf /`
- Heredocs with embedded commands

The function explicitly admits this in its doc comment (mod.rs:871-873): "It cannot defeat arbitrary shell quoting/`$()` substitution." This is a **known limitation** that should be prominently documented to users, not buried in a code comment.

**MEDIUM — `matches_command` uses prefix matching (types.rs:260-269):**
```rust
allowed.iter().any(|prefix| cmd.starts_with(prefix.as_str()))
```
This means a whitelist entry for `"ls"` also matches `"ls; rm -rf /"` because `ls; rm -rf /`.starts_with(`"ls"`). The command `"lsrm"` would also match. This is a **command injection via prefix** vulnerability.

**MEDIUM — Sandbox path check doesn't cover `working_dir` (mod.rs:612-627):**
The `check_path` function validates the `path` parameter, but `bash`'s `working_dir` parameter is never passed as `path`. An agent could set `working_dir: "/etc"` and run commands there.

**MEDIUM — `canonicalize_target` uses `current_dir()` (mod.rs:764):**
This is corrupted by the subagent CWD bug. If a subagent has changed the CWD, sandbox path resolution for relative paths would use the wrong base directory.

**LOW — `glob_match` regex injection (types.rs:465-481):**
The glob-to-regex conversion doesn't escape all regex metacharacters. Only `.` is escaped. Characters like `+`, `(`, `)`, `[`, `]`, `{`, `}`, `^`, `$`, `\` are not escaped. A pattern like `file(name)` would be interpreted as a regex group, potentially causing unexpected matches or regex errors.

#### (3) Blocking I/O
- **`mod.rs:618`** — `sandbox.canonicalize()` — blocking I/O in `check_path`. This is called synchronously from the permission check, which may be in an async context.

#### (4) Error handling
- Generally good. Deny/Ask messages include tool name, danger level, and matched rule.
- `mod.rs:623-626` — Sandbox denial includes path and allowed roots. Good.

#### (5) Missing input validation
- **No validation on config rule patterns** — Invalid glob patterns silently match nothing (via `glob_match` returning false on regex error).

#### (6) Race conditions
- **`PermissionPolicy` is `&mut self` in `check()`** — This means the policy must be behind a mutex. If multiple tools run concurrently (they don't in the current code, but the architecture allows it), the whitelist `query()` which touches `entry.touch()` (mutating `use_count` and `last_used`) would need synchronization.

#### (7) Missing test coverage
- Good test coverage for the core permission logic: destructive command detection, sandbox paths, whitelist overrides, yolo mode, paranoid mode, auto-allow, config rules.
- **Missing:**
  - No test for the prefix-matching command injection vulnerability.
  - No test for `glob_match` with regex metacharacters.
  - No test for concurrent permission checks.

#### (8) A++ Recommendations
1. **Fix `matches_command`**: Use tokenized matching, not prefix matching. Split the command on whitespace and check if the first token (or the effective program after wrapper stripping) is in the allowed list.
2. **Document the `is_destructive_command` limitations prominently** in user-facing docs.
3. **Escape all regex metacharacters in `glob_match`** or use a proper glob library.
4. **Pass `working_dir` to `check_path`** for bash commands.
5. **Fix `unwrap_or_default()` on JSON parse** in `build_approval_prompt` — log a warning instead of silently showing empty input.

---

### 3.2 `permission/whitelist.rs`

#### (2) Security vulnerabilities
**HIGH — TOML injection in `persist_to_config` (whitelist.rs:137-220):**
The function manually constructs TOML by string interpolation:
```rust
new_whitelist_section.push_str(&format!(
    "tool_pattern = \"{}\"\n",
    entry.pattern.tool_pattern  // ← not escaped!
));
```
If `tool_pattern` contains `"` or newlines, the resulting TOML is malformed or injects arbitrary config. A pattern like `bash"\ncommands = ["rm"]\ntool_pattern = "bash` would inject a whitelist entry for `rm`.

#### (7) Missing test coverage
- 4 tests: add/query, command filter, purge, remove.
- **Missing:** No test for `persist_to_config` (the injection vulnerability), no test for concurrent access.

#### (8) A++ Recommendations
1. **Use `toml::to_string`** instead of manual string interpolation for serialization.
2. **Add test for `persist_to_config`** with special characters in patterns.

---

### 3.3 `permission/audit.rs`

#### (2) Security vulnerabilities
**MEDIUM — `truncate` can panic on non-UTF-8 boundary (audit.rs:147-153):**
```rust
format!("{}...<truncated>", &s[..max_len])
```
If `max_len` falls in the middle of a multi-byte UTF-8 character, this panics. Should use `floor_char_boundary`.

**LOW — Non-atomic trim (audit.rs:121-135):**
`trim_if_needed` reads all entries, rewrites the entire file. A crash during rewrite loses entries.

#### (3) Blocking I/O
- All file operations are synchronous (`std::fs`). Since audit recording happens during permission checks (which may be in async context), this blocks the worker thread.

#### (7) Missing test coverage
- 2 tests: record/read and stats. Adequate for basic functionality.

#### (8) A++ Recommendations
1. **Fix `truncate`** to use char-boundary-safe slicing.
2. **Use atomic writes** (temp file + rename) for trim.
3. **Use `tokio::fs`** or `spawn_blocking` for file I/O.

---

### 3.4 `permission/rules.rs`

#### (2) Security vulnerabilities
**MEDIUM — Readonly command list includes `sed` and `awk` (rules.rs:62-63, 892):**
- `sed` without `-i` is read-only, but `sed -n` with `w` command writes files: `sed -n 'w /tmp/evil' file`
- `awk` can execute system commands: `awk 'system("rm -rf /")'`
- The check at rules.rs:897-899 only blocks `-i`/`--in-place` for sed, and doesn't check for `system()` in awk.

#### (7) Missing test coverage
- 2 tests: rule coverage and bash defaults. Adequate.

#### (8) A++ Recommendations
1. **Remove `sed` and `awk` from the readonly list** or add more sophisticated checking.
2. **Add tests** for sed/awk command edge cases.

---

## 4. Client System Analysis

### 4.1 `client/mod.rs`

#### (1) unwrap()/expect() in production paths
- **`mod.rs:68`** — `.expect("failed to build http client")` — This is a panic on startup if the HTTP client can't be built. Since this is during initialization (not runtime), it's borderline acceptable, but should return a `Result` instead.

#### (2) Security vulnerabilities
**MEDIUM — API key in URL (mod.rs:192):**
```rust
.bearer_auth(&current_client.model.api_key)
```
The API key is sent as a bearer token, which is correct. But if the base_url contains query parameters, the key could leak in logs. (reqwest doesn't log URLs by default, but custom middleware might.)

**LOW — `no_proxy()` disables system proxy settings (mod.rs:66):**
This means the client can't use corporate proxies, which could cause connection failures in enterprise environments. More importantly, it means the client **ignores `HTTPS_PROXY`**, which could be a security concern if the environment requires traffic inspection.

#### (3) Blocking I/O — None (all reqwest async).

#### (4) Error handling
- `mod.rs:128` — `"no choices in response"` — Good, but doesn't include the raw response for debugging.
- `mod.rs:205` — `"API error {}"` — Includes status code but not response body.
- `mod.rs:230` — `"API error {status}: {body_text}"` — Good, includes body.
- **Confusing for LLM:** The retry logic (mod.rs:188-250) retries on 429 and 5xx, but the error messages don't distinguish between "rate limited, try again" and "server error, may be persistent." The LLM can't tell if retrying the same request will help.

#### (5) Missing input validation
- **No URL validation on `base_url`** — A malformed base_url would produce a confusing reqwest error.

#### (6) Race conditions
- **`CircuitBreaker` is shared via `Arc`** (mod.rs:17) — The circuit breaker state is shared between the primary and fallback client. If both use the same breaker, a failure on the primary trips the breaker for the fallback too. Looking at the code, `fallback_client` is a `Box<OpenAIClient>` (not `Arc`), so each has its own breaker. But the fallback's breaker is never checked before use (mod.rs:168 only checks `current_client.circuit_breaker`).

#### (7) Missing test coverage
- **No tests at all** for client/mod.rs. No test for:
  - Request body construction
  - Retry logic
  - Circuit breaker integration
  - Fallback behavior
  - Tool call ID deduplication

#### (8) A++ Recommendations
1. **Return `Result` from `build_http_client`** instead of `expect()`.
2. **Check fallback's circuit breaker** before using it.
3. **Add response body to 429/5xx error messages** for LLM debugging.
4. **Add tests** with mocked HTTP server.
5. **Consider allowing proxy** via config option.

---

### 4.2 `client/resilience.rs`

#### (2) Security vulnerabilities — None.

#### (4) Error handling
- `resilience.rs:62` — `"Circuit breaker is OPEN. Requests are blocked."` — Good, clear message.

#### (6) Race conditions
**MEDIUM — HalfOpen state is not protected against concurrent requests (resilience.rs:64-68):**
```rust
CircuitState::HalfOpen => {
    state.state = CircuitState::Open;  // ← immediately set back to Open
    Ok(())
}
```
The intent is "only allow one request through in HalfOpen." But if multiple tasks call `acquire_permit` concurrently:
- Task A sees HalfOpen, sets to Open, returns Ok.
- Task B sees Open (set by A), checks timeout, may also return Ok if timeout elapsed.
This allows more than one request through the half-open state.

#### (7) Missing test coverage
- **No tests.** No test for:
  - Circuit breaker state transitions
  - Failure threshold
  - Reset timeout
  - Backoff calculation

#### (8) A++ Recommendations
1. **Add a `HalfOpen` permit counter** to truly limit concurrent half-open requests.
2. **Add tests** for all state transitions.

---

### 4.3 `client/streaming.rs`

#### (2) Security vulnerabilities
- None.

#### (4) Error handling
- `streaming.rs:37` — `Err(_) => continue` — **Silently drops unparseable SSE events.** If the API sends malformed JSON, the stream continues without error, potentially missing tool call deltas. Should log a warning.
- `streaming.rs:71` — `"no choices array"` — Good, but the error is propagated as `Err`, which terminates the stream. Could be more graceful.

#### (5) Missing input validation
- **No `index` bounds check** (streaming.rs:94) — `tc["index"].as_u64().unwrap_or(0) as usize` — A malicious API could send a huge index, causing the `HashMap` to allocate excessively.

#### (7) Missing test coverage
- 4 tests for `TokenAccumulator`. Good.
- **No tests for `SseParser`** — The most complex and critical component.
- **No tests for `ToolCallAccumulator`** — No test for:
  - Multiple tool calls in sequence
  - ID deduplication
  - Missing function name
  - Empty arguments

#### (8) A++ Recommendations
1. **Log warnings** for unparseable SSE events instead of silently dropping.
2. **Add tests for `SseParser`** with various SSE formats.
3. **Add tests for `ToolCallAccumulator`**.
4. **Add index bounds check** or cap.

---

## 5. Cross-Cutting Findings

### 5.1 Blocking I/O Summary

The following tools perform **synchronous file I/O in async functions**, blocking the tokio worker thread:

| File | Line(s) | Operation | Severity |
|------|---------|-----------|----------|
| `edit.rs` | 49, 65 | `std::fs::read_to_string`, `std::fs::write` | High |
| `read_file.rs` | 97, 109, 124, 138 | `std::fs::metadata`, `File::open`, `Read`, `Seek` | Medium |
| `write_file.rs` | 55, 60 | `create_dir_all`, `std::fs::write` | High |
| `grep.rs` | 92, 108 | `read_to_string`, `read_dir` | High |
| `glob.rs` | 56-80 | `glob::glob` iteration | Medium |
| `webfetch.rs` | 647-648, 622 | `readability::extract`, `htmd::convert` | High |
| `subagent.rs` | 704, 710, 629 | `read_to_string`, `exists()` | Medium |
| `whitelist.rs` | 143, 218 | `read_to_string`, `write` | Medium |
| `audit.rs` | 79, 112, 133 | `read_to_string`, `OpenOptions`, `write` | Medium |

**Recommendation:** Either use `tokio::fs` (for simple operations) or wrap in `tokio::task::spawn_blocking` (for CPU-intensive operations like readability extraction).

### 5.2 Missing Test Coverage Summary

| File | Lines | Has Tests? | Coverage Quality |
|------|-------|-----------|-----------------|
| `bash.rs` | 310 | ❌ None | Critical gap |
| `edit.rs` | 111 | ❌ None | Critical gap |
| `write_file.rs` | 69 | ❌ None | Critical gap |
| `grep.rs` | 132 | ❌ None | Critical gap |
| `read_file.rs` | 400 | ✅ 8 tests | Good |
| `glob.rs` | 141 | ✅ 2 tests | Adequate |
| `webfetch.rs` | 865 | ❌ None | Critical gap |
| `subagent.rs` | 838 | ❌ None | Critical gap |
| `tavily_search.rs` | 112 | ❌ None | High gap |
| `todo.rs` | 161 | ❌ None | Medium gap |
| `skill.rs` | 239 | ❌ None | Medium gap |
| `archival_memory.rs` | 180 | ❌ None | Medium gap |
| `core_memory.rs` | 189 | ❌ None | Medium gap |
| `recall_memory.rs` | 145 | ❌ None | Medium gap |
| `permission/mod.rs` | 1194 | ✅ 14 tests | Good |
| `permission/rules.rs` | 300 | ✅ 2 tests | Adequate |
| `permission/types.rs` | 539 | ✅ 6 tests | Good |
| `permission/whitelist.rs` | 300 | ✅ 4 tests | Adequate |
| `permission/audit.rs` | 246 | ✅ 2 tests | Adequate |
| `client/mod.rs` | 279 | ❌ None | Critical gap |
| `client/resilience.rs` | 97 | ❌ None | High gap |
| `client/streaming.rs` | 313 | ✅ 4 tests (accumulator only) | SseParser/ToolCallAccumulator untested |

### 5.3 Error Message Quality for LLMs

**A+ quality (clear, actionable, includes context):**
- `read_file.rs` — Size limits, binary detection, continuation hints
- `webfetch.rs` — Categorized HTTP errors with suggestions
- `permission/mod.rs` — Deny reasons with matched rule

**Confusing for LLMs:**
- `tavily_search.rs:107` — "Failed to extract results from API." returned as `Ok()`
- `bash.rs:111` — "command timed out" without command name or timeout
- `bash.rs:151, 159` — "child disappeared after spawn" (race condition, not actionable)
- `client/mod.rs:205` — "API error {}" without response body on retryable errors
- `streaming.rs:37` — Silently drops unparseable SSE events
- `grep.rs:84` — Invalid include pattern silently matches all files
- `mod.rs:675` — `unwrap_or_default()` hides invalid tool input JSON

### 5.4 Top Priority Fixes (Ranked)

1. **🔴 CRITICAL: Subagent CWD mutation bug** (subagent.rs:642-657, 810-816) — Process-global state mutation in async context. Fix: thread working_dir through ToolContext.

2. **🔴 CRITICAL: `matches_command` prefix matching** (types.rs:260-269) — Allows `ls; rm -rf /` to match whitelist entry for `ls`. Fix: tokenize and match first token.

3. **🔴 CRITICAL: TOML injection in whitelist persistence** (whitelist.rs:153-180) — Unescaped patterns in manual TOML construction. Fix: use `toml::to_string`.

4. **🟠 HIGH: `is_destructive_command` bypasses** (mod.rs:934-964) — `eval`, `bash -c`, variable expansion all bypass detection. Fix: document limitation prominently; consider a restricted shell.

5. **🟠 HIGH: SSRF in webfetch** (webfetch.rs:233-264) — No internal IP range blocking. Fix: validate resolved IPs against private ranges.

6. **🟠 HIGH: No sandbox check on bash `working_dir`** (bash.rs:99-103, mod.rs:612-627) — Fix: pass `working_dir` to `check_path`.

7. **🟠 HIGH: Blocking I/O in async functions** (12 files) — Fix: use `tokio::fs` or `spawn_blocking`.

8. **🟡 MEDIUM: `glob_match` regex injection** (types.rs:465-481) — Fix: escape all metacharacters or use glob library.

9. **🟡 MEDIUM: `sed`/`awk` in readonly list** (rules.rs:62-63, 892) — Fix: remove or add sophisticated checking.

10. **🟡 MEDIUM: Missing test coverage** (14 of 22 files have no tests) — Fix: add tests, especially for bash, edit, webfetch, subagent, client.



I'll analyze the tools/ and permission/ subsystems systematically. Let me start by locating all the files and reading them in parallel.
Let me read all the target files in parallel:
Now let me read the remaining parts of the files that were truncated:
Let me read the remaining parts of the larger files and run the greps:
Now let me read the remaining critical sections - the rest of subagent.rs, the rest of permission/mod.rs, and the rest of rules.rs tests, plus check for test modules:
Let me read the remaining sections of permission/mod.rs tests and the rest of the subagent spawn_single function:
Now I have a comprehensive picture. Let me compile the structured report.

---

# Structured Code Review Report: `tools/` and `permission/` Subsystems

## Table of Contents
1. [Cross-cutting: `eprintln!`/`println!`/`dbg!` Audit](#1-cross-cutting)
2. [tools/bash.rs](#2-toolsbashrs)
3. [tools/edit.rs](#3-toolseditrs)
4. [tools/read_file.rs](#4-toolsread_filers)
5. [tools/write_file.rs](#5-toolswrite_filers)
6. [tools/grep.rs](#6-toolsgreprs)
7. [tools/glob.rs](#7-toolsglobrs)
8. [tools/webfetch.rs](#8-toolswebfetchrs)
9. [tools/subagent.rs](#9-toolssubagentrs)
10. [tools/mod.rs](#10-toolsmodrs)
11. [permission/mod.rs](#11-permissionmodrs)
12. [permission/rules.rs](#12-permissionrulesrs)
13. [permission/types.rs](#13-permissiontypesrs)
14. [permission/whitelist.rs](#14-permissionwhitelistrs)
15. [permission/audit.rs](#15-permissionauditrs)
16. [Priority Summary](#16-priority-summary)

---

## 1. Cross-cutting: `eprintln!`/`println!`/`dbg!` Audit

No `println!` or `dbg!` found in either subsystem. The `eprintln!` instances:

| File:Line | Message |
|---|---|
| `permission/whitelist.rs:40` | `eprintln!("[permission] pruned {} expired whitelist entries", expired);` |
| `permission/audit.rs:59` | `eprintln!("[audit] failed to serialize entry: {e}");` |
| `permission/audit.rs:65` | `eprintln!("[audit] failed to write entry: {e}");` |

**Recommendation:** The rest of the codebase uses `tracing` (see `subagent.rs:813`). These should all be converted to `tracing::warn!` / `tracing::error!` for consistency and structured logging. The audit failures at `audit.rs:59,65` are particularly important — they indicate audit integrity loss and should be `tracing::error!`.

---

## 2. `tools/bash.rs`

### (1) `unwrap()`/`expect()` in production paths
**None.** All `unwrap_or`/`unwrap_or_else` usage is safe (lines 103, 104). The `s.code().unwrap_or(-1)` at line 293 is intentional (signal-terminated processes have no exit code).

### (2) Blocking I/O in async functions
- **Line 247-248:** `std::process::Stdio::piped()` — this is just a configuration constant, not blocking I/O. ✅ Acceptable.

### (3) Lock patterns
Uses `parking_lot::Mutex` (`Arc<Mutex<ProcessSupervisor>>`):
- **Lines 143, 148, 156:** Three separate `sup.lock()` calls in `run_bash_supervised` to spawn, take stdout, take stderr. Each lock is held only briefly and released immediately (RAII blocks). However, this creates a TOCTOU window: between `spawn_bash` (line 144) and `get_child` (line 149), another task could theoretically remove the child. The `ok_or_else` at line 151 handles this, so it's not a crash — just a possible spurious error.

**Recommendation:** Consider acquiring the lock once, taking both stdout and stderr handles in a single critical section:
```rust
let (stdout_handle, stderr_handle) = {
    let mut supervisor = sup.lock();
    let child = supervisor.get_child(&child_id)
        .ok_or_else(|| anyhow::anyhow!("child disappeared after spawn"))?;
    (child.take_stdout(), child.take_stderr())
};
```

- **Lines 201, 220:** Polling loop with `sup_clone.lock()` every 50ms (line 212). This is a busy-wait pattern that holds the lock repeatedly. While functional, it's inefficient.
  **Recommendation:** Use an async channel or `tokio::select!` on the child's exit future instead of polling.

### (4) Error handling gaps
- **Line 178:** `let _ = stdout.read_to_end(&mut buf).await;` — read errors silently ignored.
- **Line 190:** `let _ = stderr.read_to_end(&mut buf).await;` — same.
- **Line 221:** `let _ = supervisor.kill(&child_id);` — kill failure ignored (acceptable for cleanup, but should at least `tracing::warn!`).
- **Line 263, 271:** Legacy path has the same `let _ =` read_to_end pattern.

**Recommendation:** Log these with `tracing::debug!` at minimum so failures are diagnosable.

### (5) Missing test coverage
**No tests at all.** This is the most security-critical tool (shell execution). Missing:
- Timeout behavior test
- Exit code propagation test
- stderr capture test
- Supervised vs legacy path equivalence test
- Process-group kill on cancel test

### (6) Logic bugs / edge cases
- **Lines 170, 263:** `while let Ok(Some(line)) = lines.next_line().await` — if `next_line()` returns `Err`, the loop silently terminates, losing remaining output and hiding I/O errors. Should log the error.
- **Lines 229-231:** Exit code `-1` is used for both "child disappeared" and "signal-terminated". These are semantically different but indistinguishable in the output.
- **Line 104:** `timeout_secs` has no upper bound. A malicious/careless caller could pass `u64::MAX`. The description says "Timeout: 60 seconds" but the default is 60 with no max clamp (contrast with `webfetch.rs:238` which clamps to 120).

### (7) Code organization
- The supervised and legacy paths share ~40 lines of identical stdout/stderr/result-assembly logic (lines 163-233 vs 256-308). **Recommendation:** Extract a shared `assemble_result(stdout_fut, stderr_fut, wait_fut)` helper.

---

## 3. `tools/edit.rs`

### (1) `unwrap()`/`expect()` in production paths
- **Line 69:** `let byte_offset = old_content.find(old_string).unwrap_or(0);` — This is technically safe (count was verified >0 at line 52), but `unwrap_or(0)` would produce a wrong line number silently if the find somehow failed. Since we already checked `count >= 1`, this should be `.unwrap()` or better, restructure to avoid the double-find.

**Recommendation:** Capture the byte offset during the count check:
```rust
let first_match = old_content.find(old_string);
// count check already done, so first_match is Some
let byte_offset = first_match.unwrap(); // now safe
```

### (2) Blocking I/O in async functions
- **Line 49:** `std::fs::read_to_string(&resolved_path)` — **blocking** in `async fn execute`.
- **Line 65:** `std::fs::write(&resolved_path, &new_content)` — **blocking** in `async fn execute`.

**Recommendation:** Use `tokio::fs::read_to_string` and `tokio::fs::write`, or `tokio::task::spawn_blocking`.

### (4) Error handling gaps
- **Line 65:** `std::fs::write(&resolved_path, &new_content)?;` — if the write fails, the old content is lost (the file may be truncated/corrupted). 
  **Recommendation:** Write to a temp file first, then atomically rename.

### (5) Missing test coverage
**No tests.** Missing:
- Basic replace test
- Multiple matches → error test
- Not found → error test
- Diff output format test
- Line range calculation test

### (6) Logic bugs / edge cases
- **Line 69:** `unwrap_or(0)` as noted above.
- The `old_string == new_string` case is not rejected — it would produce a no-op edit with a diff showing no changes. **Recommendation:** Add an early bail if they're equal.
- No file size guard (contrast with `read_file.rs` which caps at 1 MB). A very large file loaded into memory could cause OOM.

### (7) Code organization
- The `execute` method is 80 lines doing argument parsing, path resolution, file I/O, matching, diff generation, and formatting. **Recommendation:** Extract `compute_line_range()`, `generate_diff()`, and `resolve_path()` helpers.

---

## 4. `tools/read_file.rs`

### (1) `unwrap()`/`expect()` in production paths
- **Line 124:** `reader.read(&mut probe).unwrap_or(0)` — If the read fails (e.g. I/O error), it's treated as 0 bytes read. This silently skips the binary check and proceeds to read lines, which may then fail with a confusing error.

**Recommendation:** Propagate the error:
```rust
let n = reader.read(&mut probe)
    .with_context(|| format!("failed to probe file: {path}"))?;
```

All other `unwrap()` calls are in `#[cfg(test)]` (lines 231, 233, 240, 256, 269, 283, 303, 305, 346, 359, 369) — acceptable in tests.

### (2) Blocking I/O in async functions
- **Line 97:** `std::fs::metadata(&resolved_path)` — **blocking**
- **Line 109:** `std::fs::File::open(&resolved_path)` — **blocking**
- **Line 111:** `std::io::BufReader::new(file)` then synchronous `read_line` loop (line 138) — **blocking**

The entire file reading pipeline uses synchronous `std::io` in an async function. This blocks the tokio runtime thread for the duration of file I/O.

**Recommendation:** Use `tokio::fs::File::open` and `tokio::io::BufReader`, or wrap the whole synchronous read in `tokio::task::spawn_blocking`.

### (3) Lock patterns
None.

### (4) Error handling gaps
- **Line 124:** As noted above, `unwrap_or(0)` swallows read errors.

### (5) Missing test coverage
**Good coverage** — 10 tests covering: full read, offset/limit, default cap, continuation, binary detection, invalid UTF-8, oversize rejection, zero offset, empty file, huge line truncation, line alignment. This is the best-tested file in the subsystem.

### (6) Logic bugs / edge cases
- **Lines 179-181:** `if collected == 0 && line_num == 0` — a file with content but where all lines are before `offset` (e.g., 5-line file, offset=999) returns `line_num=5, collected=0`, so this check doesn't fire. The test at line 339 confirms this returns an empty-range header `[Lines 999-999]` which is slightly misleading (suggests the file has 999+ lines). Minor UX issue.
- **Line 154-155:** `strip_suffix('\n')` then `strip_suffix('\r')` — handles `\n` and `\r\n` but not bare `\r` (old Mac line endings). Edge case, probably acceptable.

### (7) Code organization
Well-structured with clear constants and comments. The `floor_char_boundary` helper is duplicated in `subagent.rs:519`. **Recommendation:** Extract to a shared utility module.

---

## 5. `tools/write_file.rs`

### (1) `unwrap()`/`expect()`
None.

### (2) Blocking I/O in async functions
- **Line 55:** `std::fs::create_dir_all(parent)` — **blocking**
- **Line 60:** `std::fs::write(&resolved_path, content)` — **blocking**

**Recommendation:** Use `tokio::fs` equivalents.

### (5) Missing test coverage
**No tests.** Missing:
- Basic write test
- Parent directory creation test
- Path resolution (artifact redirect) test
- Error on unwritable path test

### (6) Logic bugs / edge cases
- **Line 64:** `content.len()` reports byte length, not char count. The message says "wrote N bytes" which is correct, but could confuse an LLM expecting character counts.
- No path validation — writing to `/dev/null` or `/proc/...` would succeed or fail unpredictably.

### (7) Code organization
Clean and minimal. No issues.

---

## 6. `tools/grep.rs`

### (1) `unwrap()`/`expect()` in production paths
- **Line 84:** `glob::Pattern::new(ext_filter).unwrap_or(glob::Pattern::new("*").unwrap())` — The inner `glob::Pattern::new("*").unwrap()` is safe (literal `"*"` always parses), but the pattern is questionable: if the user provides an invalid glob filter, it silently falls back to `"*"` (match everything), which is the opposite of what a filter should do.

**Recommendation:** Return an error for invalid patterns:
```rust
let pattern = glob::Pattern::new(ext_filter)
    .with_context(|| format!("invalid include pattern: {ext_filter}"))?;
```

### (2) Blocking I/O in async functions
- **Line 92:** `std::fs::read_to_string(path)` — **blocking**
- **Line 108:** `std::fs::read_dir(path)` — **blocking**
- The recursive `search_path` function does all I/O synchronously within the async `execute` method.

**Recommendation:** Wrap in `spawn_blocking` or use `tokio::fs`.

### (4) Error handling gaps
- **Line 92:** `if let Ok(content) = std::fs::read_to_string(path)` — silently skips files that can't be read (permission denied, binary, invalid UTF-8). No logging.
- **Line 116:** `let file_type = entry.file_type()?;` — propagates error, but this could abort the entire search if one directory entry is problematic. **Recommendation:** Log and continue.

### (5) Missing test coverage
**No tests.** Missing:
- Basic pattern match test
- Include filter test (especially the invalid-pattern fallback)
- Directory recursion test
- Hidden directory skip test
- max_results enforcement test

### (6) Logic bugs / edge cases
- **Line 84:** As noted, invalid glob silently matches everything.
- **Line 121:** Hidden directories starting with `.` are skipped, but `target` and `node_modules` are hardcoded. Other build artifact dirs (`.git` is covered by `.`) like `dist`, `build`, `__pycache__` are not. Minor.
- **Line 92:** Non-UTF-8 files are silently skipped. If searching a directory with mixed encodings, results may be incomplete with no indication.
- **No symlink loop protection.** `search_path` follows symlinks via `read_dir` → `file_type()`. A symlink loop would cause infinite recursion.
  **Recommendation:** Use `entry.metadata()` and check `file_type.is_symlink()` to skip or resolve symlinks safely.

### (7) Code organization
The `search_path` function is recursive and mixes file/dir logic. Acceptable for this size.

---

## 7. `tools/glob.rs`

### (1) `unwrap()`/`expect()` in production paths
- **Line 69:** `std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))` — safe fallback.

All other `unwrap()` calls (lines 96, 99, 105, 106, 107, 121, 137) are in `#[cfg(test)]`.

### (2) Blocking I/O in async functions
- **Line 69:** `std::env::current_dir()` — blocking (but fast, kernel call).
- The `glob::glob()` iterator and the `for entry in paths` loop are synchronous and block the async runtime.

**Recommendation:** Wrap in `spawn_blocking`.

### (4) Error handling gaps
- **Line 76:** `Err(_) => { continue; }` — glob traversal errors (permission denied, broken symlinks) are silently skipped. No logging.

### (5) Missing test coverage
**2 tests** — basic matching and no-match. Missing:
- `max_results` enforcement
- Absolute pattern path
- Invalid glob pattern error
- Permission denied on subdirectory

### (6) Logic bugs / edge cases
- **Line 48:** `Path::new(pattern).is_absolute()` — if the pattern is absolute, `base_path` is ignored. This could be surprising if a user passes both `path` and an absolute `pattern`.
- **Line 69:** `strip_prefix(std::env::current_dir()...)` — if `base_path` is `"."` and the pattern is relative, the results are stripped against the *process* CWD, not necessarily the `base_path`. This can produce confusing relative paths when `base_path != CWD`.

### (7) Code organization
Clean. No issues.

---

## 8. `tools/webfetch.rs`

### (1) `unwrap()`/`expect()` in production paths
- **Line 142:** `self.last_access.lock().unwrap()` — `std::sync::Mutex` poisoning panic.
- **Line 162:** `self.last_access.lock().unwrap()` — same.

These use `std::sync::Mutex` (not `parking_lot`), so `.lock().unwrap()` can panic on poisoning.

**Recommendation:** Either switch to `parking_lot::Mutex` (which doesn't poison) or handle the poison error gracefully:
```rust
let map = self.last_access.lock().unwrap_or_else(|e| e.into_inner());
```

### (2) Blocking I/O in async functions
- **Line 647:** `std::io::Cursor::new(html.as_bytes())` — this is in-memory, not blocking I/O. ✅
- The `readability::extractor::extract()` at line 648 is a CPU-bound synchronous call inside `extract_readable`, which is called from `execute` (async). This can block the runtime for large HTML documents.

**Recommendation:** Wrap `extract_readable` in `spawn_blocking`.

### (3) Lock patterns
- **Lines 142, 162:** `std::sync::Mutex` held briefly, released by RAII. But there's a subtle issue: the lock at line 142 is released, then `tokio::time::sleep` runs (line 157), then a second lock at line 162. Between these two locks, another request to the same domain could sneak in and set a `last_access` time, causing the current request to wait again on its next call. Minor race condition in the rate limiter.

### (4) Error handling gaps
- **Line 139:** `let host = Self::extract_host(url_str).ok()?;` — if host extraction fails, `wait()` returns `None` silently, skipping rate limiting entirely. A malformed URL bypasses the rate limiter.
- **Lines 446-458:** `check_robots` silently returns `Ok(())` on all errors (network failure, body read failure). This means robots.txt is effectively best-effort, which is reasonable but should be documented.

### (5) Missing test coverage
**No tests.** This is a 850+ line file with complex HTML parsing, rate limiting, UA rotation, and content extraction. Missing:
- `upgrade_to_https` test
- `extract_meta` test (title, OG tags, twitter cards)
- `is_image_content_type` test
- `truncate` / `format_http_error` test
- `pick_ua_profile` distribution test
- `DomainRateLimiter` wait/delay test
- `strip_html_tags` test
- `extract_readable` pipeline test

### (6) Logic bugs / edge cases
- **Line 96:** `let mut roll: f64 = rng.r#gen();` — `rand::Rng::gen()` for `f64` generates `[0, 1)`. Then `roll *= total`. If all weights sum to exactly 1.0 (which they do: 0.30+0.25+0.15+0.10+0.05+0.07+0.04+0.04 = 1.00), this is fine. But the weights are hardcoded and could drift. The fallback at line 106 handles the edge case. ✅
- **Line 322-323:** `extracted.floor_char_boundary(max_len)` — `floor_char_boundary` is a nightly-only `str` method. If this compiles, the crate must be on nightly. If on stable, this would be a compile error. (Note: `read_file.rs` has its own `floor_char_boundary` implementation, suggesting the codebase might be on stable — this could be a latent compile issue or a different `extracted` type.)
- **Line 487:** `let _meta_start = lower.find("<meta ");` — unused variable, dead code.
- **Line 523:** `name.to_lowercase().as_str()` — creates a temporary `String` and borrows it in a `match`. This works because the temporary lives for the match duration, but it's a common Rust pitfall. ✅ Compiles fine.
- **Line 625:** `if cleaned.len() > 200` — uses byte length, not char count. A 200-byte string with multibyte chars could have fewer than 200 characters. Minor.

### (7) Code organization
- The file is 850+ lines mixing: UA profiles, rate limiter, HTTP client, HTML parsing, meta extraction, readability, markdown conversion, error formatting. **Recommendation:** Split into submodules: `ua.rs`, `rate_limit.rs`, `html_parser.rs`, `error_format.rs`.
- `floor_char_boundary` is defined locally in `read_file.rs:22` and `subagent.rs:519` but here the std method is used (line 323). Inconsistent.

---

## 9. `tools/subagent.rs`

### (1) `unwrap()`/`expect()` in production paths
- **Line 380:** `pending_calls.pop_front().unwrap_or_else(...)` — safe fallback.

### (2) Blocking I/O in async functions
- **Line 624:** `std::env::current_dir()` — in `find_workspace_root` (sync fn, but called from async `spawn_single`).
- **Line 647:** `std::env::set_current_dir(orig)` — in `CwdGuard::drop`.
- **Line 653:** `std::env::current_dir()` — in `set_cwd_guard`.
- **Line 655:** `std::env::set_current_dir(target)` — **blocking + process-global side effect in async context**.
- **Line 690-691:** `std::env::var("HOME")` / `std::env::var("USERPROFILE")` — blocking (but fast).
- **Line 693:** `std::env::current_dir()` — blocking.
- **Line 704:** `std::fs::read_to_string(&global_agent)` — **blocking file read**.
- **Line 710:** `std::fs::read_to_string(&local_agent)` — **blocking file read**.

**Critical:** The `set_cwd_guard` pattern (lines 652-657) changes the **process-global** CWD in an async context. If multiple subagents run concurrently (via `SubagentSpawnAllTool`), they will race on the process CWD, corrupting each other's working directory. The `CwdGuard` restores on drop, but with concurrent subagents, the restore order is non-deterministic.

**Recommendation:** **This is a correctness bug for concurrent subagents.** Instead of changing process CWD, pass the workspace root explicitly to each tool via a context parameter, or use `spawn_blocking` with a serialized CWD change.

### (3) Lock patterns
- **Lines 137, 298:** `mgr.lock()` — `parking_lot::Mutex` on `SessionManager`. Held briefly. ✅
- **Line 298:** Inside a `tokio::task::JoinSet::spawn` closure — the lock is acquired and released within the spawned task. Safe, but if `save_subagent` is slow, it blocks the spawned task.

### (4) Error handling gaps
- **Line 138:** `let _ = mgr.save_subagent("subagent", &result);` — save failure silently ignored.
- **Line 300:** `let _ = mgr.save_subagent(&id, sub_result);` — same, in concurrent path.

### (5) Missing test coverage
**No tests.** Missing:
- `parse_result_strategy` test
- `build_tool_summary` test (message pairing logic)
- `summarise_tool_content` dispatch test
- `truncate_str` / `floor_char_boundary` test
- `find_workspace_root` test
- `SpawnResult::format_output` for each strategy

The summary functions (`summarise_grep`, `summarise_glob`, etc.) are pure functions that are easily testable and should have unit tests.

### (6) Logic bugs / edge cases
- **CWD race condition** (lines 652-657): As described above — concurrent subagents corrupt each other's CWD.
- **Line 380:** `pending_calls.pop_front()` — tool results are matched to calls by FIFO order. If the LLM returns multiple tool calls in one assistant message and they execute out of order (possible with parallel execution), the pairing will be wrong. The comment acknowledges this ("most recent pending assistant call") but FIFO is used, not LIFO.
- **Line 768:** If `tools` is explicitly `[]` (empty array), `tool_names` is empty, and the code falls through to giving `read_file` only (line 768). But if `tools` is `"all"` (string, not array), `is_all` is set at line 752. If `tools` is `["all"]` (array with "all"), `is_all` is set at line 756-761. However, if `tools` is `["all", "bash"]`, `is_all` is true and `available_tools` is used, ignoring "bash". This is probably fine but undocumented.
- **Line 774:** `final_tool_names.retain(|t| available_tools.contains(t))` — if the parent doesn't have `read_file`, the fallback at line 778 gives `read_file` anyway, which the subagent's `ToolRegistry::from_names` will build but the parent doesn't have. This is intentional (subagent gets read ability) but could be surprising.

### (7) Code organization
- 838 lines in one file. The summary functions (lines 350-527) could be in a `subagent_summary.rs` submodule.
- `floor_char_boundary` duplicated from `read_file.rs`.
- `SpawnResult` and its `format_output` / `summary` methods could be in a separate types module.

---

## 10. `tools/mod.rs`

### (1) `unwrap()`/`expect()`
None in production paths.

### (2) Blocking I/O in async functions
None directly (delegates to tool implementations).

### (3) Lock patterns
- **Lines 28-40:** `try_lock_memory` uses `parking_lot::Mutex::try_lock_for(Duration::from_secs(3))`. This is a timeout-based lock acquisition that returns a JSON error message instead of panicking. Good pattern. ✅

### (4) Error handling gaps
- **Line 248:** `if let Some(tool) = build_tool_by_name(name)` — unknown tool names silently skipped in `from_names`. The doc comment says "Unknown names are silently skipped" so this is intentional, but the caller gets no feedback.
- **Line 263:** `self.tools.remove(*name)` — unknown names silently ignored in `remove_all`. Documented. ✅

### (5) Missing test coverage
**No tests.** Missing:
- `canonicalize_json_object` test (key sorting, array handling)
- `ToolRegistry::register` / `get` / `has` test
- `validate_args` test (valid/invalid args)
- `from_names` test (unknown name skipping)
- `resolve_execution_mode` test

### (6) Logic bugs / edge cases
- **Line 109:** `self.tools.insert(tool.name().to_string(), tool)` — if a tool with the same name is registered twice, the old one is silently replaced. No warning.

### (7) Code organization
Well-organized. The `canonicalize_json_object` function is private but could be useful elsewhere.

---

## 11. `permission/mod.rs`

### (1) `unwrap()`/`expect()` in production paths
- **Line 447:** `PermissionMode::Yolo => unreachable!()` — This is inside a `match self.mode` after the Yolo case is already handled at line 333. If someone adds a new mode and forgets to update this match, it will panic. Acceptable as a exhaustiveness check.
- **Line 675:** `serde_json::from_str(_tool_input_json).unwrap_or_default()` — if the tool input JSON is invalid, defaults to `Value::Null`. This means the approval prompt will show `null` as the tool input. Minor UX issue.
- **Line 764:** `std::env::current_dir().unwrap_or_default()` — safe fallback.
- **Line 618:** `sandbox.canonicalize().unwrap_or_else(|_| sandbox.clone())` — if canonicalization fails (path doesn't exist), falls back to the raw path. This could cause sandbox bypass if the sandbox path is a symlink that doesn't resolve. **Recommendation:** Log a warning when canonicalization fails.

### (2) Blocking I/O in async functions
- **Line 741:** `std::env::var("HOME")` — in `expand_tilde` (sync fn).
- **Line 764:** `std::env::current_dir()` — in `canonicalize_target` (sync fn).
- These are called from `check` which is sync (`pub fn check`), not async. ✅

### (3) Lock patterns
- **Lines 41-44:** `global_pending_approvals()` uses `OnceLock<Arc<Mutex<...>>>` with `parking_lot::Mutex`. Deprecated but correct.

### (4) Error handling gaps
- **Line 618:** As noted, canonicalization failure silently falls back.
- **Line 675:** Invalid JSON silently becomes `Value::Null`.

### (5) Missing test coverage
**Good coverage** — 12 tests covering: read_file allowed, destructive denied, safe command auto-allowed, unknown bash asks, whitelist override, yolo mode, paranoid mode, blacklist override, auto_allow_up_to, config rule override, permissive default, destructive command detection (comprehensive), sandbox path, sandbox deny in check, auto_allow doesn't bypass destructive deny.

Missing:
- `expand_tilde` test
- `canonicalize_target` test (non-existent path, relative path, symlink)
- `normalize_command` test (IFS expansion, whitespace normalization)
- `is_readonly_command` test
- `effective_program_index` test (wrapper skipping, VAR=value)
- Developer mode test
- Permissive mode test
- Whitelist expiry test

### (6) Logic bugs / edge cases
- **Line 618:** Sandbox canonicalization fallback — as noted.
- **Line 330:** `let builtin_deny = tool_name == "bash" && danger == DangerLevel::Destructive;` — only `bash` can trigger the destructive deny. If a future tool wraps shell execution (e.g. a `python` tool that runs `os.system("rm -rf /")`), the destructive check won't fire.
- **Line 874-931:** `is_readonly_command` — the `readonly_programs` list includes `"sed"` and `"awk"` but only checks for `-i` on sed/awk (line 898). However, `sed` with `--in-place` is checked, but `awk` with `-i inplace` (GNU awk extension) is not caught.
- **Line 885:** `lower.split_whitespace()` — this normalizes all whitespace, so `ls\t-la` becomes `ls -la`. Good.
- **Line 910:** `arg.contains("..")` — this rejects any argument containing `..`, including legitimate ones like `file..txt` or `--name=v2..0`. False positive.
- **Line 934-963:** `is_destructive_command` — splits on `[';', '\n', '|', '&']` but not on `||` or `&&` (which contain `&`/`|` so they're split anyway). However, `$()` substitution is not split, so `rm -rf $(echo /)` would have `rm` detected. The fork bomb check at line 938 is pattern-based and fragile.
- **Line 340:** `normalize_command` is called in `is_destructive_command` but not in `is_readonly_command`. So `rm${IFS}-rf` would be caught as destructive (good) but `ls${IFS}-la` would NOT be caught as readonly (the `$` would trigger the `$( ` check at line 881... actually `${IFS}` doesn't contain `$(` so it passes). Wait, line 881 checks for `"$("` specifically. `${IFS}` contains `$` but not `$(`. So `ls${IFS}-la` would pass the metacharacter check, then `split_whitespace()` wouldn't split on `${IFS}`, so `tokens` would be `["ls${ifs}-la"]` (lowercased), which doesn't match any readonly program. So it returns `false` (not readonly), which means it falls through to `System`/`Ask`. This is safe but inconsistent — the destructive check normalizes but the readonly check doesn't.

**Recommendation:** Apply `normalize_command` in `is_readonly_command` as well.

### (7) Code organization
- 1194 lines in one file. The command analysis functions (`normalize_command`, `effective_program_index`, `is_destructive_tokens`, `is_destructive_command`, `is_readonly_command`) should be in a separate `command_analysis.rs` module.
- `expand_tilde` is duplicated in `audit.rs:155`.
- `canonicalize_target` is a complex function that deserves its own tests.

---

## 12. `permission/rules.rs`

### (1) `unwrap()`/`expect()` in production paths
None. All `unwrap()` calls (lines 283, 291, 308) are in `#[cfg(test)]`.

### (2) Blocking I/O in async functions
None (pure data).

### (5) Missing test coverage
**3 tests** — rule coverage, bash defaults, and basic structure. Missing:
- `default_rules()` legacy function test
- `permissive_rules()` test
- `strict_rules()` test
- Verify that `conversation_search` and `conversation_search_date` are covered by rules

### (6) Logic bugs / edge cases
- **Lines 52-70:** The readonly bash command list includes both `"ls "` and `"ls"` (with and without trailing space). This is because `matches_command` uses `starts_with`. `"ls"` would match `"lsxyz"` which isn't `ls`. This is a potential bypass: a command like `lssomething` would be treated as readonly.
  **Recommendation:** Use word-boundary matching instead of `starts_with`.

- **Line 88-95:** `*_memory_*` and `core_memory_*` rules overlap. `core_memory_append` matches both `*_memory_*` and `core_memory_*`. First match wins, both are `Allow`, so no issue. But it's redundant.

- **Lines 154-237:** The legacy `default_rules()` function duplicates the new `default_rules_with_danger()` logic but without danger levels. This is maintenance debt — changes to one must be reflected in the other.

### (7) Code organization
- The legacy `default_rules()`, `permissive_rules()`, `strict_rules()` functions should be marked `#[deprecated]` like `global_pending_approvals()` in `mod.rs`.

---

## 13. `permission/types.rs`

### (1) `unwrap()`/`expect()` in production paths
None in production. `unwrap_or` at line 178 is safe.

### (2) Blocking I/O in async functions
None.

### (4) Error handling gaps
- **Line 178:** `let num: u64 = num_str.parse().unwrap_or(300);` — if the duration string is malformed (e.g., `"abc"`), `split_at(s.len() - 1)` at line 177 takes the last char as `unit` and the rest as `num_str`. If `num_str` doesn't parse, it defaults to 300 seconds (5 minutes). Silent fallback.

### (5) Missing test coverage
**6 tests** — glob match (star, exact, wildcard), command matching, whitelist entry validity, once consumed, danger ordering. Missing:
- `parse_duration_str` test (all units, malformed input, raw seconds)
- `ApprovalScope` serialize/deserialize round-trip test
- `ApprovalScope::is_valid` for Duration with expiry
- `ToolPermissionPattern::matches_path` test
- `ToolPermissionPattern::matches_host` test

### (6) Logic bugs / edge cases
- **Line 177:** `let (num_str, unit) = s.split_at(s.len() - 1);` — **panics on empty string**. If `s` is `""` (after trim), `s.len()` is 0, and `split_at(0 - 1)` underflows. However, `parse::<u64>("")` fails at line 174, so we reach line 177 only with non-empty strings. But if `s` is a single character like `"s"`, `split_at(0)` gives `num_str=""` and `unit="s"`, then `"".parse::<u64>()` fails → `unwrap_or(300)`. So `"s"` → 300 seconds. Semantically odd but safe.
  **Recommendation:** Add `if s.is_empty() { return 300; }` guard for clarity.

- **Line 478:** `regex::Regex::new(&format!("^{}$", regex_pattern))` — the glob-to-regex conversion at lines 474-477 doesn't escape all regex special characters. Only `.` is escaped. Characters like `+`, `(`, `)`, `[`, `]`, `{`, `}`, `^`, `$`, `\` in the pattern would be interpreted as regex. A tool pattern like `git_(status)` would fail to compile as regex and return `false` (line 480), silently failing to match.
  **Recommendation:** Use a proper glob-to-regex conversion or escape all regex metacharacters:
  ```rust
  let regex_pattern: String = pattern.chars().map(|c| match c {
      '*' => ".*".to_string(),
      '?' => ".".to_string(),
      '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '\\' => format!("\\{c}"),
      _ => c.to_string(),
  }).collect();
  ```

- **Line 155:** `ApprovalScope::Once => false` in `is_valid` — Once-scoped entries are always "invalid" by this check. But `WhitelistManager::query` calls `purge_expired` first, then checks matches without calling `is_valid` on the entry. The `check` method in `mod.rs:373` does call `entry.is_valid()`. So a Once-scoped whitelist entry would be matched by `query` (which doesn't check `is_valid`) but then rejected by `check` (which does). This is confusing. Actually, looking more carefully at `whitelist.rs:67-88`, `query` does NOT call `is_valid` — it just matches the pattern. And `mod.rs:372-373`:
  ```rust
  if let Some(entry) = self.whitelist.query(...) {
      if entry.is_valid() {
  ```
  So for `Once` scope: `query` returns the entry, `is_valid()` returns `false`, so the whitelist doesn't allow it. This means Once-scoped whitelist entries never work through the `check` path. But `purge_expired` removes them. So Once entries are purged before they can be used. **This seems like a bug** — Once entries should be allowed once, then purged.

  **Recommendation:** The `Once` scope should return `true` from `is_valid` on the first check, and the caller should then consume/remove it. The current flow purges it before `check` ever sees it.

### (7) Code organization
Well-structured. `parse_duration_str` should be `pub(crate)` or tested.

---

## 14. `permission/whitelist.rs`

### (1) `unwrap()`/`expect()`
None.

### (2) Blocking I/O in async functions
- **Line 143:** `std::fs::read_to_string(&config_path)` — in `persist_to_config` (sync fn).
- **Line 218:** `std::fs::write(&config_path, new_content)` — in `persist_to_config` (sync fn).

These are synchronous functions, but `persist_to_config` may be called from an async context. **Recommendation:** Document that this is blocking, or provide an async variant.

### (4) Error handling gaps
- **Line 40:** `eprintln!` instead of `tracing::warn!` (noted in cross-cutting audit).

### (5) Missing test coverage
**4 tests** — add/query, command filter, purge once, remove. Missing:
- `persist_to_config` test (the TOML manipulation is complex and untested!)
- `load_persistent` with expired entries test
- `query` with path/host filtering test
- Concurrent access test (if used across threads)
- Duration scope expiry test

**The `persist_to_config` function (lines 137-220) is completely untested** despite containing complex TOML manipulation logic. This is a significant gap.

### (6) Logic bugs / edge cases
- **Lines 186-210:** The TOML stripping logic for `[[permissions.whitelist]]` blocks is fragile:
  - Line 193: If a whitelist entry contains an empty line (e.g., between fields), the stripping stops prematurely.
  - Line 199: `if line.starts_with('[')` — any line starting with `[` is treated as a new section, but a multi-line array value like `commands = [\n  "git"\n]` would have lines not starting with `[` that get skipped. Actually, looking more carefully: the logic skips lines while `in_whitelist` is true, stopping at empty lines or `[`-prefixed lines. But TOML array values can span multiple lines with `[` inside them. This could corrupt the config file.
  
  **Recommendation:** Use a proper TOML parser/writer (e.g., `toml_edit`) instead of line-by-line string manipulation.

- **Line 155:** `format!("tool_pattern = \"{}\"\n", entry.pattern.tool_pattern)` — if the tool pattern contains `"` or `\`, the TOML output will be malformed. No escaping.
  **Recommendation:** Escape the string properly or use a TOML serializer.

- **Line 94-111:** `purge_expired` removes `Once` entries unconditionally. But as noted in `types.rs` analysis, `Once` entries should be allowed to match once before being purged. The `query` method calls `purge_expired` first (line 65), so `Once` entries are purged before they can ever be queried. **This confirms the Once-scope bug.**

- **Line 48-51:** `add` deduplicates by `tool_pattern + scope`. If you add a `Once` entry and a `Session` entry for the same tool, both are kept. ✅

### (7) Code organization
- The TOML manipulation in `persist_to_config` should be extracted and tested separately.

---

## 15. `permission/audit.rs`

### (1) `unwrap()`/`expect()` in production paths
None. All `unwrap()` calls (lines 176, 178, 200, 208, 210, 240) are in `#[cfg(test)]`.

### (2) Blocking I/O in async functions
- **Line 25:** `std::fs::create_dir_all(parent)` — in `AuditLog::new` (sync constructor). ✅
- **Line 79:** `std::fs::read_to_string(&self.path)` — in `read_all` (sync fn).
- **Line 112-116:** `std::fs::OpenOptions::new()...open()` + `writeln!` — in `append_line` (sync fn).
- **Line 133:** `std::fs::write(&self.path, ...)` — in `trim_if_needed` (sync fn).
- **Line 157:** `std::env::var("HOME")` — in `expand_tilde` (sync fn).

The `record` method is sync and does file I/O. If called from an async context (which it is, via `audit_record` in `mod.rs:707`), it blocks the runtime.

**Recommendation:** Make `record` async or wrap in `spawn_blocking`. Alternatively, use a background writer task with a channel.

### (3) Lock patterns
None (no interior mutability — `&self` methods do file I/O directly).

### (4) Error handling gaps
- **Line 59:** `eprintln!` on serialization failure — should be `tracing::error!`.
- **Line 65:** `eprintln!` on write failure — should be `tracing::error!`.
- **Line 70:** `let _ = self.trim_if_needed();` — trim failure silently ignored. If the log grows unbounded because trimming fails, there's no feedback.
- **Line 83:** `serde_json::from_str::<AuditEntry>(line).ok()` — malformed JSONL lines silently skipped. No logging of how many lines were skipped.

### (5) Missing test coverage
**2 tests** — record/read and stats. Missing:
- `trim_if_needed` test (exceeding max_entries)
- `truncate` function test (multibyte chars — `&s[..max_len]` can panic on non-char-boundary!)
- `expand_tilde` test
- Concurrent writes test
- Malformed JSONL recovery test

### (6) Logic bugs / edge cases
- **Line 151:** `format!("{}...<truncated>", &s[..max_len])` — **`&s[..max_len]` will panic if `max_len` is not on a UTF-8 char boundary.** The `truncate` function is called at line 48 with `max_len=500`. If byte 500 falls in the middle of a multibyte character, this panics.
  **Recommendation:** Use `floor_char_boundary` or `s.char_indices().take_while(|(i, _)| *i <= max_len).last()` to find a safe boundary.

- **Line 100:** `let asked = total - allowed - denied;` — this assumes every entry is either Allow, Deny, or Ask. If a new `ApprovalLevel` variant is added, this would underflow. Using `filter(|e| e.decision == ApprovalLevel::Ask).count()` would be safer.

- **Line 121-135:** `trim_if_needed` reads all entries, skips the excess, and rewrites the entire file. This is O(n) on every record after the limit is reached. For `max_entries=10000`, every write after 10000 entries reads and rewrites 10000 lines.
  **Recommendation:** Use a ring buffer or only trim when the file size exceeds a threshold.

- **Line 25:** `std::fs::create_dir_all(parent)` — if the parent path is a file (not a directory), this fails with a confusing error.

### (7) Code organization
- `expand_tilde` is duplicated from `mod.rs:739`. **Recommendation:** Extract to a shared utility.
- `truncate` should handle char boundaries.

---

## 16. Priority Summary

### Critical ( correctness / security )
| Priority | File:Line | Issue |
|---|---|---|
| 🔴 P0 | `subagent.rs:652-657` | Process-global CWD change races with concurrent subagents — corrupts working directories |
| 🔴 P0 | `audit.rs:151` | `&s[..max_len]` panics on non-char-boundary in `truncate()` |
| 🔴 P0 | `whitelist.rs:94-111` | `Once`-scoped whitelist entries are purged before they can ever be used (dead feature) |
| 🔴 P0 | `whitelist.rs:186-210` | TOML stripping logic corrupts multi-line array values in config.toml |
| 🟠 P1 | `types.rs:478` | `glob_match` doesn't escape regex metacharacters (`+`, `(`, `)`, etc.) — silent match failures |
| 🟠 P1 | `webfetch.rs:142,162` | `std::sync::Mutex::lock().unwrap()` panics on poisoning |
| 🟠 P1 | `grep.rs` | No symlink loop protection in recursive `search_path` |

### High ( blocking I/O in async )
| Priority | File:Line | Issue |
|---|---|---|
| 🟠 P1 | `edit.rs:49,65` | `std::fs::read_to_string`/`write` in async `execute` |
| 🟠 P1 | `read_file.rs:97,109,138` | Entire file reading pipeline uses synchronous `std::io` in async |
| 🟠 P1 | `write_file.rs:55,60` | `std::fs::create_dir_all`/`write` in async `execute` |
| 🟠 P1 | `grep.rs:92,108` | Synchronous recursive directory search in async |
| 🟠 P1 | `subagent.rs:704,710` | `std::fs::read_to_string` for persona files in async |
| 🟠 P1 | `audit.rs:112,133` | Synchronous file I/O in `record()` called from async path |

### Medium ( error handling / logic )
| Priority | File:Line | Issue |
|---|---|---|
| 🟡 P2 | `bash.rs:178,190,263,271` | Read errors silently ignored with `let _ =` |
| 🟡 P2 | `bash.rs:170,263` | `next_line()` errors silently terminate stdout streaming |
| 🟡 P2 | `grep.rs:84` | Invalid glob filter silently falls back to `"*"` (matches everything) |
| 🟡 P2 | `grep.rs:92` | Non-UTF-8/unreadable files silently skipped |
| 🟡 P2 | `read_file.rs:124` | Probe read error swallowed by `unwrap_or(0)` |
| 🟡 P2 | `edit.rs:65` | Non-atomic write — file corruption risk on write failure |
| 🟡 P2 | `mod.rs:618` | Sandbox canonicalization failure silently falls back to raw path |
| 🟡 P2 | `mod.rs:874-931` | `is_readonly_command` doesn't call `normalize_command` (inconsistent with `is_destructive_command`) |
| 🟡 P2 | `mod.rs:910` | `arg.contains("..")` false-positives on legitimate filenames |
| 🟡 P2 | `rules.rs:52-70` | `starts_with` prefix matching allows `ls` to match `lsxyz` |

### Low ( code quality / testing )
| Priority | File:Line | Issue |
|---|---|---|
| 🟢 P3 | Multiple | `floor_char_boundary` duplicated 3x (`read_file.rs:22`, `subagent.rs:519`, std in `webfetch.rs:323`) |
| 🟢 P3 | Multiple | `expand_tilde` duplicated 2x (`mod.rs:739`, `audit.rs:155`) |
| 🟢 P3 | `whitelist.rs:40`, `audit.rs:59,65` | `eprintln!` should be `tracing::warn!`/`error!` |
| 🟢 P3 | `bash.rs`, `edit.rs`, `write_file.rs`, `grep.rs`, `webfetch.rs`, `subagent.rs`, `mod.rs` | No unit tests |
| 🟢 P3 | `whitelist.rs:137-220` | `persist_to_config` — 80+ lines of untested TOML manipulation |
| 🟢 P3 | `webfetch.rs` (850+ lines) | Should be split into submodules |
| 🟢 P3 | `permission/mod.rs` (1194 lines) | Command analysis functions should be extracted |
| 🟢 P3 | `rules.rs:154-237` | Legacy `default_rules()` duplicates `default_rules_with_danger()` — should be `#[deprecated]` |
| 🟢 P3 | `bash.rs:163-233` vs `256-308` | Supervised and legacy paths share ~40 lines of duplicated logic |
| 🟢 P3 | `webfetch.rs:487` | Unused variable `_meta_start` (dead code) |
| 🟢 P3 | `bash.rs:104` | No upper bound on `timeout_secs` (contrast with `webfetch.rs:238` which clamps to 120) |
