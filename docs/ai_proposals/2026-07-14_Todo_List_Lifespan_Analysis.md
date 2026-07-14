# 2026-07-14 Todo List Lifespan & Interrupt Resumability Analysis

This document provides a comprehensive analysis of how the `agent_core` framework manages the lifespan of its todo lists, how interruptions affect execution state, how to resume execution, and how this compares to industry standards like Claude Code.

---

## 1. Executive Summary & Answer

### What is the lifespan of our todo list?
There are **two distinct todo lists** active in this environment:
1. **Internal `TodoList` (Agent Core State)**: Managed by the agent internally via tools (`todo_write`, `todo_update`, `todo_read`) and injected directly into prompts via Segment 7 execution state dashboard.
   - **Lifespan**: Purely **in-memory**. It exists for the lifetime of the running client process (the CLI REPL process or the Tauri desktop application process). It is never saved to the SQLite session database or to a dedicated disk file because `TodoList::new()` is instantiated without a storage path (even though a `with_storage` method exists in `TodoList`'s code, it is not utilized in the main execution paths).
   - **Cleanup**: It is lost when the CLI exits or the Tauri app closes. In Tauri, it is also cleared when a session goal is cleared (`clear_session_goal`).
2. **Artifact `task.md` (Workspace-scoped Markdown Document)**: Used in `planning_mode` where the agent is instructed to create and manage the file `<appDataDir>/brain/<conversation-id>/task.md` in the artifacts folder as a checklist.
   - **Lifespan**: **Persistent / Permanent**. Since it is written as a physical file on disk (inside the user's home configuration or app data directory), it survives process exits, shell restarts, and computer reboots.
   - **Cleanup**: Stored permanently in the local filesystem unless the conversation directory is deleted or reset.

### Can we resume remaining todos after cancellation or interruption?
*   **Yes, absolutely.** If you just type "try again" or "resume," the remaining todo list will be picked up.
*   **Why (Internal TodoList)**: Hitting Ctrl-C, `/abort`, or an LLM failure terminates the *active agent Run*, but the REPL/process itself remains alive. The `CliState` / `AppState` (which holds the in-memory `todo_list` / `SessionTodoStore`) survives. When you enter a new prompt, a new Run is created. During context generation, `sync_from_todos(&list)` is called. Since the items are still in the in-memory list, they are injected into the agent's prompt in Segment 7. The agent sees the plan, sees which items are already marked `completed`, and sees which one is `in_progress` or `pending`. It will pick up right where it left off.
*   **Why (Artifact task.md)**: Since `task.md` is a persistent markdown file on disk, its state is fully preserved after interrupts or app restarts. When a new run starts, the agent reads its context/rules and (if instructed to do so or if it scans the artifacts folder) will read the `task.md` file, see the unchecked tasks, and resume working on them.

---

## 2. Technical Diagnosis of Current Implementation

### Internal Todo List Architecture
*   **Definition**: Found in [core/src/todo/mod.rs](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/todo/mod.rs).
*   **Usage**: The CLI REPL `main.rs` initializes a single `todo_list = Arc::new(Mutex::new(TodoList::new()));` on startup. The Tauri app backend uses `brain().todo_lists` which is a `SessionTodoStore` mapping session IDs to in-memory `TodoList` instances.
*   **Storage Path is None**: Because `TodoList::new()` is called, the `storage_path` field is `None`. This means the `persist()` function is a no-op:
    ```rust
    fn persist(&self) {
        if let Some(ref path) = self.storage_path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(&self.items) {
                let _ = std::fs::write(path, json);
            }
        }
    }
    ```
*   **DB Persistence**: The SQLite session database (managed by [core/src/session.rs](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/session.rs) and [core/src/memory/storage.rs](file:///Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/memory/storage.rs)) does **not** contain any tables or logic for serialization of todo items or execution states.

---

## 3. How Other Systems Handle It

### Claude Code (Anthropic)
1.  **JSONL History Persistence**: Claude Code persists the entire conversation history to JSONL files on disk under local configuration directories (e.g. `~/.claude/sessions/`). Running `claude --continue` or `claude --resume` re-loads the entire conversation history, re-injecting the state.
2.  **Context Compaction**: Claude Code has a `/compact` command to summarize long histories but keeps critical state variables (like plans/remaining tasks) in active context to prevent context window bloat.
3.  **Local Workspace Plans**: For planning, Claude Code frequently creates a `plan.md` or `TODO.md` file in the workspace repository itself, ensuring that any developer or agent starting in that repo can inspect the tasks and resume them seamlessly, even across different sessions/branches.
4.  **Zombie Tasks**: If a plan is aborted mid-execution, Claude Code can sometimes have "stale" tasks in its context. Starting a new session or running `/clear` resets the state.

### Aider
- Does not have a formal structured step-by-step state machine like Agent Core. It relies on the user providing prompts and updates the git repository. When interrupted, it simply stops. Users must prompt Aider again with context. Aider keeps a chat history file so restarting the session restores the conversational thread.

---

## 4. Potential Improving Plans (No Code Changes Made Yet)

### Plan A: File-Backed Persistence for Core `TodoList` (Low Effort)
Rather than instantiating the CLI / Tauri todo list with `TodoList::new()`, utilize the existing `TodoList::with_storage(path)` method.
*   **Path**: Store the todo list under `~/.agverse/chats/<session_id>/todo.json`.
*   **Result**: When the CLI exits or the Tauri application is closed, the todo list is saved on disk. Starting a session again with the same `session_id` will auto-load the remaining todo list items and their statuses.

### Plan B: SQLite Schema Extension for Todo List (Medium Effort)
Store the structured todo items directly inside the main session database.
*   **Database Schema**:
    ```sql
    CREATE TABLE IF NOT EXISTS session_todos (
        session_id TEXT,
        item_id TEXT,
        description TEXT,
        status TEXT,
        depends_on TEXT, -- JSON array of dependent task IDs
        created_at TEXT,
        completed_at TEXT,
        PRIMARY KEY (session_id, item_id),
        FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
    );
    ```
*   **Result**: Todo lists are fully integrated into the session lifecycle. Whenever `/session save` is called or a prompt completes, the todos are serialized to SQLite. Resuming a session (`/session resume`) automatically loads the correct todo list.

### Plan C: Auto-Resume CLI Steering Commands (Low Effort)
Create a new slash command like `/resume` or `/continue`.
*   **Logic**: Instead of having to type "try again," `/resume` will automatically trigger a new Run with a system prompt context stating: *"Resume working on the remaining todo list. Focus on the active step: <step_description>"*.

### Plan D: Synchronize Core `TodoList` with Workspace `task.md` (Medium Effort)
Integrate the in-memory/DB todo list with the workspace's markdown `task.md` file:
*   Whenever the agent calls `todo_write` or `todo_update`, update the physical `task.md` file in the workspace or app-specific artifact folder.
*   If `task.md` is edited by the user manually, parse it to update the internal `TodoList` state. This aligns human-edited checklists with AI execution states.
