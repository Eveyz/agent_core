//! Workflow, node, and edge definitions plus database CRUD.
//!
//! A [`WorkflowDef`] is a persisted DAG of nodes connected by edges. Nodes are
//! typed (`agent`, `input`, `output`, `transform`, `human_approval`) and carry
//! a free-form `config` JSON that the executor interprets (e.g. a node-level
//! router for conditional routing).

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::memory::storage::Storage;

/// The type of a workflow node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    /// Entry point — receives the workflow input.
    Input,
    /// Exit point — returns the workflow output.
    Output,
    /// Executes a custom agent (`agent_id` references `agents.id`).
    Agent,
    /// Transforms its input (JSON jq-style mapping in `config`).
    Transform,
    /// Pauses for human approval before continuing.
    HumanApproval,
}

impl NodeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Agent => "agent",
            Self::Transform => "transform",
            Self::HumanApproval => "human_approval",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "output" => Self::Output,
            "agent" => Self::Agent,
            "transform" => Self::Transform,
            "human_approval" => Self::HumanApproval,
            _ => Self::Input,
        }
    }
}

/// A node in a workflow graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDef {
    pub id: String,
    pub workflow_id: String,
    pub node_type: NodeType,
    pub label: String,
    /// For `agent` nodes: the agent definition id.
    #[serde(default)]
    pub agent_id: String,
    /// Free-form config JSON (router rules, transform spec, approval prompt…).
    #[serde(default = "default_json_object")]
    pub config: serde_json::Value,
    /// React Flow canvas position.
    #[serde(default)]
    pub position_x: f64,
    #[serde(default)]
    pub position_y: f64,
    pub created_at: String,
}

fn default_json_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// An edge connecting two nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeDef {
    pub id: String,
    pub workflow_id: String,
    pub source_node_id: String,
    pub target_node_id: String,
    #[serde(default)]
    pub source_handle: String,
    #[serde(default)]
    pub target_handle: String,
    #[serde(default)]
    pub label: String,
    /// Legacy per-edge condition (superseded by node-level routers, kept for
    /// backwards compatibility / canvas rendering).
    #[serde(default)]
    pub condition: String,
    #[serde(default = "default_data_mapping")]
    pub data_mapping: serde_json::Value,
    pub created_at: String,
}

fn default_data_mapping() -> serde_json::Value {
    serde_json::json!({"pass_through": true})
}

/// The trust posture of a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustMode {
    /// Inherit the global / per-agent permission config.
    Inherit,
    /// Trust mode: all tools auto-allowed (workflow-level Yolo).
    Trusted,
    /// Read-only mode: only ReadOnly-level tools allowed.
    Readonly,
}

impl Default for TrustMode {
    fn default() -> Self {
        Self::Inherit
    }
}

impl TrustMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Trusted => "trusted",
            Self::Readonly => "readonly",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "trusted" => Self::Trusted,
            "readonly" => Self::Readonly,
            _ => Self::Inherit,
        }
    }
}

/// What to do when a node fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnNodeFailure {
    /// Abort the entire workflow (default).
    Abort,
    /// Continue with downstream nodes (failed node output = null).
    Continue,
    /// Skip the failed node's downstream branch only.
    Skip,
}

impl Default for OnNodeFailure {
    fn default() -> Self {
        Self::Abort
    }
}

impl OnNodeFailure {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Abort => "abort",
            Self::Continue => "continue",
            Self::Skip => "skip",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "continue" => Self::Continue,
            "skip" => Self::Skip,
            _ => Self::Abort,
        }
    }
}

/// A complete workflow definition (nodes + edges + config).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDef {
    pub id: String,
    pub name: String,
    pub description: String,
    /// JSON schema describing the workflow input.
    #[serde(default = "default_json_object")]
    pub input_schema: serde_json::Value,
    pub trust_mode: TrustMode,
    pub max_concurrent: usize,
    pub on_node_failure: OnNodeFailure,
    /// Extra config (timeouts, etc.).
    #[serde(default = "default_json_object")]
    pub config: serde_json::Value,
    pub nodes: Vec<NodeDef>,
    pub edges: Vec<EdgeDef>,
    pub created_at: String,
    pub updated_at: String,
}

