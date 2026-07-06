# Active Plan: Cascading Session Deletion

Timestamp: 2026-07-06

This plan tracks the design and implementation of cascading deletion of session data to prevent database and filesystem leaks.

## Proposed Changes

### Database Cleanup
Extend the database cleanup in `SessionManager::delete` (in `core/src/session.rs`) to cover:
* Recursive child/subagent sessions (`parent_session_id = session_id`)
* Vector recall memories (`recall_memory`)
* Conversation summaries (`conversation_summaries`)
* Observability and run logs (`agent_history`, `workflow_runs`, `cronjob_runs`)

### Filesystem Cleanup
Remove associated filesystem directories and files:
* Mid-turn message snapshots: `~/.agverse/sessions/<session_id>.messages.json`
* Artifact directories (e.g., plans/walkthroughs/media): `~/.agverse/chats/<session_id>/`

---

## Tasks

Detailed tasks are tracked locally in `task.md`.
