I'll read all the tool files in parallel.
Let me read the remaining portions of the larger files.
Let me read the remaining portions of webfetch.rs and subagent.rs.
Now let me check the ProcessSupervisor implementation to understand bash's supervised path, and the `redirect_if_artifact` paths function used by multiple tools.
Now I have a complete picture. Here is my thorough analysis of every tool implementation.

---

# Complete Tool Implementation Analysis: `core/src/tools/`

## Files Analyzed (all lines read)

| File | Lines | Tools Defined |
|------|-------|---------------|
| `bash.rs` | 1–310 | `bash` |
| `edit.rs` | 1–111 | `edit` |
| `read_file.rs` | 1–376 | `read_file` |
| `write_file.rs` | 1–69 | `write_file` |
| `grep.rs` | 1–132 | `grep` |
| `glob.rs` | 1–141 | `glob` |
| `webfetch.rs` | 1–865 | `webfetch` |
| `tavily_search.rs` | 1–112 | `tavily_search` |
| `subagent.rs` | 1–838 | `subagent`, `subagents` |
| `archival_memory.rs` | 1–180 | `archival_memory_insert`, `archival_memory_search`, `archival_memory_delete` |
| `core_memory.rs` | 1–189 | `core_memory_append`, `core_memory_replace`, `core_memory_read` |
| `recall_memory.rs` | 1–145 | `conversation_search`, `conversation_search_date` |
| `skill.rs` | 1–239 | `skill_list`, `skill_load`, `skill_deactivate`, `skill_reload` |
| `todo.rs` | 1–161 | `todo_write`, `todo_read`, `todo_update` |
| `mod.rs` | 1–323 | (registry, trait, factory, helpers) |

Supporting files also examined: `paths.rs` (lines 45–69, `redirect_if_artifact`), `runtime/supervisor.rs` (lines 104–303, `ProcessSupervisor`).

---

## 1. `bash.rs` — BashTool

### (1) Purpose
Execute a bash shell command via `sh -c` and return stdout+stderr. Supports optional streaming progress callbacks and process-group supervision for clean cancellation.

### (2) Parameters Schema (lines 64–83)
```json
{
  "command": {"type": "string", "required": true},
  "working_dir": {"type": "string"},
  "timeout_secs": {"type": "integer"}
}
```
Defaults: `working_dir` → `default_working_dir` or `"."`; `timeout_secs` → 60.

### (3) Error Handling Quality — Moderate
- **Good**: Timeout via `tokio::time::timeout` (lines 106–111) with context message.
- **Good**: Spawn errors captured with `.context("failed to spawn command")` (legacy, line 251) or propagated from `supervisor.spawn_bash` (supervised, line 144).
- **Bug — non-zero exit is NOT an error** (lines 229–231, 304–306): When `exit_code != 0`, the tool returns `Ok(result)` with an `[exit code: N]` suffix. This is arguably intentional (the LLM sees the error text), but it means downstream error-handling logic that checks `Result::is_err()` will treat a failed command as success. The entire result is returned as `Ok`.
- **Bug — read errors silently swallowed**: Lines 178 and 190 (`let _ = stdout.read_to_end(&mut buf).await;` / `let _ = stderr.read_to_end(...)`): I/O errors reading stdout/stderr are silently discarded. If the pipe is broken or an error occurs, you get an empty string with no indication of the failure.
- **Bug — streaming path loses stderr**: In the streaming `on_update` path (lines 166–174), only stdout lines are streamed. Stderr is still collected separately and appended at the end, which is fine — but if stdout streaming hits an error mid-way (`lines.next_line()` returns `Err`), the `while let Ok(Some(line))` loop silently terminates, truncating output.
- **Bug — exit code -1 ambiguity** (supervised path, line 206): If the child "disappeared" from the supervisor (line 205), `try_exit_code()` returns `Some(-1)`, but a process killed by signal also typically yields -1 (via `code().unwrap_or(-1)`). The wait loop polls every 50ms (line 212), which is a busy-wait with unnecessary latency.

### (4) Security Concerns — HIGH RISK
- **Command injection by design**: This tool's entire purpose is to run arbitrary shell commands. The command string is passed directly to `sh -c` (legacy, lines 243–245) or `supervisor.spawn_bash` → `sh -c` (supervisor, supervisor.rs line 126). There is **no sandboxing, no command allowlist, no working directory confinement, no network egress filtering**. Any agent with `bash` access has full shell access to the host.
- **No PATH/env sanitization**: The environment is inherited as-is from the parent process. A malicious `PATH` or `LD_PRELOAD` in the environment would affect all commands.
- **`working_dir` is unvalidated** (lines 99–103): The LLM can specify any directory, including system directories. No path traversal protection.
- **`timeout_secs` has no upper bound** (line 104): `args["timeout_secs"].as_u64().unwrap_or(60)` — the LLM can pass `timeout_secs: 999999` and hold a process for ~11 days. No clamping like webfetch does.
- **Stdin is piped in supervised path** (supervisor.rs line 129) but never used/written. This is harmless but means the child could hang if it reads stdin (e.g., `cat` with no input will hang until timeout).

### (5) Performance Issues
- **Busy-wait polling** (supervised path, lines 199–213): The `wait_fut` polls `try_exit_code()` every 50ms in a loop instead of using an async `child.wait()`. This is wasteful and adds up to 50ms latency to every command.
- **Three separate mutex acquisitions** for spawn + take_stdout + take_stderr (lines 142–161): Each acquires `sup.lock()` separately. Under contention this is 3 lock/unlock cycles.
- **Full stdout buffered in memory** when not streaming (lines 177–179): `read_to_end` into a `Vec<u8>` — for commands producing large output, this loads the entire output into memory.
- **No output size limit**: Unlike `read_file` (24K char cap) or `webfetch` (80K char cap), bash has **no output truncation**. A command like `cat /dev/urandom | head -c 1G` would exhaust memory.