impl Default for WorkflowDef {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            input_schema: serde_json::Value::Object(serde_json::Map::new()),
            trust_mode: TrustMode::Inherit,
            max_concurrent: 3,
            on_node_failure: OnNodeFailure::Abort,
            config: serde_json::Value::Object(serde_json::Map::new()),
            nodes: Vec::new(),
            edges: Vec::new(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }
}

// ── DB CRUD ─────────────────────────────────────────────────────────

/// Create a new workflow (header + nodes + edges) in one transaction.
pub fn create(storage: &Storage, wf: &WorkflowDef) -> Result<WorkflowDef> {
    let id = if wf.id.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        wf.id.clone()
    };
    let now = Utc::now().to_rfc3339();
    let input_schema = serde_json::to_string(&wf.input_schema)?;
    let config = serde_json::to_string(&wf.config)?;

    {
        let mut db = storage.conn();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO workflows (id, name, description, input_schema, trust_mode, \
             max_concurrent, on_node_failure, config, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                wf.name,
                wf.description,
                input_schema,
                wf.trust_mode.as_str(),
                wf.max_concurrent as i64,
                wf.on_node_failure.as_str(),
                config,
                now,
                now,
            ],
        )?;
        insert_nodes(&tx, &id, &wf.nodes, &now)?;
        insert_edges(&tx, &id, &wf.edges, &now)?;
        tx.commit()?;
    }
    get(storage, &id)
}

/// Fetch a full workflow (header + nodes + edges).
pub fn get(storage: &Storage, id: &str) -> Result<WorkflowDef> {
    let db = storage.conn();
    let (name, description, input_schema, trust_mode, max_concurrent, on_node_failure, config, created_at, updated_at) =
        db.query_row(
            "SELECT name, description, input_schema, trust_mode, max_concurrent, \
             on_node_failure, config, created_at, updated_at FROM workflows WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)? as usize,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )
        .context("workflow not found")?;

    let nodes = load_nodes(&db, id)?;
    let edges = load_edges(&db, id)?;

    Ok(WorkflowDef {
        id: id.to_string(),
        name,
        description,
        input_schema: serde_json::from_str(&input_schema).unwrap_or_default(),
        trust_mode: TrustMode::from_str(&trust_mode),
        max_concurrent,
        on_node_failure: OnNodeFailure::from_str(&on_node_failure),
        config: serde_json::from_str(&config).unwrap_or_default(),
        nodes,
        edges,
        created_at,
        updated_at,
    })
}

/// List workflow headers (without nodes/edges) newest first.
pub fn list(storage: &Storage) -> Result<Vec<WorkflowDef>> {
    let db = storage.conn();
    let mut stmt = db.prepare(
        "SELECT id, name, description, input_schema, trust_mode, max_concurrent, \
         on_node_failure, config, created_at, updated_at FROM workflows \
         ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let input_schema: String = row.get(3)?;
        let trust_mode: String = row.get(4)?;
        let config: String = row.get(7)?;
        Ok(WorkflowDef {
            id,
            name: row.get(1)?,
            description: row.get(2)?,
            input_schema: serde_json::from_str(&input_schema).unwrap_or_default(),
            trust_mode: TrustMode::from_str(&trust_mode),
            max_concurrent: row.get::<_, i64>(5)? as usize,
            on_node_failure: OnNodeFailure::from_str(&row.get::<_, String>(6)?),
            config: serde_json::from_str(&config).unwrap_or_default(),
            nodes: Vec::new(),
            edges: Vec::new(),
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
        })
    })?;
    let mut workflows = Vec::new();
    for row in rows {
        workflows.push(row?);
    }
    Ok(workflows)
}

