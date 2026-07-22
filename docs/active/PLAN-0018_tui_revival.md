# PLAN-0018: TUI Full Revival (A + five-tier)

```yaml
---
id: PLAN-0018
type: PLAN
title: TUI Full Revival
status: Done
author: zniverse
created: 2026-07-23
updated: 2026-07-23
reviewers: []
related: [PLAN-0017]
supersedes: ~
superseded_by: ~
tags: [cli, tui, harness]
---
```

## Goal

Revive `ageverse --tui` on `CliState`/`RunManager`/`RunEvent`, with CLI slash parity, five-tier approval, and the UX/layout fixes from the gap analysis. Scope A (no GUI-only: images, `/goal`, `/plan`, `/btw`, workflow).

## Delivered

- Entry: `run_tui_mode` → `bootstrap_runtime` → `tui::run_tui`
- Shared [`cli/src/commands.rs`](../../cli/src/commands.rs) slash dispatch + approval key mapping
- [`cli/src/tui/events.rs`](../../cli/src/tui/events.rs) Run lifecycle glue
- [`cli/src/tui/state.rs`](../../cli/src/tui/state.rs) `RunEvent` reducer, five-tier Approval + Answer/Help/Pager/SessionList/Rewind/QuitConfirm
- Layout: thin status, dynamic input height, overlay autocomplete
- UX: Esc layers, multiline (Shift+Enter/Ctrl+J), follow scroll (G/End), thought/tool expand, yank (`arboard`), subagent focus

## Acceptance checklist

- [x] `cargo check -p agent-cli` passes
- [x] `ageverse --tui` entry no longer bails (wired to CliState)
- [x] Five-tier approval modal (1–5 / Esc=Deny) — no silent AllowSession
- [x] `InputRequested` → Answer modal
- [x] Slash commands routed via `commands` module (REPL + TUI share dispatch; not “not yet implemented”)
- [x] Session/MCP available via same bootstrap as REPL
- [x] Follow scroll + correct pause hint
- [x] Multiline input + paste path
- [x] Thought collapse / tool expand / pager / yank / help overlay
- [x] Unit tests: `approval_from_choice_key`, `ALL_COMMANDS`, help text
- [x] PLAN-0017 task 2.3 marked Done

## Manual smoke (recommended)

```bash
cargo run -p agent-cli --bin ageverse -- --tui
# /models, send a message, Esc abort, ? help, G follow, /quit confirm
```