### (6) Bugs / Logic Errors
- **Line 221 — kill after completion**: After `tokio::join!` completes (child is done), the code calls `supervisor.kill(&child_id)` (line 221). This is a no-op if the child already exited, but if the child is still finishing (race between stdout EOF and process exit), this could SIGKILL a process that was about to exit cleanly, potentially losing final stderr output.
- **Legacy path `kill_on_drop(true)`** (line 249) vs supervised path `kill_on_drop(false)` (supervisor.rs line 132): Inconsistent lifecycle management. The legacy path relies on tokio's drop semantics; the supervised path relies on explicit `kill()`.
- **Timeout doesn't kill the process** (lines 106–111): `tokio::time::timeout` cancels the future, which drops `run_bash`, which drops the child handle. In the legacy path, `kill_on_drop(true)` ensures the child dies. But in the **supervised path**, the child is owned by the `ProcessSupervisor` (not by `run_bash`), so dropping the future does NOT kill the child. The child continues running as an orphan until `kill_all()` is called externally or the supervisor is dropped. **This is a process leak on timeout in the supervised path.**

---

## 2. `edit.rs` — EditTool

### (1) Purpose
Modify an existing file by finding an exact `old_string` and replacing it with `new_string`. Generates a unified diff and reports the edited line range.

### (2) Parameters Schema (lines 18–28)
```json
{
  "file_path": {"type": "string", "required": true},
  "old_string": {"type": "string", "required": true},
  "new_string": {"type": "string", "required": true}
}
```

### (3) Error Handling Quality — Good
- **Good**: Checks for 0 matches (line 53) and >1 matches (line 56) with clear error messages.
- **Good**: Read errors include the resolved path (line 50).
- **Missing**: No write-error context (line 65) — `std::fs::write(&resolved_path, &new_content)?` uses `?` without `.with_context()`, so a permission error gives a bare OS error.
- **Missing**: No check for `old_string == new_string` — a no-op edit will succeed silently, wasting a turn.

### (4) Security Concerns — Low
- **No path validation**: `file_path` can be any absolute path. The LLM can edit `/etc/passwd` if the process has permissions. The `_session_id` artifact redirect (lines 41–46) only applies to specific filenames (plan.md, images, etc.), not general path traversal protection.
- **No atomic write**: `std::fs::write` (line 65) is not atomic — if the process crashes mid-write, the file could be corrupted. Should use write-to-temp-then-rename pattern.

### (5) Performance Issues — Minimal
- **Full file read into memory** (line 49): For very large files (up to the 1MB limit in read_file, but edit has **no size limit at all**), this could be expensive. Edit has no file size guard.
- **Diff computation** (lines 74–80): Uses `similar::TextDiff::from_lines` which is O(N×M) worst case for the full file. For large files this is slow.