/// Save (upsert) a workflow's full graph. Replaces all nodes/edges.
pub fn save(storage: &Storage, wf: &WorkflowDef) -> Result<WorkflowDef> {
    let now = Utc::now().to_rfc3339();
    let input_schema = serde_json::to_string(&wf.input_schema)?;
    let config = serde_json::to_string(&wf.config)?;

    {
        let mut db = storage.conn();
        let tx = db.transaction()?;
        tx.execute(
            "UPDATE workflows SET name = ?2, description = ?3, input_schema = ?4, \
             trust_mode = ?5, max_concurrent = ?6, on_node_failure = ?7, config = ?8, \
             updated_at = ?9 WHERE id = ?1",
            params![
                wf.id,
                wf.name,
                wf.description,
                input_schema,
                wf.trust_mode.as_str(),
                wf.max_concurrent as i64,
                wf.on_node_failure.as_str(),
                config,
                now,
            ],
        )?;
        tx.execute("DELETE FROM workflow_nodes WHERE workflow_id = ?1", params![wf.id])?;
        tx.execute("DELETE FROM workflow_edges WHERE workflow_id = ?1", params![wf.id])?;
        insert_nodes(&tx, &wf.id, &wf.nodes, &now)?;
        insert_edges(&tx, &wf.id, &wf.edges, &now)?;
        tx.commit()?;
    }
    get(storage, &wf.id)
}

/// Delete a workflow and its nodes/edges/runs (cascades via FK).
pub fn delete(storage: &Storage, id: &str) -> Result<()> {
    let db = storage.conn();
    db.execute("DELETE FROM workflows WHERE id = ?1", params![id])
        .context("failed to delete workflow")?;
    Ok(())
}

fn insert_nodes(
    tx: &rusqlite::Transaction,
    workflow_id: &str,
    nodes: &[NodeDef],
    now: &str,
) -> Result<()> {
    for node in nodes {
        let config = serde_json::to_string(&node.config)?;
        tx.execute(
            "INSERT INTO workflow_nodes \
             (id, workflow_id, node_type, label, agent_id, config, position_x, position_y, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                node.id,
                workflow_id,
                node.node_type.as_str(),
                node.label,
                node.agent_id,
                config,
                node.position_x,
                node.position_y,
                now,
            ],
        )?;
    }
    Ok(())
}

fn insert_edges(
    tx: &rusqlite::Transaction,
    workflow_id: &str,
    edges: &[EdgeDef],
    now: &str,
) -> Result<()> {
    for edge in edges {
        let data_mapping = serde_json::to_string(&edge.data_mapping)?;
        tx.execute(
            "INSERT INTO workflow_edges \
             (id, workflow_id, source_node_id, target_node_id, source_handle, target_handle, \
             label, condition, data_mapping, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                edge.id,
                workflow_id,
                edge.source_node_id,
                edge.target_node_id,
                edge.source_handle,
                edge.target_handle,
                edge.label,
                edge.condition,
                data_mapping,
                now,
            ],
        )?;
    }
    Ok(())
}

fn load_nodes(db: &rusqlite::Connection, workflow_id: &str) -> Result<Vec<NodeDef>> {
    let mut stmt = db.prepare(
        "SELECT id, workflow_id, node_type, label, agent_id, config, position_x, position_y, created_at \
         FROM workflow_nodes WHERE workflow_id = ?1",
    )?;
    let rows = stmt.query_map(params![workflow_id], |row| {
        let node_type: String = row.get(2)?;
        let config: String = row.get(5)?;
        Ok(NodeDef {
            id: row.get(0)?,
            workflow_id: row.get(1)?,
            node_type: NodeType::from_str(&node_type),
            label: row.get(3)?,
            agent_id: row.get(4)?,
            config: serde_json::from_str(&config).unwrap_or_default(),
            position_x: row.get(6)?,
            position_y: row.get(7)?,
            created_at: row.get(8)?,
        })
    })?;
    let mut nodes = Vec::new();
    for row in rows {
        nodes.push(row?);
    }
    Ok(nodes)
}

