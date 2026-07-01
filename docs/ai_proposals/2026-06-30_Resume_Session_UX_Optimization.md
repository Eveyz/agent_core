# 2026-06-30 Resume Session UX Optimization

## 1. Problem Analysis
When the application starts, it immediately attempts to restore the previously active session from the database. Currently, this process blocks or degrades the user experience in the following ways:
- **No Visual Feedback / Loading Indicator**: There is no loading state or visual feedback indicating that a session is being restored. The app mounts with an empty chat screen or default view, but the session is loaded asynchronously.
- **Unresponsive/Stuck UI**: While `resumeSession` is pending, clicking other sidebar items, creating sessions, or attempting to type results in race conditions or ignored inputs, which feels to the user like the buttons are "disabled".
- **Synchronous Locking in Backend**: The `get_config` Tauri command is defined as synchronous (`fn get_config`) and acquires a `blocking_lock()` on the asynchronous `run_manager`. While not directly related to session loading database speeds, any synchronous command that does blocking locks on async structures can lock up Tauri's main thread and freeze the GUI.

## 2. Proposed Solutions

### Backend Optimizations
- **Asynchronous Config Retrieval**: Convert the synchronous `get_config` command in `app/src-tauri/src/lib.rs` to an asynchronous command (`async fn get_config`) to ensure it runs on the tokio thread pool without any chance of blocking the GUI/main thread.

### Frontend UX Optimizations
- **Redux Loading State**: Introduce an `isResuming` boolean flag inside `ChatState`.
  - Set `isResuming` to `true` when `resumeSession.pending` is triggered.
  - Set `isResuming` to `false` when `resumeSession.fulfilled` or `resumeSession.rejected` is triggered.
- **Visual Feedback (Loading View)**: When `isResuming` is true, render a beautiful cosmic themed loading screen in the main chat/empty area instead of the blank `EmptyState` or a partially loaded chat area. This loading screen will use the app's standard planetary orbit/glow animation with a spinning Loader icon and a message "Resuming session...".
- **Disabling Inputs & Sidebar Interactions**:
  - Disable the `ChatInput` component and display the message "Resuming session..." inside the input box when `isResuming` is true.
  - Visually dim the sidebar and disable all pointer events (using a `.sidebar-resuming` CSS class) when `isResuming` is true, preventing conflicting interactions (such as opening settings, selecting other sessions, or creating new sessions) until the session is fully restored.