### (6) Bugs / Logic Errors
- **Line 69 — `unwrap_or(0)` on find**: `old_content.find(old_string).unwrap_or(0)` — if the find fails (which shouldn't happen after the count check at line 52), it defaults to byte offset 0, reporting the wrong line range. This is dead code in practice (count > 0 guarantees find succeeds) but is misleading.
- **Line 71 — line_end calculation**: `line_end = line_start + old_string.matches('\n').count()`. If `old_string` doesn't end with a newline, `line_end` points to the line containing the last character of `old_string`, which is correct. But if `old_string` is `"foo\n"`, `line_start` is the line with `foo`, and `line_end = line_start + 1`, which points to the *next* line — slightly misleading in the UI message "Edited lines L5–L6" when the edit was only on line 5.
- **No file locking**: Concurrent edits to the same file from subagents could cause lost updates (read-modify-write race).
- **`old_string` can match across content that includes the line-number prefix**: The description in `read_file` warns to strip the `N\t` prefix, but if the LLM forgets, the edit will silently fail to find the string. No helpful error about possible line-number prefix contamination.

---

## 3. `read_file.rs` — ReadFileTool

### (1) Purpose
Read a text file with line numbers, supporting pagination via offset/limit, binary detection, UTF-8 validation, and output size capping.

### (2) Parameters Schema (lines 50–71)
```json
{
  "path": {"type": "string", "required": true},
  "offset": {"type": "integer", "minimum": 1},
  "limit": {"type": "integer", "minimum": 1}
}
```
Constants: `MAX_FILE_SIZE_BYTES = 1_048_576` (1MB), `MAX_LINES_DEFAULT = 300`, `MAX_OUTPUT_CHARS = 24_000`.

### (3) Error Handling Quality — Excellent
- **Good**: Size guard before opening (lines 97–105).
- **Good**: Binary detection via NUL byte probe (lines 121–127).
- **Good**: UTF-8 validation via `BufRead::read_line` (line 138–140).
- **Good**: Explicit offset/limit validation (lines 81–86).
- **Good**: Empty file detection (line 179).
- **Good**: Comprehensive test suite (lines 225–376) covering all edge cases.
- **Minor**: Line 124 `reader.read(&mut probe).unwrap_or(0)` — if the read fails (not EOF), it silently treats it as 0 bytes and proceeds, which could skip the binary check. Should propagate the error.

### (4) Security Concerns — Low
- **No path validation**: Can read any file the process has permissions for (`/etc/shadow`, `~/.ssh/id_rsa`, etc.). No path confinement to workspace.
- **Symlink following**: `std::fs::metadata` and `std::fs::File::open` follow symlinks by default. A symlink could point outside the workspace.
- **TOCTOU**: `metadata()` check (line 97) and `File::open` (line 109) are separate calls — a file could be replaced (e.g., with a symlink to a larger file or a FIFO) between the two calls.

### (5) Performance Issues — Good
- **Good**: Streams lines rather than loading the whole file (though it does seek back after the binary probe, line 130–132).
- **Good**: Character budget prevents unbounded output.
- **Minor**: The binary probe reads 8192 bytes then seeks back (lines 122–132). For small files, this means reading the beginning twice.
- **Minor**: Lines before `offset` are still read into memory via `read_line` (line 137–148) — for a file with offset=10000, it reads and discards 9999 lines. Could use `BufReader::seek` or skip more efficiently, though for text files with variable line lengths this is hard to avoid.

### (6) Bugs / Logic Errors
- **Line 158 — format string width**: `format!("{:>6}\t{}\n", line_num, trimmed)` — right-aligns line numbers to 6 digits. For files with >999,999 lines, the formatting breaks alignment but still works.
- **Line 186–190 — `last_emitted` fallback logic**: If `last_emitted >= start`, use `last_emitted`; else use `start`. The comment says "file had exactly one matching line (or hit a cap after one line)" but the actual condition for `last_emitted < start` is that no lines were collected (`last_emitted` stays 0). In that case, the header says `[Lines {offset}-{offset}]` with an empty body — see test at line 339 which confirms this behavior.
- **Line 199–200 — continuation logic**: `stopped_early = collected >= limit || hit_char_cap` and `more_after = line_num > end`. If `hit_char_cap` is true but `more_after` is false (single huge line), it falls into the `else if` branch which requires `stopped_early && more_after` — this is false, so no continuation hint. But the `hit_char_cap` branch at line 201 handles this correctly. The logic is correct but convoluted.

---

## 4. `write_file.rs` — WriteFileTool

### (1) Purpose
Create or overwrite a file with given content. Creates parent directories if needed.

### (2) Parameters Schema (lines 19–34)
```json
{
  "path": {"type": "string", "required": true},
  "content": {"type": "string", "required": true}
}
```

### (3) Error Handling Quality — Minimal
- **Good**: Parent directory creation with context (lines 53–57).
- **Good**: Write error with context (lines 60–61).
- **Missing**: No file size limit — can write arbitrarily large files.
- **Missing**: No check if the path is a directory (would get an OS error, but no friendly message).

### (4) Security Concerns — Low
- **No path validation**: Can write to any path. Same concern as edit — no workspace confinement.
- **No atomic write**: Non-atomic write means crash mid-write = corrupted file.
- **Creates directories recursively** (line 55): `create_dir_all` will create arbitrary directory structures anywhere on the filesystem.

### (5) Performance Issues — Minimal
- Content is already in memory (from JSON args), so write is a single syscall. No issues.

### (6) Bugs / Logic Errors
- **Line 64 — `content.len()` reports bytes, not chars**: The success message says "wrote N bytes" using `content.len()` which is byte length. This is correct for `std::fs::write` but could confuse the LLM if it's thinking in characters.
- **No symlink protection**: If `path` is a symlink, `std::fs::write` will follow it and overwrite the target.

---

## 5. `grep.rs` — GrepTool

### (1) Purpose
Search file contents using regex, returning matching lines with file paths and line numbers. Recursive directory search with filtering.

### (2) Parameters Schema (lines 19–42)
```json
{
  "pattern": {"type": "string", "required": true},
  "path": {"type": "string"},
  "include": {"type": "string"},
  "max_results": {"type": "integer"}
}
```
Defaults: `path` → `"."`, `max_results` → 50.

### (3) Error Handling Quality — Moderate
- **Good**: Invalid regex reported with context (lines 50–51).
- **Good**: Directory read errors reported (line 109).
- **Missing**: `read_to_string` errors silently ignored (line 92) — `if let Ok(content)` means non-UTF-8 files or permission-denied files are silently skipped with no indication.
- **Missing**: `entry.file_type()` errors silently propagated (line 116) — a single broken symlink entry will abort the entire search.

### (4) Security Concerns — Low
- **No path confinement**: Can search any directory.
- **No symlink loop protection**: `entry.file_type()` (line 116) follows symlinks for the type check. A symlink loop could cause infinite recursion. However, `file_type()` on a symlink returns the symlink type, not the target, so this may be safe. Actually — `std::fs::DirEntry::file_type()` does NOT follow symlinks (it uses `lstat`), so symlinks to directories would be classified as files, not dirs, and wouldn't be recursed into. This is actually safe.

### (5) Performance Issues — Significant
- **Full file read into memory** (line 92): `std::fs::read_to_string(path)` loads the entire file. No streaming, no size limit. A 1GB file would exhaust memory.
- **No `.gitignore` support**: Only skips `.`, `target`, `node_modules` (line 121). Will search through `vendor/`, `.cache/`, build artifacts, etc.
- **Synchronous I/O in async context**: All file operations are `std::fs` (blocking) called from an async function. This blocks the tokio runtime thread. Should use `tokio::fs` or `spawn_blocking`.
- **Limited directory skip list** (line 121): Only `target` and `node_modules`. Missing common directories like `.git`, `dist`, `build`, `__pycache__`, `.venv`, `vendor`.

### (6) Bugs / Logic Errors
- **Line 83–84 — `glob::Pattern::new(ext_filter).unwrap_or(glob::Pattern::new("*").unwrap())`**: If the user provides an invalid glob pattern for `include`, it silently falls back to `*` (matches everything) instead of reporting the error. The second `unwrap` is safe (`*` always parses) but the silent fallback is misleading.
- **Line 86 — `let pattern =` shadowing**: The variable `pattern` at line 83 shadows the `pattern` parameter name, but they're in different scopes. Not a bug but confusing.
- **Line 97 — `regex.is_match(line)`**: Uses `is_match` which finds a match anywhere in the line, not `find` with position. This is correct for grep semantics but means the result doesn't highlight WHERE the match is.
- **Line 102 — `line.trim()`**: Matching lines are trimmed before output, which could remove meaningful leading whitespace (e.g., in Python/YAML files where indentation is significant). The line number and file path are preserved, but the content is altered.
- **`max_results` counts matching lines, not files**: If a single file has 50 matches, the search stops and never looks at other files. This could be surprising.

---

## 6. `glob.rs` — GlobTool

### (1) Purpose
Search for file paths matching a glob pattern, with base directory and result limit.

### (2) Parameters Schema (lines 20–39)
```json
{
  "pattern": {"type": "string", "required": true},
  "path": {"type": "string"},
  "max_results": {"type": "integer"}
}
```
Defaults: `path` → `"."`, `max_results` → 100.

### (3) Error Handling Quality — Moderate
- **Good**: Invalid glob pattern reported (lines 56–59).
- **Good**: Test coverage (lines 90–141).
- **Missing**: Errors from individual path entries are silently skipped (lines 76–78) — `Err(_) => continue`. Permission errors, broken symlinks, etc. are invisible.

### (4) Security Concerns — Low
- **No path confinement**: Can glob any directory on the filesystem.
- **`std::env::current_dir().unwrap_or(...)` on line 69**: If `current_dir()` fails (e.g., directory was deleted), it falls back to `.`. The `strip_prefix` logic for display is best-effort.

### (5) Performance Issues — Moderate
- **Synchronous I/O in async context**: `glob::glob` uses blocking filesystem calls in an async function.
- **No `.gitignore` support**: Returns all matching files including those in `.git/`, `target/`, etc.
- **`max_results` truncates but doesn't short-circuit glob iteration efficiently**: The `glob` crate's iterator will still traverse the directory tree; only the results are limited (line 62–64).

### (6) Bugs / Logic Errors
- **Line 48 — absolute pattern handling**: `if Path::new(pattern).is_absolute()` — on Unix, a pattern like `/foo/*/bar` is detected as absolute and used directly. But on Windows, `C:\foo\*` would also be detected. The `base.join(pattern)` for relative patterns is correct.
- **Line 67–73 — display path logic**: Tries to strip the base prefix, then falls back to stripping `current_dir()`, then falls back to the full path. If the base is `.` and the pattern is `**/*.rs`, the paths will be relative and `strip_prefix(".")` works. But if `base` is an absolute path and results are under a different prefix, the full path is shown. This is correct but the three-level fallback is fragile.
- **Line 69 — `unwrap_or_else` panic risk**: `std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))` — this doesn't panic (it uses `unwrap_or_else`), but `strip_prefix(Path::new("."))` on a path like `/foo/bar/baz.rs` would fail, falling through to the else branch. This is correct.

---

## 7. `webfetch.rs` — WebFetchTool (CRITICAL)

### (1) Purpose
Fetch and extract readable content from a web page. Includes browser fingerprint emulation (rotating User-Agents with matching Client Hints), per-domain rate limiting, robots.txt compliance, and content extraction (Readability + HTML-to-Markdown).

### (2) Parameters Schema (lines 215–231)
```json
{
  "url": {"type": "string", "required": true},
  "timeout": {"type": "number", "default": 30}
}
```

### (3) Error Handling Quality — Excellent
- **Good**: Timeout clamped to 1.0–120.0 (line 238).
- **Good**: HTTP errors return informative messages with suggestions (lines 742–865, `format_http_error` and `format_http_status`).
- **Good**: Binary content detection via content-type (lines 304–310).
- **Good**: Short content warning (lines 625–634) — flags JS-rendered pages.
- **Good**: Markdown conversion fallback to plain text (lines 636–642).
- **Good**: Truncation at 80K chars with floor_char_boundary (lines 322–332).
- **Missing**: robots.txt fetch errors are silently ignored (lines 446–453) — this is actually correct behavior (fail open), but means a misconfigured robots.txt can't block access.

### (4) Security Concerns — Moderate
- **SSRF risk**: The URL is not validated against internal IPs or domains. The LLM can fetch `http://localhost:8080/admin`, `http://169.254.169.254/latest/meta-data/` (AWS metadata), `http://192.168.1.1/`, etc. This is a **server-side request forgery (SSRF) vulnerability**. There is no allowlist/denylist for internal addresses.
- **`upgrade_to_https` (lines 726–732)**: Only upgrades `http://` to `https://`. Does NOT handle other schemes (e.g., `file://`, `ftp://`, `gopher://`). The `reqwest` client by default only supports http/https, so this is safe, but the function name is misleading.
- **URL not validated**: `url::Url::parse` is called only for robots.txt (line 262) and rate limiting (line 139). The actual fetch uses the raw string. An invalid URL would result in a reqwest error, which is handled.
- **Cookie jar is shared across all requests** (line 184, 253): The `Jar` is shared across all webfetch calls. Cookies from one site could be sent to another if the jar doesn't properly scope them. The `reqwest::cookie::Jar` does scope by domain, so this is safe.
- **Browser fingerprint spoofing** (lines 14–107): The tool deliberately impersonates real browsers with rotating User-Agents, Client Hints, Sec-Fetch headers, etc. This is designed to bypass bot detection. **This is ethically questionable** and could violate ToS of target websites.
- **No redirect domain validation**: Redirects are followed up to 8 times (line 257) without checking if the redirect target is an internal address. An external site could redirect to an internal service (SSRF via redirect).

### (5) Performance Issues
- **New HTTP client per request** (lines 250–259): `reqwest::Client::builder()...build()` is called on every `execute()`. Client construction includes TLS setup, connection pool initialization, etc. Should reuse a single client.
- **Rate limiter uses `std::sync::Mutex`** (line 142, 162): `self.last_access.lock().unwrap()` — uses std Mutex (blocking) in async context. Under contention this blocks the tokio thread. Should use `tokio::sync::Mutex` or a lock-free approach.
- **`extract_meta` allocates a full lowercase copy** (line 486): `let lower = html.to_lowercase()` — for large HTML pages (common), this doubles memory. Could use case-insensitive searching.
- **`strip_html_tags` fallback** (lines 665–720): Character-by-character processing with string accumulation. For large pages this is slow but only used as a fallback when htmd fails.
- **Robots.txt fetched on every request** (lines 436–470): No caching of robots.txt. Every fetch to the same domain re-fetches `/robots.txt`. Should cache per-domain.

### (6) Bugs / Logic Errors
- **Line 263 — robots.txt check uses the wrong URL for matching**: `matcher.one_agent_allowed_by_robots(&robots_body, "*", parsed_url.as_str())` — `parsed_url` is the URL after `upgrade_to_https`, which is correct. But `parsed_url.as_str()` includes the full URL, while robots.txt rules are path-based. The `robotstxt` crate should handle this correctly, but this should be verified.
- **Line 460 — `DefaultMatcher::default()`**: Creates a new matcher for every request. This is stateless so it's not a bug, but it's wasteful.
- **Line 463 — User-agent `"*"`**: The comment says "most conservative" but using `*` means the most permissive rules apply. If a site has `User-agent: BadBot` with `Disallow: /` and `User-agent: *` with `Disallow:` (allow all), using `*` would allow access where a specific UA might be blocked. The comment is misleading.
- **Line 487 — `_meta_start` unused**: `let _meta_start = lower.find("<meta ");` — assigned but never used. Dead code.
- **Line 499–500 — scan limit slicing**: `let scan = &html[..scan_limit]` and `let scan_lower = &lower[..scan_limit]` — if the HTML is not valid UTF-8 at the boundary... actually, `html` is already a `&str` so it's valid UTF-8. But if `scan_limit` falls in the middle of a multi-byte character, `&html[..scan_limit]` would panic. However, `html.len().min(16_384)` uses byte length, and slicing at a non-char-boundary panics. **This is a potential panic bug** if the HTML has a multi-byte UTF-8 character spanning the 16KB boundary.
- **Line 509 — `pos = abs_idx + 6`**: After finding `<meta `, pos advances by 6 (length of `<meta `), but `abs_idx` is the position of `<meta ` in the scan string. The next iteration searches `scan_lower[pos..]`. This is correct for skipping past the current match, but the `tag_end` calculation at line 506–508 uses `abs_idx` not `pos`, so it's fine.
- **Line 158 — rate limiter records access AFTER sleeping**: The `wait` function sleeps, then records `Instant::now()` (line 163). But the actual HTTP request happens after `wait` returns, so the recorded time is before the request completes. The next request's delay calculation will use the time from the *start* of the previous request, not its completion. For fast requests this doesn't matter, but for slow requests (30s timeout), the effective delay between requests could be much shorter than intended.

---

## 8. `tavily_search.rs` — TavilySearchTool

### (1) Purpose
Search the web via Tavily API (AI-optimized search engine). Returns synthesized answers and content snippets.

### (2) Parameters Schema (lines 41–52)
```json
{
  "query": {"type": "string", "required": true}
}
```

### (3) Error Handling Quality — Moderate
- **Good**: HTTP send error with context (line 74).
- **Good**: JSON parse error with context (line 77).
- **Good**: Empty results handled (lines 89–91).
- **Missing**: No HTTP status code check — if Tavily returns a 401 (bad API key) or 429 (rate limit), the `.json()` call will likely fail with a parse error, giving a confusing message instead of "invalid API key" or "rate limited".
- **Missing**: No timeout on the HTTP request. `reqwest::Client::new()` (line 16) has no timeout configured.

### (4) Security Concerns — Low
- **API key in request body** (line 61): `"api_key": self.api_key` — the API key is sent in the JSON body, not as a header. This is Tavily's API design, not a bug, but means the key could appear in logs if the request body is logged.
- **API key from env** (line 22): `std::env::var("TAVILY_API_KEY")` — standard practice.

### (5) Performance Issues — Minimal
- **New client per tool instance** (line 16): `reqwest::Client::new()` is created once per `TavilySearchTool` instance, which is fine since the tool is typically a singleton.
- **No response size limit**: The API response could be large, but Tavily limits to 3 results (line 65), so this is bounded.

### (6) Bugs / Logic Errors
- **Line 106–108 — `else` branch for non-array results**: If `response["results"]` is not an array (e.g., the API returns an error object), the code pushes "Failed to extract results from API." to the output. But if `response["answer"]` was also empty, the output is just that error string with no indication of what the API actually returned. Should include the raw response for debugging.
- **No retry logic**: A transient network error or 5xx from Tavily results in an immediate error return.

---

## 9. `subagent.rs` — SubagentSpawnTool & SubagentSpawnAllTool (CRITICAL)

### (1) Purpose
- **`subagent`**: Spawn a single sub-agent with isolated context for a specific task.
- **`subagents`**: Spawn multiple sub-agents concurrently (parallel execution).

### (2) Parameters Schema
**`subagent`** (lines 78–111):
```json
{
  "id": {"type": "string", "required": true},
  "task": {"type": "string", "required": true},
  "system_prompt": {"type": "string"},
  "tools": {"type": "array", "items": {"type": "string"}},
  "max_iterations": {"type": "integer"},
  "result_strategy": {"type": "string", "enum": ["auto", "full", "summary"]}
}
```

**`subagents`** (lines 187–217):
```json
{
  "tasks": {"type": "array", "required": true, "items": {same fields}}
}
```

### (3) Error Handling Quality — Moderate
- **Good**: Missing `tasks` array handled (line 231).
- **Good**: Empty tasks array returns early (line 233).
- **Good**: Join errors captured (line 339–341).
- **Good**: Individual subagent errors don't fail the batch (line 336–338).
- **Missing**: No validation of `id` uniqueness in `subagents` — duplicate IDs would cause confusion in session saving (line 297–300).
- **Missing**: No limit on number of concurrent subagents — the LLM could spawn 100 subagents, exhausting API rate limits and memory.

### (4) Security Concerns — HIGH
- **No recursion depth limit**: A subagent with the `subagent` tool can spawn its own subagents, creating unbounded recursion. Each level consumes memory and API calls. There is **no depth counter or recursion guard**. The `available_tools` are filtered (line 774) but `subagent` is not excluded from the available tools list — a subagent can have the `subagent` tool.
- **Process-wide CWD mutation** (lines 639–657, 810–816): `set_cwd_guard` changes the process working directory using `std::env::set_current_dir`. This is **process-global state** — in a concurrent environment (which `subagents` enables), multiple subagents changing the CWD simultaneously will cause race conditions. The `CwdGuard` restores on drop, but if two subagents run concurrently, one's `set_cwd` can override the other's, and the restore order is nondeterministic. **This is a data race bug.**
- **Subagents inherit all parent tools** (line 765): `available_tools.to_vec()` when `"all"` is specified. This includes memory tools, skill tools, and potentially the `subagent` tool itself.
- **Subagent persona files read from filesystem** (lines 703–712): Reads `~/.agverse/agents/{id}.md` and `.agverse/agents/{id}.md`. The `id` is user-controlled (from LLM). A malicious `id` like `../../etc/passwd` could read arbitrary files. **Path traversal vulnerability** in persona loading.
- **No permission escalation check**: The subagent uses the parent's `permission_config` (line 802), but there's no check that the subagent's tool set is a subset of what the parent is allowed to use.

### (5) Performance Issues — Significant
- **Full subagent LLM runs**: Each subagent makes multiple LLM API calls (up to `max_iterations`). With `subagents` spawning N subagents concurrently, this is N × max_iterations API calls.
- **`max_context_tokens: 32000`** (line 790): Hardcoded. Large contexts could be expensive.
- **Tool summary building** (lines 360–406): Iterates all messages with string allocations and truncations. For long subagent runs, this could be slow but is bounded by message count.
- **JoinSet collects all results before returning** (lines 311–313): No streaming of individual results — the parent waits for ALL subagents to complete before getting any output.

### (6) Bugs / Logic Errors
- **Lines 810–816 — CWD guard in concurrent path**: As noted above, `set_cwd_guard` is called inside `spawn_single`, which is called from concurrent tasks in `SubagentSpawnAllTool`. Multiple concurrent CWD changes are a **race condition**. The CwdGuard's Drop restores the original CWD, but "original" was captured at the time of `set_cwd_guard`, which may have already been changed by another concurrent subagent.
- **Line 733 — `max_iterations` parsing**: `args["max_iterations"].as_u64().unwrap_or(parent_max_iterations as u64) as usize` — if the LLM passes `max_iterations: 0`, the subagent gets 0 iterations and can't do anything. No minimum check.
- **Lines 752–762 — "all" wildcard detection**: Checks if `tools` is the string `"all"` or if the first array element is `"all"`. But if `tools` is `["all", "bash"]`, it's treated as "all" and the "bash" is ignored. Also, `tools: "all"` (string instead of array) is handled, but the schema says `tools` is an array — this is a schema/type mismatch.
- **Line 768 — empty tools fallback**: If the agent passes `tools: []` (empty array), `tool_names` is empty, and the code gives `["read_file"]`. But the comment says "Agent explicitly passed empty tools — respect that" — giving `read_file` doesn't respect that.
- **Line 299–300 — session saving race**: In the concurrent path, `mgr.save_subagent(&id, sub_result)` is called inside each spawned task. The `mgr` is an `Arc<Mutex<SessionManager>>` — multiple concurrent saves are serialized by the mutex, which is correct. But the save happens after the result is collected, so if the save fails, the result is still returned (error is ignored with `let _ =`).
- **Line 138 — single subagent session save**: `mgr.save_subagent("subagent", &result)` always uses "subagent" as the ID, not the actual subagent ID from args. This means all single-subagent sessions are saved under the same key, overwriting each other. **Bug.**
- **Line 269–273 — tool list resolution in `subagents`**: `available_tools = if tools.is_empty() { self.available_tools.clone() } else { tools }`. But `tools` here is the per-task tools list. If a task specifies `tools: ["bash"]`, only bash is available. But the `spawn_single` function at line 774 filters: `final_tool_names.retain(|t| available_tools.contains(t))`. Here `available_tools` is the per-task list, not the parent's available tools. So a subagent could get tools the parent doesn't have. Wait — looking more carefully: `spawn_single` receives `available_tools` as `&[String]`, and in the `subagents` path, it's passed `&available_tools` (the per-task resolved list, line 289). But `spawn_single` uses `available_tools` for the filter at line 774 AND for the "all" wildcard at line 765. So if a task says `tools: ["bash", "subagent"]` and the parent has `["bash", "read_file"]`, the filter at line 774 would remove "subagent" (correct). But if a task says `tools: []` (empty), `available_tools` becomes `self.available_tools.clone()` (parent's full list), and then `spawn_single` would give the subagent ALL parent tools including potentially `subagent` itself. **Recursion is possible.**

