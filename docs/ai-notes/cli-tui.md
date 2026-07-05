I'll read all 14 source files in parallel to analyze them thoroughly.
Now let me read the remaining portions of the larger files that were truncated.
Let me read the remaining portions of the larger files.
Let me verify one specific detail in the markdown.rs file regarding the `find_syntax` function:
Let me verify whether the code compiles, since I noticed a potential type issue in `find_syntax`:
Now let me verify the `find_syntax` type issue I noticed:
I've now read all 14 files completely. Let me verify a couple more details:
I have now read all 14 source files completely. The code compiles successfully (7 warnings for the CLI crate). Here is my thorough, file-by-file analysis.

---

# Complete Analysis of CLI/TUI Source Files

**Verification**: `cargo check` passes with 7 warnings for `agent-cli`. No compilation errors. No `unsafe` code anywhere. All `.unwrap()` calls are listed below with safety analysis.

---

## 1. `main.rs` (~1712 lines)

### (1) Purpose
Entry point for both CLI and TUI modes. CLI mode uses rustyline for line editing with streaming agent output. Handles config loading, interactive setup (permission/hooks/tool-mode prompts), tool registration, MCP connection, session management, and a large command dispatch loop. TUI mode delegates to `tui::run_tui`.

### (2) Key types/structs
- `Args` (line 170): argh CLI args, single `--tui`/`-t` flag
- `CommandCompleter` (line 92): rustyline `Helper` for slash-command tab completion (defined inline, **duplicates** `cli_completer.rs`)
- Color helper functions: `dim`, `bold`, `cyan`, `yellow`, `green`, `red`, `blue`, `magenta`, `reset` (lines 130-158)
- `truncate` (line 160): char-boundary-safe truncation

### (3) Notable patterns
- Config fallback chain: `config.toml` → generate template → `from_env()` (lines 178-191)
- Interactive yes/no prompts for subsystems (lines 324-354)
- Deferred tool output: `Mutex<Option<(String, serde_json::Value)>>` buffers `ToolExecutionStart` until `ToolExecutionEnd` (line 1305)
- Streaming output via callback with `AtomicBool` flags for state tracking (`first_event`, `in_thinking`, `in_agent_text`, `in_sub_thinking`, `in_sub_text`) (lines 1297-1302)
- Graceful shutdown: auto-save session → MCP shutdown → history save (lines 496-531)

### (4) Code quality issues
- **`#![allow(deprecated)]`** (line 1): Globally suppresses deprecated warnings — masks potential issues
- **`.expect()` panics** (lines 383, 464): `Storage::new().expect("Failed to open session DB")` and `Editor::with_config(config).expect("Failed to create line editor")` — will crash on failure with no recovery
- **22 `.unwrap()` on `strip_prefix`** (lines 850, 857, 867, 877, 893, 898, 911, 914, 953, 955, 967, 1022, 1024, 1099, 1109, 1124, 1134, 1153, 1163, 1178, 1188): All guarded by `starts_with` checks upstream, so safe — but fragile under refactoring
- **Unused imports** (compiler warnings): `ApprovalChoice` (line 6), `Cell` (line 17), `HashMap` (line 18), `Ordering` (line 21)
- **Duplicate `CommandCompleter`**: Defined both here (line 92) and in `cli_completer.rs` (line 8) — the `cli_completer.rs` version is never used
- **`let _ = skill_manager.scan()`** (line 358): Silently ignores skill scan errors
- **`let _ = rl.load_history(...)`** (line 466): Silently ignores history load errors
- **`&id[..8]`** (lines 512, 739): Byte-slices session ID — panics if `id.len() < 8` and not char-boundary-safe for non-ASCII IDs

### (5) Performance concerns
- **Per-token `print!` + `flush()`** (line 1378): High I/O syscall overhead during streaming
- **`agent.context_messages()`** (lines 498, 723): Potentially clones all messages on quit/save
- **`pending_tool.lock()`** (lines 1335, 1356, 1386, 1408, 1418, 1442, 1465): Locked on every agent event using `parking_lot::Mutex` — fine for contention but called very frequently

### (6) Bugs / logic errors
- **BUG (lines 437-440)**: Bogus format string — prints `"Memory:      Memory:      enabled"`:
  ```rust
  println!(
      "Memory:      {}",
      "Memory:      enabled"
  );
  ```
  Copy-paste error: the format argument is a literal string instead of just `"enabled"`.
- **Auto-approve via global singleton** (lines 1456-1460): Uses `agent_core::permission::global_pending_approvals()` — a process-global `Mutex<HashMap>`. If multiple agents ran concurrently, approvals could cross-contaminate. Security concern: all tool executions are silently auto-approved.
- **Inconsistent markdown rendering**: CLI mode uses `termimad::MadSkin` (line 1300), TUI mode uses `syntect` — different rendering quality/behavior between modes.
- **TUI mode lacks MCP and session support** (lines 249-298): `run_tui_mode` doesn't register MCP tools or create a `SessionManager`, unlike CLI mode (lines 407-405). TUI users can't use MCP tools or persist sessions.

