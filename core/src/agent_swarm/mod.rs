use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::{
    AgentMessage, AgentMessaging, DeliveryReceipt, MessageKind, MessagePart, SendAgentMessage,
    agent_registry,
    memory::storage::Storage,
    tools::{Tool, ToolRegistry},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwarmStatus {
    Running,
    Completed,
    Cancelled,
    NeedsAttention,
}

impl SwarmStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::NeedsAttention => "needs_attention",
        }
    }

    fn parse(value: &str) -> std::io::Result<Self> {
        match value {
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "needs_attention" => Ok(Self::NeedsAttention),
            value => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown swarm status '{value}'"),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmRun {
    pub id: String,
    pub project_id: String,
    pub root_agent_id: String,
    pub goal: String,
    pub status: SwarmStatus,
    pub max_messages: u32,
    pub messages_used: u32,
    pub max_turns: u32,
    pub turns_used: u32,
    pub summary: String,
    pub error: String,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSwarm {
    pub project_id: String,
    pub root_agent_id: String,
    pub goal: String,
    pub max_messages: u32,
    pub max_turns: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SwarmCommand {
    Send {
        from_agent_id: String,
        to_agent_id: String,
        parts: Vec<MessagePart>,
        priority: bool,
        idempotency_key: String,
    },
    Complete {
        agent_id: String,
        summary: String,
    },
    Cancel {
        reason: String,
    },
    Intervene {
        instruction: String,
        max_messages: Option<u32>,
        max_turns: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwarmSnapshot {
    pub run: SwarmRun,
    pub participant_agent_ids: Vec<String>,
    pub messages: Vec<AgentMessage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwarmEvent {
    pub sequence: i64,
    pub run_id: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwarmObservation {
    pub snapshot: SwarmSnapshot,
    pub events: Vec<SwarmEvent>,
    pub next_sequence: i64,
}

#[derive(Clone)]
pub struct SwarmCoordinator {
    storage: Storage,
    messaging: AgentMessaging,
    commands: Arc<Mutex<()>>,
}

#[derive(Clone)]
pub struct SwarmToolContext {
    pub run_id: String,
    pub agent_id: String,
    pub coordinator: SwarmCoordinator,
}

pub struct SendAgentMessageTool {
    context: SwarmToolContext,
}

impl SendAgentMessageTool {
    pub fn new(context: SwarmToolContext) -> Self {
        Self { context }
    }
}

#[async_trait]
impl Tool for SendAgentMessageTool {
    fn name(&self) -> &str {
        "send_agent_message"
    }
    fn description(&self) -> &str {
        "Send a durable message to another saved agent in the current swarm. Use the contact's name or id."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "to": { "type": "string", "description": "Recipient agent name or id" },
                "message": { "type": "string", "description": "Concrete request or update" },
                "priority": { "type": "boolean", "default": false }
            },
            "required": ["to", "message"]
        })
    }
    async fn execute(&self, args: serde_json::Value) -> Result<String> {
        let to = args
            .get("to")
            .and_then(|value| value.as_str())
            .context("to is required")?;
        let message = args
            .get("message")
            .and_then(|value| value.as_str())
            .context("message is required")?;
        if message.trim().is_empty() {
            bail!("message must not be empty");
        }
        let agents = agent_registry::list(&self.context.coordinator.storage)?;
        let recipient = agents
            .into_iter()
            .find(|agent| agent.id.eq_ignore_ascii_case(to) || agent.name.eq_ignore_ascii_case(to))
            .with_context(|| format!("agent contact '{to}' not found"))?;
        let priority = args
            .get("priority")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let snapshot = self.context.coordinator.command(
            &self.context.run_id,
            SwarmCommand::Send {
                from_agent_id: self.context.agent_id.clone(),
                to_agent_id: recipient.id,
                parts: vec![MessagePart::text(message)],
                priority,
                idempotency_key: format!("swarm-tool:{}", uuid::Uuid::new_v4()),
            },
        )?;
        let sent = snapshot
            .messages
            .last()
            .context("sent message missing from swarm snapshot")?;
        Ok(serde_json::json!({
            "status": "queued", "message_id": sent.id, "to": sent.to_display_name,
            "swarm_run_id": snapshot.run.id, "messages_remaining": snapshot.run.max_messages - snapshot.run.messages_used
        }).to_string())
    }
}

pub struct CompleteSwarmTool {
    context: SwarmToolContext,
}

impl CompleteSwarmTool {
    pub fn new(context: SwarmToolContext) -> Self {
        Self { context }
    }
}

#[async_trait]
impl Tool for CompleteSwarmTool {
    fn name(&self) -> &str {
        "complete_swarm"
    }
    fn description(&self) -> &str {
        "Complete the current swarm goal with a user-facing summary. Only the root agent may call this."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object", "properties": { "summary": { "type": "string" } },
            "required": ["summary"]
        })
    }
    async fn execute(&self, args: serde_json::Value) -> Result<String> {
        let summary = args
            .get("summary")
            .and_then(|value| value.as_str())
            .context("summary is required")?;
        let snapshot = self.context.coordinator.command(
            &self.context.run_id,
            SwarmCommand::Complete {
                agent_id: self.context.agent_id.clone(),
                summary: summary.to_string(),
            },
        )?;
        Ok(serde_json::to_string(&snapshot.run)?)
    }
}

pub fn register_swarm_tools(registry: &mut ToolRegistry, context: SwarmToolContext) {
    registry.register(Box::new(SendAgentMessageTool::new(context.clone())));
    registry.register(Box::new(CompleteSwarmTool::new(context)));
}

impl SwarmCoordinator {
    pub fn new(storage: Storage, messaging: AgentMessaging) -> Self {
        Self {
            storage,
            messaging,
            commands: Arc::new(Mutex::new(())),
        }
    }

    pub fn start(&self, command: StartSwarm) -> Result<SwarmRun> {
        if command.goal.trim().is_empty() {
            bail!("swarm goal must not be empty");
        }
        if command.max_messages == 0 || command.max_turns == 0 {
            bail!("swarm budgets must be greater than zero");
        }
        agent_registry::get(&self.storage, &command.root_agent_id)
            .with_context(|| format!("root agent '{}' does not exist", command.root_agent_id))?;
        let _guard = self.commands.lock();
        let now = Utc::now().to_rfc3339();
        let run = SwarmRun {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: command.project_id,
            root_agent_id: command.root_agent_id,
            goal: command.goal,
            status: SwarmStatus::Running,
            max_messages: command.max_messages,
            messages_used: 0,
            max_turns: command.max_turns,
            turns_used: 0,
            summary: String::new(),
            error: String::new(),
            created_at: now.clone(),
            updated_at: now.clone(),
            completed_at: None,
        };
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO agent_swarm_runs
             (id, project_id, root_agent_id, goal, status, max_messages, messages_used,
              max_turns, turns_used, summary, error, created_at, updated_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, 0, '', '', ?8, ?8, NULL)",
            params![
                run.id,
                run.project_id,
                run.root_agent_id,
                run.goal,
                run.status.as_str(),
                run.max_messages,
                run.max_turns,
                now
            ],
        )?;
        tx.execute(
            "INSERT INTO agent_swarm_participants (run_id, agent_id, joined_at) VALUES (?1, ?2, ?3)",
            params![run.id, run.root_agent_id, now],
        )?;
        append_event(
            &tx,
            &run.id,
            "swarm_started",
            &serde_json::json!({ "goal": run.goal, "root_agent_id": run.root_agent_id }),
            &now,
        )?;
        tx.commit()?;
        Ok(run)
    }

    pub fn command(&self, run_id: &str, command: SwarmCommand) -> Result<SwarmSnapshot> {
        let _guard = self.commands.lock();
        let run = self.run(run_id)?;
        if run.status != SwarmStatus::Running && !matches!(command, SwarmCommand::Intervene { .. })
        {
            bail!("swarm run '{run_id}' is not running");
        }
        match command {
            SwarmCommand::Send {
                from_agent_id,
                to_agent_id,
                parts,
                priority,
                idempotency_key,
            } => {
                self.send(
                    &run,
                    from_agent_id,
                    to_agent_id,
                    parts,
                    priority,
                    idempotency_key,
                )?;
            }
            SwarmCommand::Complete { agent_id, summary } => {
                self.complete(&run, &agent_id, &summary)?
            }
            SwarmCommand::Cancel { reason } => self.cancel(&run, &reason)?,
            SwarmCommand::Intervene {
                instruction,
                max_messages,
                max_turns,
            } => {
                self.intervene(&run, &instruction, max_messages, max_turns)?;
            }
        }
        self.snapshot(run_id)
    }

    pub fn observe(&self, run_id: &str, after_sequence: i64) -> Result<SwarmObservation> {
        let snapshot = self.snapshot(run_id)?;
        let db = self.storage.conn();
        let mut statement = db.prepare(
            "SELECT sequence, run_id, event_type, payload, created_at FROM agent_swarm_events
             WHERE run_id = ?1 AND sequence > ?2 ORDER BY sequence ASC",
        )?;
        let events = statement
            .query_map(params![run_id, after_sequence], event_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next_sequence = events
            .last()
            .map(|event| event.sequence)
            .unwrap_or(after_sequence);
        Ok(SwarmObservation {
            snapshot,
            events,
            next_sequence,
        })
    }

    pub fn snapshot(&self, run_id: &str) -> Result<SwarmSnapshot> {
        let run = self.run(run_id)?;
        let db = self.storage.conn();
        let mut statement = db.prepare(
            "SELECT agent_id FROM agent_swarm_participants WHERE run_id = ?1 ORDER BY joined_at ASC, rowid ASC",
        )?;
        let participant_agent_ids = statement
            .query_map(params![run_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(db);
        let messages = self.messaging.messages_for_context(run_id)?;
        Ok(SwarmSnapshot {
            run,
            participant_agent_ids,
            messages,
        })
    }

    pub fn latest_for_agent_project(
        &self,
        agent_id: &str,
        project_id: &str,
    ) -> Result<Option<SwarmSnapshot>> {
        let run_id = {
            let db = self.storage.conn();
            db.query_row(
                "SELECT run.id FROM agent_swarm_runs AS run
                 JOIN agent_swarm_participants AS participant ON participant.run_id = run.id
                 WHERE participant.agent_id = ?1 AND run.project_id = ?2
                 ORDER BY run.updated_at DESC, run.id DESC LIMIT 1",
                params![agent_id, project_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        };
        run_id.map(|id| self.snapshot(&id)).transpose()
    }

    /// Records the start of a peer-agent turn when `context_id` belongs to a swarm.
    /// Non-swarm message contexts are deliberately ignored for compatibility.
    pub fn begin_turn(&self, context_id: &str, agent_id: &str) -> Result<()> {
        let _guard = self.commands.lock();
        let Some(run) = self.try_run(context_id)? else {
            return Ok(());
        };
        if run.status != SwarmStatus::Running {
            bail!("swarm run '{context_id}' is not running");
        }
        if run.turns_used >= run.max_turns {
            self.needs_attention(&run.id, "turn budget exhausted")?;
            bail!("swarm turn budget exhausted");
        }
        let now = Utc::now().to_rfc3339();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        tx.execute("UPDATE agent_swarm_runs SET turns_used = turns_used + 1, updated_at = ?1 WHERE id = ?2", params![now, run.id])?;
        append_event(
            &tx,
            &run.id,
            "turn_started",
            &serde_json::json!({ "agent_id": agent_id }),
            &now,
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn mark_needs_attention(&self, context_id: &str, reason: &str) -> Result<()> {
        let _guard = self.commands.lock();
        if self.try_run(context_id)?.is_some() {
            self.needs_attention(context_id, reason)?;
        }
        Ok(())
    }

    /// Creates the protocol reply for a completed request and accounts for it
    /// in the same swarm budget. Non-swarm contexts use legacy messaging.
    pub fn reply(
        &self,
        original: &AgentMessage,
        source_conversation_id: &str,
        output: String,
    ) -> Result<DeliveryReceipt> {
        let _guard = self.commands.lock();
        let run = self.try_run(&original.context_id)?;
        if let Some(run) = &run {
            if run.status != SwarmStatus::Running {
                bail!("swarm run '{}' is not running", run.id);
            }
            if run.messages_used >= run.max_messages {
                self.needs_attention(&run.id, "message budget exhausted before reply")?;
                bail!("swarm message budget exhausted");
            }
        }
        let receipt = self.messaging.send(SendAgentMessage {
            source_conversation_id: source_conversation_id.to_string(),
            to_agent_id: original.from_agent_id.clone(),
            kind: MessageKind::Reply,
            parts: vec![MessagePart::text(output)],
            context_id: Some(original.context_id.clone()),
            correlation_id: Some(original.correlation_id.clone()),
            reply_to: Some(original.id.clone()),
            idempotency_key: format!("agent-reply:{}", original.id),
            hop_count: original.hop_count + 1,
            priority: false,
        })?;
        if let Some(run) = run
            && !receipt.replayed
        {
            self.attach_message(&run.id, &receipt.message)?;
        }
        Ok(receipt)
    }

    fn send(
        &self,
        run: &SwarmRun,
        from_agent_id: String,
        to_agent_id: String,
        parts: Vec<MessagePart>,
        priority: bool,
        idempotency_key: String,
    ) -> Result<DeliveryReceipt> {
        if run.messages_used >= run.max_messages {
            self.needs_attention(&run.id, "message budget exhausted")?;
            bail!("swarm message budget exhausted");
        }
        let db = self.storage.conn();
        let is_participant = db
            .query_row(
                "SELECT 1 FROM agent_swarm_participants WHERE run_id = ?1 AND agent_id = ?2",
                params![run.id, from_agent_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        drop(db);
        if !is_participant {
            bail!("sender is not a participant in this swarm");
        }
        let source = self
            .messaging
            .open_conversation(&from_agent_id, Some(&run.project_id))?;
        let receipt = self.messaging.send(SendAgentMessage {
            source_conversation_id: source.id,
            to_agent_id: to_agent_id.clone(),
            kind: MessageKind::Request,
            parts,
            context_id: Some(run.id.clone()),
            correlation_id: Some(run.id.clone()),
            reply_to: None,
            idempotency_key,
            hop_count: 1,
            priority,
        })?;
        let now = Utc::now().to_rfc3339();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        let joined = tx.execute(
            "INSERT OR IGNORE INTO agent_swarm_participants (run_id, agent_id, joined_at) VALUES (?1, ?2, ?3)",
            params![run.id, to_agent_id, now],
        )?;
        let attached = tx.execute(
            "INSERT OR IGNORE INTO agent_swarm_messages (run_id, message_id) VALUES (?1, ?2)",
            params![run.id, receipt.message.id],
        )?;
        if attached > 0 {
            tx.execute("UPDATE agent_swarm_runs SET messages_used = messages_used + 1, updated_at = ?1 WHERE id = ?2", params![now, run.id])?;
        }
        if joined > 0 {
            append_event(
                &tx,
                &run.id,
                "participant_joined",
                &serde_json::json!({ "agent_id": to_agent_id }),
                &now,
            )?;
        }
        if attached > 0 {
            append_event(
                &tx,
                &run.id,
                "message_sent",
                &serde_json::json!({ "message_id": receipt.message.id, "from_agent_id": from_agent_id, "to_agent_id": to_agent_id }),
                &now,
            )?;
        }
        tx.commit()?;
        Ok(receipt)
    }

    fn attach_message(&self, run_id: &str, message: &AgentMessage) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO agent_swarm_messages (run_id, message_id) VALUES (?1, ?2)",
            params![run_id, message.id],
        )?;
        if inserted > 0 {
            tx.execute("UPDATE agent_swarm_runs SET messages_used = messages_used + 1, updated_at = ?1 WHERE id = ?2", params![now, run_id])?;
            append_event(
                &tx,
                run_id,
                "message_sent",
                &serde_json::json!({ "message_id": message.id, "from_agent_id": message.from_agent_id, "to_agent_id": message.to_agent_id, "kind": message.kind }),
                &now,
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn complete(&self, run: &SwarmRun, agent_id: &str, summary: &str) -> Result<()> {
        if agent_id != run.root_agent_id {
            bail!("only the root agent can complete a swarm");
        }
        if summary.trim().is_empty() {
            bail!("completion summary must not be empty");
        }
        let active = self.messaging.active_tasks_for_context(&run.id)?;
        if active.len() > 1
            || active
                .first()
                .is_some_and(|task| task.recipient_agent_id != run.root_agent_id)
        {
            bail!("swarm cannot complete while other agent work is still active");
        }
        let now = Utc::now().to_rfc3339();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        tx.execute("UPDATE agent_swarm_runs SET status = 'completed', summary = ?1, updated_at = ?2, completed_at = ?2 WHERE id = ?3", params![summary, now, run.id])?;
        append_event(
            &tx,
            &run.id,
            "swarm_completed",
            &serde_json::json!({ "agent_id": agent_id, "summary": summary }),
            &now,
        )?;
        tx.commit()?;
        Ok(())
    }

    fn cancel(&self, run: &SwarmRun, reason: &str) -> Result<()> {
        self.messaging.cancel_context_tasks(&run.id, reason)?;
        let now = Utc::now().to_rfc3339();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        tx.execute("UPDATE agent_swarm_runs SET status = 'cancelled', error = ?1, updated_at = ?2, completed_at = ?2 WHERE id = ?3", params![reason, now, run.id])?;
        append_event(
            &tx,
            &run.id,
            "swarm_cancelled",
            &serde_json::json!({ "reason": reason }),
            &now,
        )?;
        tx.commit()?;
        Ok(())
    }

    fn intervene(
        &self,
        run: &SwarmRun,
        instruction: &str,
        max_messages: Option<u32>,
        max_turns: Option<u32>,
    ) -> Result<()> {
        let next_messages = max_messages
            .unwrap_or(run.max_messages)
            .max(run.messages_used);
        let next_turns = max_turns.unwrap_or(run.max_turns).max(run.turns_used);
        let now = Utc::now().to_rfc3339();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        tx.execute("UPDATE agent_swarm_runs SET status = 'running', max_messages = ?1, max_turns = ?2, error = '', updated_at = ?3, completed_at = NULL WHERE id = ?4", params![next_messages, next_turns, now, run.id])?;
        append_event(
            &tx,
            &run.id,
            "user_intervened",
            &serde_json::json!({ "instruction": instruction, "max_messages": next_messages, "max_turns": next_turns }),
            &now,
        )?;
        tx.commit()?;
        Ok(())
    }

    fn needs_attention(&self, run_id: &str, reason: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        tx.execute("UPDATE agent_swarm_runs SET status = 'needs_attention', error = ?1, updated_at = ?2 WHERE id = ?3", params![reason, now, run_id])?;
        append_event(
            &tx,
            run_id,
            "swarm_needs_attention",
            &serde_json::json!({ "reason": reason }),
            &now,
        )?;
        tx.commit()?;
        Ok(())
    }

    fn run(&self, run_id: &str) -> Result<SwarmRun> {
        let db = self.storage.conn();
        db.query_row(
            "SELECT id, project_id, root_agent_id, goal, status, max_messages, messages_used,
                    max_turns, turns_used, summary, error, created_at, updated_at, completed_at
             FROM agent_swarm_runs WHERE id = ?1",
            params![run_id],
            run_from_row,
        )
        .optional()?
        .with_context(|| format!("swarm run '{run_id}' not found"))
    }

    fn try_run(&self, run_id: &str) -> Result<Option<SwarmRun>> {
        let db = self.storage.conn();
        db.query_row(
            "SELECT id, project_id, root_agent_id, goal, status, max_messages, messages_used,
                    max_turns, turns_used, summary, error, created_at, updated_at, completed_at
             FROM agent_swarm_runs WHERE id = ?1",
            params![run_id],
            run_from_row,
        )
        .optional()
        .map_err(Into::into)
    }
}

fn append_event(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    event_type: &str,
    payload: &serde_json::Value,
    created_at: &str,
) -> Result<()> {
    tx.execute("INSERT INTO agent_swarm_events (run_id, event_type, payload, created_at) VALUES (?1, ?2, ?3, ?4)", params![run_id, event_type, serde_json::to_string(payload)?, created_at])?;
    Ok(())
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<SwarmRun> {
    Ok(SwarmRun {
        id: row.get(0)?,
        project_id: row.get(1)?,
        root_agent_id: row.get(2)?,
        goal: row.get(3)?,
        status: SwarmStatus::parse(&row.get::<_, String>(4)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
        })?,
        max_messages: row.get(5)?,
        messages_used: row.get(6)?,
        max_turns: row.get(7)?,
        turns_used: row.get(8)?,
        summary: row.get(9)?,
        error: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        completed_at: row.get(13)?,
    })
}

fn event_from_row(row: &Row<'_>) -> rusqlite::Result<SwarmEvent> {
    let payload: String = row.get(3)?;
    Ok(SwarmEvent {
        sequence: row.get(0)?,
        run_id: row.get(1)?,
        event_type: row.get(2)?,
        payload: serde_json::from_str(&payload).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
        })?,
        created_at: row.get(4)?,
    })
}