---

## 10. `archival_memory.rs` — ArchivalMemoryInsertTool, ArchivalMemorySearchTool, ArchivalMemoryDeleteTool

### (1) Purpose
- **Insert**: Store knowledge in archival (long-term) memory.
- **Search**: Search archival memory using keyword search (SQLite FTS5).
- **Delete**: Delete a record by ID.

### (2) Parameters Schemas
**Insert** (lines 31–46): `content` (required), `metadata` (optional JSON string).
**Search** (lines 87–103): `query` (required), `top_k` (default 5).
**Delete** (lines 152–163): `id` (required).

### (3) Error Handling Quality — Good
- **Good**: Uses `try_lock_memory` with 3-second timeout and busy message (lines 52–54, 110–112, 168–170).
- **Good**: Delete returns success=false if not found (line 172–176).
- **Missing**: No content length limit on insert — could store arbitrarily large content.
- **Missing**: `metadata` is accepted as a string but not validated as JSON. The underlying `insert` function may handle this, but the tool doesn't validate.

### (4) Security Concerns — Low
- Memory operations are confined to the MemoryManager's SQLite database. No filesystem path concerns.

### (5) Performance Issues — Low
- All operations go through SQLite, which is fast for typical use.
- **Search uses keyword (FTS5) not semantic** (line 114): The description says "semantic similarity" but the implementation uses `search_by_keyword` (line 114). **Description mismatch bug.**

