# Plan: Learn From Session History (/learn)

Date: 2026-07-09

Enhance the `/learn` command to load session history and support empty arguments.

## Proposed Changes

### Frontend Parsing
* **`app/src/components/chat/ChatInput.tsx`**: Intercept `/learn` without arguments and forward empty string to backend.

### Backend Extraction
* **`app/src-tauri/src/lib.rs`**: 
  - Load session history.
  - If content is empty or generic, extract learnings directly from history.
  - If content is specific, use history as context for extraction.

---

## Tasks

Detailed tasks were tracked locally in `task.md`.