---

## 2. `cli_completer.rs` (52 lines)

### (1) Purpose
Alternative `CommandCompleter` for rustyline using `extract_word` for word-boundary-aware completion. **Entirely dead code** — never constructed or used anywhere.

### (2) Key types
- `CommandCompleter` (line 8): `#[derive(rustyline::Helper)]` struct with `commands: Vec<String>`

### (3) Notable patterns
- Uses `extract_word` (line 32) for proper word boundary detection — better than main.rs's manual `rfind('/')` approach
- Uses `#[derive(rustyline::Helper)]` macro (line 7) — cleaner than main.rs's manual trait impls

### (4) Code quality issues
- **Entirely dead code**: Compiler confirms "struct `CommandCompleter` is never constructed" and "associated function `new` is never used"
- **Out of sync with `ALL_COMMANDS`**: Missing many commands (`/context`, `/new`, `/rewind`, `/sessions`, `/session`, `/todo`, `/tasks`, `/skills`, `/abort`, `/state`, `/clear-queues`, `/follow-up`, `/steer`, `/tool-mode`, etc.)

### (5) Performance concerns
- N/A (dead code)

### (6) Bugs / logic errors
- If this were used, the command list is incomplete and diverges from main.rs's `ALL_COMMANDS` (line 28). Two sources of truth for commands is a maintenance hazard.

---

## 3. `tui/mod.rs` (317 lines)

### (1) Purpose
TUI entry point and main event loop. Sets up terminal, mouse capture (including motion tracking via raw ANSI), MPSC event channel, 60fps tick timer, and orchestrates rendering, cache rebuilds, command processing, and agent run spawning.

### (2) Key types
- `run_tui` (line 16): async entry, terminal setup/teardown
- `run_app` (line 41): main event loop
- `process_command` (line 197): async command processor with agent access (list_models, switch_model, clear, register_model)
- `update_default_model` (line 301): config.toml line rewriter
- `STREAMING_REBUILD_THROTTLE` (line 14): 50ms cache rebuild throttle

### (3) Notable patterns
- **MPSC event loop**: unbounded channel + separate tick task (lines 56-68)
- **Throttled cache rebuild**: During streaming, rebuild at most every 50ms (lines 159-166)
- **Frame rate limiting**: 16ms min frame, 500ms idle redraw, 4ms sleep when waiting (lines 178-191)
- **Raw ANSI escape codes** for mouse motion tracking: `\x1B[?1003h` / `\x1B[?1003l` (lines 24, 34) — bypasses crossterm

### (4) Code quality issues
- **`std::thread::sleep` in async context** (line 190): Blocks the tokio runtime worker thread. Should use `tokio::time::sleep`. In a single-threaded runtime this would freeze the app.
- **`.unwrap()` on `strip_prefix`** (lines 216, 239): Safe due to `starts_with` guards but fragile
- **`std::fs::read_to_string("config.toml").unwrap_or_default()`** (line 279): If config read fails transiently, uses empty string then appends to it — could clobber existing config on write
- **`update_default_model`** (lines 301-317): Line-based TOML rewriting is fragile — doesn't handle comments, inline tables, or multi-line values. `let _ = std::fs::write(...)` silently ignores write errors.
- **Manual raw escape codes** (lines 24, 34) instead of crossterm commands — terminal-dependent, fragile
- **Comment numbering gap**: Line 109 says "4." but there's no "3." — indicates deleted code section

### (5) Performance concerns
- **60fps tick** (16ms interval, line 61): `AppEvent::Tick` always returns `true` from `apply` (state.rs line 992-993), setting `needs_draw` every 16ms — causes redraw every frame even when nothing changed
- **Busy-poll loop** (lines 76-96): `event::poll(Duration::from_millis(0))` — tight spin loop when events are available
- **Synchronous cache rebuild** (line 168): Happens in the main event loop, could cause frame drops with large conversations

### (6) Bugs / logic errors
- **BUG (line 190)**: `std::thread::sleep(Duration::from_millis(4))` in async function blocks the tokio executor. Should be `tokio::time::sleep`.
- **TUI mode missing MCP/session setup** (lines 249-298): Unlike CLI mode, `run_tui_mode` doesn't connect MCP servers or create `SessionManager`. TUI users lack MCP tools and session persistence.
- **Potential lock contention**: `take_pending_request` spawns a task locking the agent (line 141), while the main loop also locks for commands (lines 113, 203, 217, 231, 260). Under heavy use this could stall the UI.

---

## 4. `tui/state.rs` (1204 lines)