### (6) Bugs / Logic Errors
- **Line 83–84 — description mismatch**: Description says "Search archival memory using semantic similarity" but line 114 calls `search_by_keyword(query, top_k)`. The comment at line 109 confirms "Pure keyword search via SQLite FTS5 — no embedding model needed." The description is misleading.
- **`top_k` has no upper bound**: The LLM could pass `top_k: 1000000`, potentially causing a large query result set.

---

## 11. `core_memory.rs` — CoreMemoryAppendTool, CoreMemoryReplaceTool, CoreMemoryReadTool

### (1) Purpose
- **Append**: Append content to a core memory block.
- **Replace**: Find-and-replace within a core memory block.
- **Read**: Read a core memory block.

### (2) Parameters Schemas
**Append** (lines 30–45): `block_id` (required), `content` (required).
**Replace** (lines 87–106): `block_id` (required), `old_content` (required), `new_content` (required).
**Read** (lines 155–166): `block_id` (required).

### (3) Error Handling Quality — Good
- **Good**: All use `try_lock_memory` with timeout.
- **Good**: Read returns a JSON error for missing blocks (lines 183–186) instead of an anyhow error.
- **Good**: Append and Replace propagate errors from the underlying memory operations.

### (4) Security Concerns — Low
- Operations are confined to memory blocks. No path concerns.