fn load_edges(db: &rusqlite::Connection, workflow_id: &str) -> Result<Vec<EdgeDef>> {
    let mut stmt = db.prepare(
        "SELECT id, workflow_id, source_node_id, target_node_id, source_handle, target_handle, \
         label, condition, data_mapping, created_at \
         FROM workflow_edges WHERE workflow_id = ?1",
    )?;
    let rows = stmt.query_map(params![workflow_id], |row| {
        let data_mapping: String = row.get(8)?;
        Ok(EdgeDef {
            id: row.get(0)?,
            workflow_id: row.get(1)?,
            source_node_id: row.get(2)?,
            target_node_id: row.get(3)?,
            source_handle: row.get(4)?,
            target_handle: row.get(5)?,
            label: row.get(6)?,
            condition: row.get(7)?,
            data_mapping: serde_json::from_str(&data_mapping).unwrap_or_else(|_| {
                serde_json::json!({"pass_through": true})
            }),
            created_at: row.get(9)?,
        })
    })?;
    let mut edges = Vec::new();
    for row in rows {
        edges.push(row?);
    }
    Ok(edges)
}

// ── Run records ─────────────────────────────────────────────────────

/// A workflow execution record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub session_id: String,
    pub status: String,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub error: String,
    pub total_token_input: i64,
    pub total_token_output: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub created_at: String,
}

/// A per-node result within a workflow run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRunNodeResult {
    pub id: String,
    pub workflow_run_id: String,
    pub node_id: String,
    pub agent_history_id: String,
    pub status: String,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub error: String,
    pub token_input: i64,
    pub token_output: i64,
    pub cost_usd: f64,
    pub latency_ms: i64,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
}

/// Create a workflow_run record. Returns the run id.
pub fn create_run(
    storage: &Storage,
    workflow_id: &str,
    session_id: &str,
    input: &serde_json::Value,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let input_str = serde_json::to_string(input)?;
    let db = storage.conn();
    db.execute(
        "INSERT INTO workflow_runs (id, workflow_id, session_id, status, input, output, error, \
         total_token_input, total_token_output, started_at, finished_at, created_at) \
         VALUES (?1, ?2, ?3, 'running', ?4, '{}', '', 0, 0, ?5, NULL, ?6)",
        params![id, workflow_id, session_id, input_str, now, now],
    )?;
    Ok(id)
}

/// Finalize a workflow_run with status + output + token totals.
pub fn finish_run(
    storage: &Storage,
    run_id: &str,
    status: &str,
    output: &serde_json::Value,
    error: &str,
    token_input: i64,
    token_output: i64,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let output_str = serde_json::to_string(output)?;
    let db = storage.conn();
    db.execute(
        "UPDATE workflow_runs SET status = ?2, output = ?3, error = ?4, \
         total_token_input = ?5, total_token_output = ?6, finished_at = ?7 \
         WHERE id = ?1",
        params![run_id, status, output_str, error, token_input, token_output, now],
    )?;
    Ok(())
}

/// Record a per-node result.
pub fn record_node_result(
    storage: &Storage,
    result: &WorkflowRunNodeResult,
) -> Result<()> {
    let input_str = serde_json::to_string(&result.input)?;
    let output_str = serde_json::to_string(&result.output)?;
    let db = storage.conn();
    db.execute(
        "INSERT INTO workflow_run_node_results \
         (id, workflow_run_id, node_id, agent_history_id, status, input, output, error, \
         token_input, token_output, cost_usd, latency_ms, started_at, finished_at, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            result.id,
            result.workflow_run_id,
            result.node_id,
            result.agent_history_id,
            result.status,
            input_str,
            output_str,
            result.error,
            result.token_input,
            result.token_output,
            result.cost_usd,
            result.latency_ms,
            result.started_at,
            result.finished_at,
            result.created_at,
        ],
    )?;
    Ok(())
}