### (1) Purpose
Central TUI application state. Contains all data types (entries, streaming, cache, modal, autocomplete, subagent tracking), the MPSC reducer (`apply`), agent event handling, mouse handling, command dispatch, history navigation, and conversation model types.

### (2) Key types/structs
- `CachedBlock` (line 16): rendered block with `kind`, `wrapped_height`, `subagent_id`, `lines`
- `BlockKind` (line 36): enum (Spacing, User, Thought, Response, Tool, Subagent, Notice, Error, System, Working)
- `CachedConversation` (line 56): render cache with entry_blocks, streaming_blocks, combined blocks, version, width, height
- `CommandMode` (line 93): multi-step input state machine for `/models new`
- `AutocompleteState` (line 128): slash-command autocomplete with options, filtered_options, selected_index
- `ModalState` (line 168): ModelPicker / ModelForm / None
- `AppState` (line 185): main state struct with 30+ fields
- `Entry` (line 1068): System/User/Turn
- `TurnBlock` (line 1075): Thought/Response/Tool/Subagent/Notice/Error
- `ToolResult` (line 1091): text + is_error
- `SubagentState` (line 1098): full subagent tracking with children, timing, activity
- `Streaming` (line 1113): current streaming turn
- `AppEvent` (line 1198): Key/Mouse/Resize/Agent/Tick

### (3) Notable patterns
- **Content versioning**: `content_version` (wrapping_add) + `cache_dirty` + `force_cache_rebuild` for cache invalidation (lines 301-310)
- **Block merging**: Consecutive Thought/Response blocks merged during streaming (lines 812-815)
- **Closure-based updaters**: `do_update`, `updater`, `finalizer` closures for nested subagent state updates — repeated 5+ times with identical structure (lines 634, 848, 888, 922, 950)
- **Input history**: Dedup against last entry + 500-entry cap (lines 313-326)
- **Hover detection**: `find_hovered_subagent` maps mouse row to subagent ID via cache block heights (lines 1042-1061)

### (4) Code quality issues
- **`.unwrap()` on `history_index`** (lines 348, 373): Safe due to logic flow but fragile
- **`.unwrap()` on `strip_prefix("/model ")`** (line 447): Safe due to starts_with guard
- **`Vec::remove(0)`** (line 321): O(n) shift on every history overflow — should use `VecDeque`
- **`#[allow(dead_code)]`** on `BlockKind` (line 35), `Entry` (line 1067), `SubagentState` (line 1097): Suppresses warnings about unused variants/fields
- **Closure duplication**: The search-streaming-then-search-entries pattern is repeated 5+ times (lines 648-660, 871-878, 910-917, 939-946, 963-970) — should be refactored into a helper

### (5) Performance concerns
- **`find_subagent`** (line 380): Linear scan through all streaming blocks + all entries + all turn blocks — O(n*m). Called on every subagent event.
- **`append_subagent_child_block`** (line 847): Updates both streaming AND all entries — double scan
- **`update_subagent_tool_result`** (line 881): Same double-scan pattern
- **`handle_command` known-command list** (lines 459-470): Long `matches!` macro — hard to maintain

### (6) Bugs / logic errors
- **BUG (lines 1118-1126) `truncate_activity`**: Parameter is `max_chars` but comparison uses `cleaned.len()` (byte length). For multi-byte UTF-8 (CJK, emoji), a 50-character string could be 150 bytes, so `cleaned.len() <= 50` would be false and the string would be unnecessarily truncated. Should use `cleaned.chars().count() <= max_chars`.
- **Auto-approve all requests** (lines 681-690): `ApprovalRequired` and `SubagentApprovalRequired` silently auto-approved with `AllowSession`. **Security concern** — no user interaction for any-approved with tool approval.
- **`tool_detail` for SubagentToolEnd** (line 773): `serde_json `AllowSession`. **Security concern** — no user interaction for any tool approval.
- **`tool_detail` for SubagentTool::from_str(&result).unwrap_or_default()` — if result isn't valid JSON (End** (line 773): `serde_json::from_str(&result).e.g., error text), falls back to `Value::Null`, losing activity detail.unwrap_or_default()` — if result isn't valid JSON (e.g Not a crash but information loss.
- **Scroll direction convention**:., error text), falls back to `Value::Null`, losing activity detail. Not a crash but information loss `scroll` is measured.
- **Scroll direction convention**: `scroll` from bottom (0 = newest). `ScrollUp` increases scroll (older), ` is measured from bottom (0 = newest). `ScrollUp` increasesScrollDown` decreases (newer) — scroll (older), `ScrollDown` decreases (new consistent with PageUp/PageDown but potentially confusing.

---

er) — consistent with PageUp/PageDown but## 5. `tui/render.rs` (309 lines)

### (1) Purpose
Layout computation, conversation potentially confusing.

---

## 5. `tui rendering (block-level with visible-block/render.rs` (309 lines)