### (5) Performance Issues — Low
- All operations are in-memory or SQLite-backed.

### (6) Bugs / Logic Errors
- **Replace has no uniqueness check**: Unlike `edit.rs` which checks for multiple matches, `core_memory_replace` delegates to `memory.core_mut().replace(block_id, old_content, new_content)`. If `old_content` appears multiple times, the behavior depends on the underlying implementation (likely replaces first occurrence). No error for multiple matches.
- **Append always returns success** (line 58): Even if the block doesn't exist (the `append` call might create it or error). The response at line 58–63 always returns `success: true`.

---

## 12. `recall_memory.rs` — ConversationSearchTool, ConversationSearchDateTool

### (1) Purpose
- **conversation_search**: Search past conversation history using BM25 keyword search + salience reranking.
- **conversation_search_date**: Search conversation history by date range.

### (2) Parameters Schemas
**Search** (lines 30–46): `query` (required), `top_k` (default 5).
**Date** (lines 97–117): `start_date` (required), `end_date` (required), `top_k` (default 10).

### (3) Error Handling Quality — Good
- **Good**: Both use `try_lock_memory`.
- **Good**: Date format errors would propagate from `search_by_date`.
- **Missing**: No date format validation before passing to the search function.

### (4) Security Concerns — Low
- Confined to SQLite-backed memory.