/// Mark a node result's status (e.g. "skipped").
pub fn set_node_status(storage: &Storage, run_id: &str, node_id: &str, status: &str) -> Result<()> {
    let db = storage.conn();
    db.execute(
        "UPDATE workflow_run_node_results SET status = ?3 WHERE workflow_run_id = ?1 AND node_id = ?2",
        params![run_id, node_id, status],
    )?;
    Ok(())
}

/// List recent runs for a workflow.
pub fn list_runs(storage: &Storage, workflow_id: &str, limit: usize) -> Result<Vec<WorkflowRun>> {
    let db = storage.conn();
    let mut stmt = db.prepare(
        "SELECT id, workflow_id, session_id, status, input, output, error, \
         total_token_input, total_token_output, started_at, finished_at, created_at \
         FROM workflow_runs WHERE workflow_id = ?1 ORDER BY started_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![workflow_id, limit as i64], |row| {
        let input: String = row.get(4)?;
        let output: String = row.get(5)?;
        Ok(WorkflowRun {
            id: row.get(0)?,
            workflow_id: row.get(1)?,
            session_id: row.get(2)?,
            status: row.get(3)?,
            input: serde_json::from_str(&input).unwrap_or_default(),
            output: serde_json::from_str(&output).unwrap_or_default(),
            error: row.get(6)?,
            total_token_input: row.get(7)?,
            total_token_output: row.get(8)?,
            started_at: row.get(9)?,
            finished_at: row.get(10)?,
            created_at: row.get(11)?,
        })
    })?;
    let mut runs = Vec::new();
    for row in rows {
        runs.push(row?);
    }
    Ok(runs)
}

/// Load all node results for a run.
pub fn get_run_node_results(storage: &Storage, run_id: &str) -> Result<Vec<WorkflowRunNodeResult>> {
    let db = storage.conn();
    let mut stmt = db.prepare(
        "SELECT id, workflow_run_id, node_id, agent_history_id, status, input, output, error, \
         token_input, token_output, cost_usd, latency_ms, started_at, finished_at, created_at \
         FROM workflow_run_node_results WHERE workflow_run_id = ?1",
    )?;
    let rows = stmt.query_map(params![run_id], |row| {
        let input: String = row.get(5)?;
        let output: String = row.get(6)?;
        Ok(WorkflowRunNodeResult {
            id: row.get(0)?,
            workflow_run_id: row.get(1)?,
            node_id: row.get(2)?,
            agent_history_id: row.get(3)?,
            status: row.get(4)?,
            input: serde_json::from_str(&input).unwrap_or_default(),
            output: serde_json::from_str(&output).unwrap_or_default(),
            error: row.get(7)?,
            token_input: row.get(8)?,
            token_output: row.get(9)?,
            cost_usd: row.get(10)?,
            latency_ms: row.get(11)?,
            started_at: row.get(12)?,
            finished_at: row.get(13)?,
            created_at: row.get(14)?,
        })
    })?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_save_release_db_lock_before_get() {
        let storage = Storage::new(":memory:").expect("in-memory storage");
        let wf = WorkflowDef {
            name: "Test Workflow".to_string(),
            ..Default::default()
        };

        let created = create(&storage, &wf).expect("create should not deadlock");
        assert_eq!(created.name, "Test Workflow");
        assert!(!created.id.is_empty());

        let saved = save(
            &storage,
            &WorkflowDef {
                id: created.id.clone(),
                name: "Renamed Workflow".to_string(),
                ..created
            },
        )
        .expect("save should not deadlock");
        assert_eq!(saved.name, "Renamed Workflow");
    }
}