### (1) Purpose
Layout computation, conversation rendering ( culling), cache rebuilding, scrollbar, and subagent detailblock-level with visible-block c view rendering.

### (2) Key types
- `LayoutAreas`ulling), cache rebuilding, scrollbar, and subagent detail view rendering.

### (2 (line 14): status/main/dropdown/input rects
- `compute_layout` (line 21): calculates layout areas from terminal) Key types
- `LayoutAreas` (line  size + dropdown height
- `dropdown_height` (line 42): computes autocomplete14): status/main/dropdown/input rects
- `compute_layout` (line 21): calculates layout areas from terminal size + dropdown height
- `dropdown dropdown height
- `render` (line 50): top-level render function
- `_height` (line 42): computes autocomplete dropdown height
- `render` (line render_conversation` (line 77): block-level conversation rendering
- `render_blocks` (line 116): shared50): top-level render function
- `render_conversation` (line 77): block-level block rendering with scroll offset
- `rebuild_cache` (line 213 conversation rendering
- `render_blocks` (line 116): shared block rendering with scroll offset
): incremental cache rebuild
- `render_subagent_detail` (line 284): subagent detail- `rebuild_cache` (line 213): incremental cache view

### (3) Notable patterns
- **Visible-block culling**: Only renders blocks within rebuild
- `render_subagent_detail` (line 284): subagent detail view

### (3) the viewport (lines 128-158)
- **Scroll-from-top**: `scroll_from Notable patterns
- **Visible-block culling**: Only renders blocks within the viewport_top = max_scroll.saturating_sub(scroll)` (line  (lines 128-158)
- **Scroll-from-top**: `scroll_from_top82) — scroll measured from bottom
- **Incremental = max_scroll.saturating_sub(scroll)` (line 82) — scroll measured from entry cache**: Entry blocks only rebuilt when count changes (line  bottom
- **Incremental entry cache**: Entry blocks only rebuilt when count219)
- **Working indicator**: Dynamically added to cache when agent running but streaming empty (lines 256-270)

### (4) changes (line 219)
- **Working indicator**: Dynamically added to cache when agent Code quality issues
- **Layout math could produce zero-height main area** (line 26): running but streaming empty (lines 256-270)

### (4) Code quality issues
- **Layout math could produce zero-height `main_bottom.saturating_sub(3)` on very small terminals
- **`as u16` cast** (line 44 main area** (line 26): `main_bottom.saturating_sub): Could theoretically overflow(3)` on very small terminals
- **`as u16` cast** (line 44): Could theoretically but practically impossible
- No error handling — all functions assume valid state

### (5) Performance concerns
- **`rebuild_cache` clones all entry overflow but practically blocks** (line 249): `blocks.extend(state.cache.entry_blocks.iter().cl impossible
- No error handling — all functions assume valid state

### (5) Performance concerns
- **`rebuild_cache` clones all entry blocks** (line 24oned())` — clones every `CachedBlock` (containing `Vec<Line<'static>>`)9): `blocks.extend(state.cache.entry_blocks.iter().cloned())` — clones every ` on every rebuild
- **`render_blocks` allocates** constraints and visible vectors on every frame (CachedBlock` (containing `Vec<Line<'static>>`) on every rebuild
-lines 142-143)
- **`estimate_wrapped **`render_blocks` allocates** constraints and visible vectors on every frame (lines 142-143)
_rows` is O(n²)** (blocks.rs line- **`estimate_wrapped_rows` is O(n²)** (blocks.rs 62): `remaining.chars().count()` called in every while-loop iteration

### (6) Bugs / logic errors
- **BUG (line 219) — Cache invalidation gap**: `rebuild_cache` only rebuilds entry blocks when `rendered_entry_count != entry_count`. If an line 62): existing entry's *content* changes (e.g., a `Tool `remaining.chars().count()` called in every while-loop iteration

### (6) Bugs / logic errors
- **BUG (line 219) — Cache invalidation gap**: `rebuild_cache` only rebuilds entry blocks when `rendered_entry_count != entry_count`. If an existing entry's *content* changes (e.g., aExecutionEnd` updates a tool result on a flushed entry via ` `ToolExecutionEnd` updates ado_update` at line 654), the entry block cache is tool result on a flushed entry via `do_update` at line  NOT rebuilt because the count didn't change. Even though `mark_dirty654), the entry block cache is NOT_force` sets `force_cache_rebuild = true`, `re rebuilt because the count didn't change. Even though `mark_dirty_force` setsbuild_cache` still skips entry `force_cache_rebuild = true`, `rebuild_cache` still skips block rebuild. **Stale tool results will be displayed for flushed entry block rebuild. **Stale entries.**
- **Duplicate tool results will be displayed for flushed entries.**
- **Duplicate scroll clamping**: Both `render_conversation` (line 81) and `render_subagent_detail` (line 300) clamp scroll independently.