### (5) Performance Issues — Low
- BM25 search is efficient. Salience reranking adds overhead but is bounded.

### (6) Bugs / Logic Errors
- **Line 26–27 — description says "semantic similarity"**: But the implementation uses BM25 + salience (line 59: `search_conversation_bm25_with_salience`). The comment at lines 52–54 explains this, but the description is misleading — it says "semantic similarity" when it's actually keyword-based.
- **No `top_k` upper bound**: Same as archival memory.

---

## 13. `skill.rs` — SkillListTool, SkillLoadTool, SkillDeactivateTool, SkillReloadTool

### (1) Purpose
- **List**: List all available skills with names, descriptions, triggers, and active status.
- **Load**: Load a skill by name, injecting its content into context.
- **Deactivate**: Deactivate a loaded skill (or all).
- **Reload**: Rescan skill directories and reload manifests.

### (2) Parameters Schemas
- **List**: Empty object.
- **Load**: `name` (required).
- **Deactivate**: `name` (required, "all" special-cased).
- **Reload**: Empty object.

### (3) Error Handling Quality — Good
- **Good**: Load returns a friendly message if skill not found (lines 120–125).
- **Good**: Deactivate handles "all" specially (lines 175–178).
- **Good**: Reload preserves active skills (lines 218–232).
- **Good**: Reload re-activates only skills that still exist (line 229).

### (4) Security Concerns — Low
- Skill content is read from `SKILL.md` files in `.agent/skills/` directories. The skill name is used to look up the skill, not as a path directly, so path traversal via skill name is unlikely (depends on `SkillManager` implementation).

### (5) Performance Issues — Low
- All operations are lightweight (in-memory list operations, filesystem scan for reload).

### (6) Bugs / Logic Errors
- **Line 46 — `self.manager.lock()`**: Uses `parking_lot::Mutex::lock()` which blocks indefinitely. Unlike memory tools that use `try_lock_memory` with a timeout, skill tools have no timeout. If the SkillManager is held by another operation, this blocks forever.
- **Line 116 — `mgr.find_by_name(name)`**: Called before `load_skill_context`, which likely also does a lookup. Double lookup is minor inefficiency.
- **Line 128 — `mgr.activate(name)`**: Called after `load_skill_context` succeeds, but if activation fails (returns false), there's no error — the success message is still returned.

