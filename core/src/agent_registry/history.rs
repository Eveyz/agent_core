//! Agent execution history persistence.
//!
//! Every time a custom agent runs (standalone or inside a workflow), an entry
//! is recorded in the `agent_history` table. This gives per-agent observability
//! (token usage, latency, success/failure) and feeds the experimental Reflector
//! that proposes skill drafts.

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::memory::storage::Storage;

/// A single agent execution record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHistoryEntry {
    pub id: String,
    pub agent_id: String,
    pub session_id: String,
    pub workflow_run_id: String,
    /// "manual" / "workflow" / "cronjob"
    pub trigger: String,
    pub input: String,
    pub output: String,
    pub iterations_used: u32,
    pub success: bool,
    pub model_used: String,
    pub token_input: i64,
    pub token_output: i64,
    pub process_time_ms: i64,
    pub created_at: String,
}

impl Default for AgentHistoryEntry {
    fn default() -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: String::new(),
            session_id: String::new(),
            workflow_run_id: String::new(),
            trigger: "manual".to_string(),
            input: String::new(),
            output: String::new(),
            iterations_used: 0,
            success: true,
            model_used: String::new(),
            token_input: 0,
            token_output: 0,
            process_time_ms: 0,
            created_at: now,
        }
    }
}

fn from_row(row: &rusqlite::Row) -> rusqlite::Result<AgentHistoryEntry> {
    Ok(AgentHistoryEntry {
        id: row.get("id")?,
        agent_id: row.get("agent_id")?,
        session_id: row.get("session_id")?,
        workflow_run_id: row.get("workflow_run_id")?,
        trigger: row.get("trigger")?,
        input: row.get("input")?,
        output: row.get("output")?,
        iterations_used: row.get::<_, i64>("iterations_used")? as u32,
        success: row.get::<_, i64>("success")? != 0,
        model_used: row.get("model_used")?,
        token_input: row.get("token_input")?,
        token_output: row.get("token_output")?,
        process_time_ms: row.get("process_time_ms")?,
        created_at: row.get("created_at")?,
    })
}

const SELECT_COLS: &str =
    "id, agent_id, session_id, workflow_run_id, trigger, input, output, \
     iterations_used, success, model_used, token_input, token_output, \
     process_time_ms, created_at";

/// Record a new agent execution entry. Returns the inserted id.
pub fn record(storage: &Storage, entry: &AgentHistoryEntry) -> Result<String> {
    let now = Utc::now().to_rfc3339();
    let db = storage.conn();
    db.execute(
        "INSERT INTO agent_history \
         (id, agent_id, session_id, workflow_run_id, trigger, input, output, \
         iterations_used, success, model_used, token_input, token_output, \
         process_time_ms, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            entry.id,
            entry.agent_id,
            entry.session_id,
            entry.workflow_run_id,
            entry.trigger,
            entry.input,
            entry.output,
            entry.iterations_used as i64,
            entry.success as i64,
            entry.model_used,
            entry.token_input,
            entry.token_output,
            entry.process_time_ms,
            now,
        ],
    )
    .context("failed to record agent history")?;
    Ok(entry.id.clone())
}

/// List history entries for an agent, newest first.
pub fn list(storage: &Storage, agent_id: &str, limit: usize) -> Result<Vec<AgentHistoryEntry>> {
    let db = storage.conn();
    let mut stmt = db.prepare(&format!(
        "SELECT {SELECT_COLS} FROM agent_history WHERE agent_id = ?1 \
         ORDER BY created_at DESC LIMIT ?2"
    ))?;
    let entries = stmt
        .query_map(params![agent_id, limit as i64], from_row)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(entries)
}

/// Fetch recent history entries across all agents (for the Reflector).
pub fn get_recent(storage: &Storage, limit: usize) -> Result<Vec<AgentHistoryEntry>> {
    let db = storage.conn();
    let mut stmt = db.prepare(&format!(
        "SELECT {SELECT_COLS} FROM agent_history ORDER BY created_at DESC LIMIT ?1"
    ))?;
    let entries = stmt
        .query_map(params![limit as i64], from_row)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(entries)
}

/// Count executions for an agent.
pub fn count(storage: &Storage, agent_id: &str) -> Result<usize> {
    let db = storage.conn();
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM agent_history WHERE agent_id = ?1",
            params![agent_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(count as usize)
}