---

## 6. `tui/input.rs` (361 lines)

### (1) Purpose
Keyboard event handler. Handles modal navigation, command submission, cursor movement (char/word boundary), text editing (insert/delete/kill), history navigation, and autocomplete interaction.

### (2) Key types
- `handle_key` (line 5): main key handler
- `handle_modal_key` (line 298): modal-specific handler
- Boundary helpers: `prev_char_boundary` (261), `next_char_boundary` (265), `prev_word_boundary` (272), `next_word_boundary` (284)

### (3) Notable patterns
- **Emacs keybindings**: Ctrl+A/E (home/end), Ctrl+K (kill to end), Ctrl+U (kill to start), Ctrl+W (kill word), Ctrl+Left/Right (word movement) (lines 163-241)
- **History navigation exit**: Any text editing exits history nav (lines 187-188, 202-203, 219-220, 226-227, 234-235, 246-247)
- **Autocomplete**: Tab completes, Enter executes directly, Up/Down navigate (lines 45-65, 136-160)

### (4) Code quality issues
- **`.unwrap()` on `strip_prefix("/model ")`** (line 84): Safe but fragile
- **`String::remove`** (line 196): O(n) shift — fine for typical inputs

### (5) Performance concerns
- Boundary helpers are O(n) — fine for typical input lengths

### (6) Bugs / logic errors
- **BUG (lines 177, 180) — Home/End require Ctrl**: `KeyCode::Home | KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL)` — the guard applies to the entire arm, so pressing `Home` alone doesn't match (falls through to `_ => {}`). **Home and End keys do nothing without Ctrl.**
- **BUG (line 208) — Ctrl+Delete missing autocomplete update**: Ctrl+Delete calls `replace_range` but doesn't call `update_autocomplete()`. The non-Ctrl branch (line 212) does. Autocomplete dropdown won't refresh after Ctrl+Delete.
- **BUG (lines 57-66) — Enter with autocomplete active but no valid selection**: If `selected_index >= filtered_options.len()`, the input isn't set and `autocomplete.active` is NOT set to false. The code proceeds to process the current input as a command while autocomplete stays active.
- **Tab with no autocomplete active** (lines 45-56): Falls through to `_ => {}` — Tab does nothing. Not a bug but potentially confusing.

---

## 7. `tui/markdown.rs` (495 lines)

### (1) Purpose
Markdown-to-ratatui-Lines conversion using `pulldown-cmark`. Handles headings, lists, code blocks (with `syntect` syntax highlighting), tables (with column shrinking), blockquotes, inline formatting (bold/italic/strikethrough), inline code, and horizontal rules.

### (2) Key types
- `markdown_to_lines` (line 23): main conversion function
- `render_md_table` (line 214): table renderer with proportional column shrinking
- `render_syntect_block` (line 385): syntax-highlighted code block renderer
- `find_syntax` (line 444): language name → syntect SyntaxReference mapper
- `pad_cell` (line 315): display-width-aware cell padding/truncation
- Style helpers: `push_md_style` (342), `flush_md_line` (348), `merge_style` (361), `heading_color` (371)
- Constants: `CODE_BG`, `INLINE_CODE_BG`, `TOOL_COLOR` (lines 13-15)
- Statics: `SYNTAX_SET`, `SYNTAX_THEME` (lines 17-21) via `LazyLock`

### (3) Notable patterns
- **Style stack** (line 27): Stack of `Style` values for nested formatting
- **LazyLock statics** (lines 17-21): Syntax set and theme loaded once
- **Table column shrinking** (lines 244-274): Proportional scaling + iterative shrinking of widest columns
- **`Options::all()`** (line 24): Enables all markdown extensions

### (4) Code quality issues
- **`.unwrap()` on `style_stack.last()`** (line 162): Safe because stack initialized with one element (line 27), but fragile if refactored
- **`.unwrap()` on `stack.last()`** (line 343): Same
- **`_ => &language`** (line 486): Returns `&language` (`&&str`) which coerces to `&str` via deref coercion — works but subtle
- **`Options::all()`** (line 24): Enables all cmark options including `ENABLE_SMART_PUNCTUATION` which may not be desired

### (5) Performance concerns
- **`SYNTAX_SET::load_defaults_newlines()`** (line 17): Heavy initial load but cached via LazyLock
- **`render_syntect_block` allocates per line** (line 396): New `Vec<Span>` for each code line
- **`render_md_table`** (lines 229-237): O(rows × cols) for column width computation
- **`pad_cell` truncation** (lines 315-339): Iterates chars twice

### (6) Bugs / logic errors
- **Table cells don't support inline formatting** (lines 156-173): Text inside table cells is captured as plain string (line 160), losing bold/italic styling. Design limitation.
- **No wrapping for markdown text lines**: `markdown_to_lines` doesn't pre-wrap lines to `width`. Lines are rendered with `Wrap { trim: false }` in widgets, but height estimation uses `estimate_wrapped_rows` which may not match ratatui's actual wrapping. **Height estimation can diverge from actual rendered height.**