---

## 14. `todo.rs` — TodoWriteTool, TodoReadTool, TodoUpdateTool

### (1) Purpose
- **Write**: Overwrite the entire todo list with new items.
- **Read**: Read the current todo list.
- **Update**: Update a todo item's status.

### (2) Parameters Schemas
- **Write**: `items` (array of strings, required).
- **Read**: Empty object.
- **Update**: `id` (string, required), `status` (enum, required).

### (3) Error Handling Quality — Good
- **Good**: Empty items array rejected (line 57).
- **Good**: Invalid status rejected with error (line 143).
- **Good**: Update errors propagated (line 147–148).

### (4) Security Concerns — Low
- Pure in-memory operations. No filesystem or network concerns.

### (5) Performance Issues — Low
- All operations are in-memory with a mutex.

### (6) Bugs / Logic Errors
- **Line 60 — `self.todo_list.lock()`**: Uses `parking_lot::Mutex::lock()` with no timeout, same as skill tools. Could block indefinitely.
- **Line 157 — `args["status"].as_str().unwrap_or("")`**: After matching `status` to a `TodoStatus` enum variant, the output message re-reads `args["status"]` as a string. If the LLM passed `status: 123` (non-string), the match would have failed at line 137 (`ok_or_else` returns error). But if it's a valid string, this is fine. The `unwrap_or("")` is defensive but unreachable.
- **Line 150 — `list.get(id)`**: Called after `update_status`, which may have moved the item or changed internal state. The `get` should still work if the ID is stable.

---

## 15. `mod.rs` — ToolRegistry, Tool trait, factory

### (1) Purpose
Defines the `Tool` trait, `ToolRegistry` (HashMap-based tool collection), `ToolUpdateFn` type, `try_lock_memory` helper, and `build_tool_by_name` factory.

### (2) Key Implementation Details
- **`try_lock_memory`** (lines 28–40): 3-second timeout with `try_lock_for`, returns a JSON busy message. Good pattern.
- **`validate_args`** (lines 137–152): Uses `jsonschema` crate for validation. Good.
- **`call_one`** (lines 176–220): Validates args, executes tool, converts errors to strings (not `Result`). The error format is `Error executing tool '{name}': {e}`.
- **`canonicalize_json_object`** (lines 307–323): Recursively sorts JSON keys for prompt cache stability. Good optimization.
- **`from_names`** (lines 245–253): Silently skips unknown tool names. Could be surprising.

### (3) Bugs / Logic Errors
- **Line 108–109 — `register` overwrites silently**: `self.tools.insert(tool.name().to_string(), tool)` — if two tools have the same name, the second overwrites the first with no warning.
- **Line 248 — unknown tools silently skipped**: `if let Some(tool) = build_tool_by_name(name)` — unknown names produce no error, just a smaller registry.
- **`build_tool_by_name`** (lines 270–284): Cannot build memory tools, skill tools, todo tools, or subagent tools (they require runtime dependencies). Only builds the 7 core tools. This is documented but means `from_names` can't build a complete registry.

---

## Summary of Critical Issues

### CRITICAL (Security/Data Loss)
1. **webfetch.rs SSRF** (line 233): No internal IP/domain filtering — can fetch `http://169.254.169.254/`, `http://localhost:*`, etc.
2. **subagent.rs recursion** (line 765): No recursion depth limit — subagents can spawn subagents indefinitely.
3. **subagent.rs CWD race** (lines 810–816): Process-global `set_current_dir` in concurrent context causes data races.
4. **subagent.rs path traversal** (lines 703–712): Persona file loading uses unsanitized `id` in path construction.
5. **bash.rs process leak on timeout** (lines 106–111 + supervised path): Timeout in supervised path doesn't kill the child (owned by supervisor, not the future).

### HIGH (Bugs)
6. **subagent.rs line 138**: Single subagent session always saved as "subagent" ID, overwriting previous saves.
7. **webfetch.rs line 499–500**: Potential panic if 16KB scan limit falls on a UTF-8 character boundary.
8. **bash.rs no output size limit**: Unbounded memory usage for large command output.
9. **bash.rs `timeout_secs` unbounded** (line 104): No upper limit, LLM can set extremely long timeouts.

### MEDIUM (Correctness/Performance)
10. **grep.rs synchronous I/O in async**: All `std::fs` calls block the tokio runtime.
11. **glob.rs synchronous I/O in async**: Same issue.
12. **webfetch.rs new client per request** (lines 250–259): Should reuse HTTP client.
13. **webfetch.rs robots.txt not cached**: Re-fetched on every request to the same domain.
14. **webfetch.rs `std::sync::Mutex` in async** (lines 142, 162): Rate limiter uses blocking mutex.
15. **archival_memory.rs description mismatch** (line 84): Says "semantic similarity" but does keyword search.
16. **recall_memory.rs description mismatch** (line 27): Says "semantic similarity" but does BM25 keyword search.
17. **edit.rs non-atomic write** (line 65): Crash mid-write corrupts the file.
18. **edit.rs no file size limit**: Can edit arbitrarily large files.
19. **subagent.rs no concurrent subagent limit**: LLM can spawn unlimited concurrent subagents.

### LOW (Code Quality)
20. **bash.rs exit code -1 ambiguity** (line 206): Signal death and "child disappeared" are indistinguishable.
21. **grep.rs silent error swallowing** (line 92): `read_to_string` errors silently ignored.
22. **glob.rs silent error swallowing** (lines 76–78): Individual path errors silently skipped.
23. **webfetch.rs line 487**: Dead variable `_meta_start`.
24. **skill.rs/todo.rs no lock timeout** (lines 46, 60, etc.): `parking_lot::Mutex::lock()` blocks indefinitely, unlike memory tools which use `try_lock_for`.
