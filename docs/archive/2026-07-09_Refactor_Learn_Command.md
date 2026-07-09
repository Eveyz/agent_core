# Archive: Refactor Learn Command as Inline Turn

Date: 2026-07-09

## Overview
This document archives the plan and implementation details for refactoring the `/learn` command to execute inline as a standard chat turn using system instruction injection, argument-stripping, prioritised skill-creator checks, and cross-agent customization paths.

## Changes Made
- Intercepted `/learn` in `core/src/runtime/run/lifecycle.rs`. Cleaned the displayed user message context to hide rule arguments, leaving it as `"/learn"`.
- Extracted the rule content arguments and appended them to a structured system prompt.
- Guided the model to choose between appending to the `human` core memory block (for simple preferences) or writing a Custom Skill.
- Instructed the model to prioritize looking for and calling the `skill-creator` meta skill.
- Added fallbacks pointing to Workspace, Antigravity global, Claude Code global (`~/.claudecode`), and OpenCode/Codex global customization folders.
- Allowed the model to execute the normal loop, utilizing the `core_memory_append` or file-writing tools and streaming response.
- Removed `learn_memory` Tauri command and references from `app/src-tauri/src/lib.rs`.
- Streamlined `ChatInput.tsx` to let `/learn` fall through to standard message streams.
- Cleaned up callback in `App.tsx` and custom card list selectors in `ChatArea.tsx`.
- Removed `learnEntries` state fields, selectors, and reducers from Redux slice files (`chatSlice.ts`, `selectors.ts`, `types.ts`, `eventHandlers.ts`).