---

## 8. `tui/widgets/mod.rs` (6 lines)

### (1) Purpose
Module declarations for 6 widget submodules: blocks, diff, dropdown, input_bar, modal, status.

### (2-6) Analysis
No issues. Simple module declarations.

---

## 9. `tui/widgets/blocks.rs` (939 lines)

### (1) Purpose
Block rendering widgets and cache-building helpers. Contains height estimation, line generators for each block type, cache builders, and `Widget` implementations for all block types (System, User, Thought, Response, Tool, Subagent, Notice, Error, Working).

### (2) Key types/structs
- Height: `estimate_wrapped_rows` (25), `compute_block_height` (70)
- Line generators: `system_block_lines` (77), `user_block_lines` (84), `thought_block_lines` (94), `response_block_lines` (126), `tool_block_lines` (181), `subagent_block_lines` (233), `notice_block_lines` (306), `error_block_lines` (333), `working_block_lines` (345)
- Cache builders: `entry_to_blocks` (374), `turn_block_to_blocks` (410), `subagent_detail_blocks` (490)
- Widgets: `SystemBlock` (598), `UserBlock` (621), `ThoughtBlock` (657), `ResponseBlock` (680), `ToolBlock` (703), `SubagentBlock` (815), `NoticeBlock` (862), `ErrorBlock` (885), `WorkingBlock` (908)
- Statics: `GEAR_FRAMES` (813) = `["◐","◓","◑","◒"]`, `SPINNER_FRAMES` (939) = braille spinner
- Helpers: `format_duration` (362), `tool_args_summary` (145), `truncate_str` (159)
- Constants: `TOOL_TITLE_HEIGHT = 3` (142), color constants (14-21)

### (3) Notable patterns
- **Cached lines + dynamic title**: ToolBlock stores content lines in cache but renders title dynamically for gear animation (lines 703-810)
- **Scroll-aware rendering**: Each widget takes `skip` parameter for vertical scrolling
- **Subagent block color coding**: done/success/working/hovered (lines 841-860)
- **Two different spinner animations**: GEAR_FRAMES for tools, SPINNER_FRAMES for working indicator

### (4) Code quality issues
- **Massive code duplication**: `SystemBlock`, `ThoughtBlock`, `ResponseBlock`, `NoticeBlock`, `ErrorBlock` have **identical** render implementations (just `Paragraph::new(Text::from(self.lines.to_vec())).wrap(Wrap{trim:false}).scroll((self.skip,0)).render(area, buf)`). Should be a single generic widget.
- **`notice_block_lines` icon detection** (lines 313-325): Fragile case-sensitive string matching — `"registered"` → ✓ but `"Registered"` → ℹ

### (5) Performance concerns
- **BUG/PERF (line 62) — `estimate_wrapped_rows` is O(n²)****: `remaining.chars().count()` is called in every while-loop iteration. For a line of N characters wrapping K times, total cost is O(K×N). For long lines with narrow widths, this is significant.
- **`Text::from(self.lines.to_vec())`** (lines 614, 650, 673, 696, 807, 855, 878, 901): **Clones the entire line vector on every render for every visible block.** This is the hottest allocation path in the render loop.
- **`subagent_detail_blocks`** (line 490): Builds blocks from scratch every render call (no caching)

### (6) Bugs / logic errors
- **BUG (lines 150-154) — `tool_args_summary` strip fallback**: For input `"{content"` (no closing `}`):
  - `first = "{content"`
  - `stripped = first.strip_prefix("{").unwrap_or(first)` → `"content"`
  - `.strip_suffix("}").unwrap_or(first)` → `first` = `"{content"` (the **original**, not the intermediate `"content"`)
  - Result: `"{content"` — the `{` prefix is re-added. The `unwrap_or(first)` should be `unwrap_or(stripped_intermediate)`.
- **BUG (height estimation) — padding not accounted for**: In `turn_block_to_blocks`:
  - `Response` (line 431): lines generated with `inner_width` (width minus pad), but height computed with `width` (line 432). When pad is non-empty, actual wrapping produces more rows than estimated → **scroll misalignment**.
  - `Tool` (line 443): Same issue — lines use `inner_width`, height uses `width`.
  - `Thought` (line 421): Uses `width` for both, but `thought_block_lines` doesn't wrap. The widget uses `Wrap`, so actual height could exceed estimate for long thought lines.
  - `User` (line 388): `user_block_lines` doesn't wrap, but `UserBlock` widget uses `Wrap` → height underestimation for long user messages.
- **`WorkingBlock` ignores area height** (line 919): Renders single-line `Paragraph` without checking if `area.height > 1`.

---

