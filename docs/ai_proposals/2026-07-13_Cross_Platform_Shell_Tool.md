# 2026-07-13 — Cross-platform `shell` tool (replaces `bash`)

Date: 2026-07-13

## Summary

Renamed the agent shell tool from `bash` to `shell` and made the interpreter
platform-aware so Windows hosts no longer fail on `sh -c`.

| Platform | Interpreter |
|----------|-------------|
| Windows  | `powershell.exe -NoProfile -NonInteractive -Command …` |
| Unix     | `sh -c …` |

## Motivation

On Windows, `BashTool` / `ProcessSupervisor::spawn_bash` hard-coded `sh -c`.
Without Git Bash or WSL, every shell tool call failed at spawn. The app is
actively used on Windows, so this was a blocker.

## Design

1. **Tool name: `shell`** — clearer than pretending every host has bash.
2. **Shared helper** — `core/src/runtime/platform_shell.rs` owns
   `shell_program()`, `shell_command()`, and `is_shell_tool()`.
3. **`ShellTool`** — replaces `BashTool` in `core/src/tools/shell.rs`.
4. **`ProcessSupervisor::spawn_shell`** — replaces `spawn_bash`
   (`spawn_bash` kept as a deprecated alias).
5. **Compatibility** — `is_shell_tool` and several UI/permission paths still
   accept legacy `"bash"` so old transcripts and configs keep working.
6. **Permissions** — default readonly allowlist gains common PowerShell
   cmdlets (`Get-ChildItem`, `Get-Content`, `Select-String`, …).

## Out of scope (follow-ups)

- Windows Job Object process-tree kill (supervisor still stubs process groups
  on non-Unix).
- Skill `.sh` scripts on Windows (still need a POSIX shell or `.ps1` variants).
- Eval grader still invokes `bash checker.sh` for golden/contract suites.

## Key files

- `core/src/runtime/platform_shell.rs` (new)
- `core/src/tools/shell.rs` (new; `bash.rs` removed)
- `core/src/runtime/supervisor.rs`
- `core/src/permission/{mod,rules}.rs`
- `app/src/components/chat/{TurnIterationUI,turnHelpers,toolIcons,BashWidget}.tsx`