## 10. `tui/widgets/diff.rs` (242 lines)

### (1) Purpose
Renders unified diffs with side-by-side line numbers, color-coded backgrounds (red deletions, green additions, dark context), context line folding, word-boundary wrapping, and addition/deletion stats footer.

### (2) Key types
- `render_diff_output` (line 11): main diff renderer
- `flush_context` (line 107): folds large context runs (keeps first/last 3, folds middle)
- `push_wrapped_diff_line` (line 140): wrapped diff line builder
- `wrap_diff_content` (line 165): word-first, char-fallback wrapper
- `diff_line` (line 219): single styled diff line with background fill
- `parse_hunk_header` (line 233): `@@ -12,5 +13,5 @@` parser

### (3) Notable patterns
- **Parallel line number tracking**: `old_line` and `new_line` counters (lines 23-24)
- **Context folding**: `MAX_CTX = 3`, folds middle into "... N unchanged lines ..." (lines 118-136)
- **Word-first wrapping with char fallback** (lines 177-201)

### (4) Code quality issues
- **`&raw[..1]` and `&raw[1..]`** (lines 60-61): Byte-index slicing. Safe for standard diff lines (start with ASCII `+`/`-`/` `) but would panic on malformed multi-byte input.

### (5) Performance concerns
- **`wrap_diff_content` allocates** new Strings for each word/chunk
- **`diff_line` fill** (line 221): `" ".repeat(fill)` allocates per line
- **`split_inclusive(' ')`** (line 177): Creates iterator with allocations

### (6) Bugs / logic errors
- **BUG (lines 56-58) — Empty line skipping**: `if raw.is_empty() { continue; }` skips empty lines. In valid unified diffs, context lines start with a space, so a blank context line is `" "` (one space), not empty. But if a tool strips trailing whitespace, blank context lines become empty and are skipped, causing **line number drift**.
- **`parse_hunk_header`** (line 233): `trim_start_matches("@@")` removes ALL leading `@` characters. For `@@ -12,5 +13,5 @@`, this works correctly. Edge case: `@@@ -12,5 +13,5 @@@` (merge conflict style) would also work but isn't standard unified diff.

---

## 11. `tui/widgets/dropdown.rs` (87 lines)

### (1) Purpose
Autocomplete dropdown widget showing filtered slash-command options with selection highlighting and viewport scrolling.

### (2) Key types
- `Dropdown<'a>` (line 12): holds `&AppState`

### (3) Notable patterns
- **Viewport scroll**: Center-based scrolling keeps selected item visible (lines 39-47)
- **Dynamic constraints**: Builds layout for visible rows (lines 51-59)

### (4) Code quality issues
- None significant

### (5) Performance concerns
- **`visible` computed twice** (lines 50, 63): Iterates options twice — once for count, once for rendering
- **Constraints rebuilt every render** (lines 51-59): Minor

### (6) Bugs / logic errors
- **BUG (line 76) — Character truncation, not display width**: `opt.chars().take(max_w).collect()` truncates by character count, not display width. CJK characters (width 2) or emoji would overflow the row. Should use `unicode_width`-aware truncation like `truncate_str` in blocks.rs.
- **`visible` variable** (line 50): Computed as `min(max_h, total - scroll_offset)` — correct.

---

## 12. `tui/widgets/input_bar.rs` (99 lines)

### (1) Purpose
Input bar widget rendering the prompt (`❯` or command-mode hint), user input text, and cursor position calculation.

### (2) Key types
- `InputBar<'a>` (line 13): holds `&AppState`
- `cursor_position` (line 24): computes `(x, y)` cursor coordinates using `UnicodeWidthStr`

### (3) Notable patterns
- **Display-width-aware cursor**: Uses `UnicodeWidthStr::width()` for cursor positioning (lines 31, 37)
- **Command-mode vs normal prompt**: Different styling (yellow hint vs green `❯`) (lines 48-73, 76-97)
- **Cursor clamping**: `cursor_x.min(max_x)` prevents cursor from going off-screen (line 34)

### (4) Code quality issues
- **Unused imports**: `Constraint` and `Layout` (compiler warning) — imported but not used
- **`state.input[..state.cursor_pos.min(state.input.len())]`** (lines 31, 37): Safe due to `min`, but if `cursor_pos` is not at a char boundary, this would panic. No invariant enforcement on `cursor_pos`.

### (5) Performance concerns
- Minimal — simple widget

### (6) Bugs / logic errors
- No bugs found. Cursor position calculations verified correct (border + prompt width accounted for).

---

## 13. `tui/widgets/modal.rs` (139 lines)

### (1) Purpose
Modal overlay for model picker and model registration form. Renders centered popup with `Clear` background.

### (2) Key types
- `Modal<'a>` (line 34): holds `&AppState`
- `centered_rect` (line 14): centering helper using Layout constraints

### (3) Notable patterns
- **Centered rect**: Dual Layout (vertical + horizontal) for centering (lines 14-32)
- **Clear widget**: Clears underlying area before rendering (lines 51, 95)
- **Model form**: 4 fields with active field highlighting via `▌` cursor indicator (lines 97-121)

### (4) Code quality issues
- **`models.len() + 4` cast** (line 49): `as u16` — could overflow for >65531 models (practically impossible)
- No real cursor positioning in form — uses `▌` character as indicator

### (5) Performance concerns
- **Modal rendered every frame** (render.rs line 72): Even when `ModalState::None`, widget is constructed and rendered (matches `None => {}`). Minor overhead.

### (6) Bugs / logic errors
- **Model form lacks cursor movement** (input.rs lines 317-357): Only supports appending chars and `pop()` (Backspace). No left/right arrow, no mid-field editing. **Limitation**: Can't edit in the middle of a field.
- **Model form Enter submits without validation** (input.rs lines 327-337): Empty fields are submitted. The `register_model` handler (mod.rs line 241) checks for 4 pipe-separated parts but doesn't validate non-emptiness.

---

## 14. `tui/widgets/status.rs` (99 lines)

### (1) Purpose
Status bar widget showing agent name, model, token count, agent state (with color coding), and scroll-paused indicator.

### (2) Key types
- `StatusBar<'a>` (line 12): holds `&AppState`

### (3) Notable patterns
- **State-based styling**: Different colors for streaming (amber+blink), thinking (purple), responding (cyan), running tools (amber), idle (green) (lines 25-52)
- **Scroll-paused indicator**: Shows when `scroll > 0` (lines 80-88)
- **`SLOW_BLINK` modifier** (line 30): Terminal blinking for streaming state

### (4) Code quality issues
- `SLOW_BLINK` may not be supported by all terminals

### (5) Performance concerns
- Minimal

### (6) Bugs / logic errors
- **BUG (line 83) — Misleading "press End to resume"**: Status bar says `"⬆ scroll paused — press End to resume"` when `scroll > 0`. But in input.rs, `End` only works with Ctrl modifier (line 180: `KeyCode::End | KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL)`), and it sets `cursor_pos = state.input.len()` — it does NOT reset `scroll` to 0. **There is no keybinding that resets conversation scroll to bottom.** PageDown decrements by 5, requiring multiple presses. The status bar instruction is wrong on two counts: (1) End doesn't work without Ctrl, (2) End doesn't reset scroll at all.

---

# Cross-Cutting Summary of Key Issues

## Confirmed Bugs (10)
1. **main.rs:437-440** — Bogus format string prints `"Memory:      Memory:      enabled"`
2. **mod.rs:190** — `std::thread::sleep` in async function blocks tokio executor
3. **render.rs:219** — Cache invalidation gap: entry blocks not rebuilt when content changes (only count)
4. **blocks.rs:150-154** — `tool_args_summary` re-adds `{` prefix when `}` suffix missing
5. **blocks.rs height estimation** — Padding not accounted for; actual wrapping diverges from estimate (Response/Tool/Thought/User blocks)
6. **input.rs:177,180** — Home/End keys require Ctrl modifier, don't work standalone
7. **input.rs:208** — Ctrl+Delete doesn't call `update_autocomplete()`
8. **state.rs:1118-1126** — `truncate_activity` uses byte length (`len()`) instead of char count for `max_chars` comparison
9. **dropdown.rs:76** — Truncation by char count not display width (CJK/emoji overflow)
10. **status.rs:83** — "press End to resume" is wrong; End doesn't reset scroll

## Security Concerns (1)
- **state.rs:681-690 + main.rs:1456-1460** — All tool approval requests silently auto-approved with `AllowSession`

## Performance Hotspots (5)
1. **blocks.rs:62** — `estimate_wrapped_rows` is O(n²) due to `remaining.chars().count()` in loop
2. **blocks.rs (all widgets)** — `Text::from(self.lines.to_vec())` clones all lines on every render
3. **render.rs:249** — `rebuild_cache` clones all entry blocks on every rebuild
4. **mod.rs tick** — 60fps Tick always returns `true`, forcing redraw every 16ms
5. **state.rs:380** — `find_subagent` linear scan through all entries + blocks, called frequently

## Dead Code (1 file + partial)
- **cli_completer.rs** — Entire file unused; duplicate of `CommandCompleter` in main.rs
- **main.rs** — `blue()` and `magenta()` color helpers marked `#[allow(dead_code)]`

## Code Smells
- `#![allow(deprecated)]` in main.rs (line 1) — hides deprecated API usage
- `#[allow(dead_code)]` on `BlockKind`, `Entry`, `SubagentState` — unused variants
- 5+ repeated closure patterns for subagent state updates in state.rs — should be refactored
- 5 identical block widget implementations in blocks.rs — should be generic
