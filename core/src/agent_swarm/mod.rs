use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    Completing,
    Completed,
    Cancelled,
    NeedsAttention,
}

impl SwarmStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completing => "completing",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::NeedsAttention => "needs_attention",
        }
    }

    fn parse(value: &str) -> std::io::Result<Self> {
        match value {
            "running" => Ok(Self::Running),
            "completing" => Ok(Self::Completing),
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
    pub max_hops: u8,
    pub hops_used: u8,
    pub summary: String,
    pub error: String,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub completion_task_id: Option<String>,
    pub completion_turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSwarm {
    pub project_id: String,
    pub root_agent_id: String,
    pub goal: String,
    pub max_messages: u32,
    pub max_turns: u32,
    pub max_hops: u8,
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
        hop_count: u8,
    },
    Complete {
        agent_id: String,
        summary: String,
        current_task_id: Option<String>,
        current_turn_id: Option<String>,
    },
    Cancel {
        reason: String,
    },
    Intervene {
        instruction: String,
        max_messages: Option<u32>,
        max_turns: Option<u32>,
        max_hops: Option<u8>,
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
    active_runs: crate::ActiveAgentRuns,
    commands: Arc<Mutex<()>>,
}

#[derive(Clone)]
pub struct SwarmToolContext {
    pub run_id: String,
    pub agent_id: String,
    pub next_hop: u8,
    pub effect_scope_id: String,
    pub active_task_id: Option<String>,
    pub active_turn_id: Option<String>,
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
        let mut effect = Sha256::new();
        effect.update(self.context.effect_scope_id.as_bytes());
        effect.update(self.context.agent_id.as_bytes());
        effect.update(recipient.id.as_bytes());
        effect.update(message.as_bytes());
        effect.update([priority as u8]);
        let idempotency_key = format!("swarm-tool:{}", hex::encode(effect.finalize()));
        let snapshot = self.context.coordinator.command(
            &self.context.run_id,
            SwarmCommand::Send {
                from_agent_id: self.context.agent_id.clone(),
                to_agent_id: recipient.id,
                parts: vec![MessagePart::text(message)],
                priority,
                idempotency_key: idempotency_key.clone(),
                hop_count: self.context.next_hop,
            },
        )?;
        let sent = snapshot
            .messages
            .iter()
            .find(|message| message.idempotency_key == idempotency_key)
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
                current_task_id: self.context.active_task_id.clone(),
                current_turn_id: self.context.active_turn_id.clone(),
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
    pub fn new(
        storage: Storage,
        messaging: AgentMessaging,
        active_runs: crate::ActiveAgentRuns,
    ) -> Self {
        Self {
            storage,
            messaging,
            active_runs,
            commands: Arc::new(Mutex::new(())),
        }
    }

    pub fn start(&self, command: StartSwarm) -> Result<SwarmRun> {
        if command.goal.trim().is_empty() {
            bail!("swarm goal must not be empty");
        }
        if command.max_messages == 0 || command.max_turns == 0 || command.max_hops == 0 {
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
            max_hops: command.max_hops,
            hops_used: 0,
            summary: String::new(),
            error: String::new(),
            created_at: now.clone(),
            updated_at: now.clone(),
            completed_at: None,
            completion_task_id: None,
            completion_turn_id: None,
        };
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO agent_swarm_runs
             (id, project_id, root_agent_id, goal, status, max_messages, messages_used,
              max_turns, turns_used, max_hops, hops_used, summary, error,
              created_at, updated_at, completed_at, completion_task_id, completion_turn_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, 0, ?8, 0, '', '', ?9, ?9, NULL, NULL, NULL)",
            params![
                run.id,
                run.project_id,
                run.root_agent_id,
                run.goal,
                run.status.as_str(),
                run.max_messages,
                run.max_turns,
                run.max_hops,
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
        let command_allowed = match (&run.status, &command) {
            (SwarmStatus::Running, _) => true,
            (
                SwarmStatus::Completing | SwarmStatus::NeedsAttention,
                SwarmCommand::Cancel { .. },
            ) => true,
            (SwarmStatus::NeedsAttention, SwarmCommand::Intervene { .. }) => true,
            // Let `intervene` own the lifecycle-specific validation so terminal
            // runs retain the stable "cannot be reopened" error contract.
            (_, SwarmCommand::Intervene { .. }) => true,
            _ => false,
        };
        if !command_allowed {
            bail!("swarm run '{run_id}' is not running");
        }
        match command {
            SwarmCommand::Send {
                from_agent_id,
                to_agent_id,
                parts,
                priority,
                idempotency_key,
                hop_count,
            } => {
                self.send(
                    &run,
                    from_agent_id,
                    to_agent_id,
                    parts,
                    priority,
                    idempotency_key,
                    hop_count,
                )?;
            }
            SwarmCommand::Complete {
                agent_id,
                summary,
                current_task_id,
                current_turn_id,
            } => self.complete(
                &run,
                &agent_id,
                &summary,
                current_task_id.as_deref(),
                current_turn_id.as_deref(),
            )?,
            SwarmCommand::Cancel { reason } => self.cancel(&run, &reason)?,
            SwarmCommand::Intervene {
                instruction,
                max_messages,
                max_turns,
                max_hops,
            } => {
                self.intervene(&run, &instruction, max_messages, max_turns, max_hops)?;
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
    pub fn begin_turn(
        &self,
        context_id: &str,
        agent_id: &str,
        turn_id: &str,
        lane: crate::AgentRunLane,
    ) -> Result<()> {
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
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO agent_swarm_active_turns
             (run_id, turn_id, agent_id, lane, started_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![run.id, turn_id, agent_id, run_lane_name(lane), now],
        )?;
        if inserted == 0 {
            tx.commit()?;
            return Ok(());
        }
        tx.execute("UPDATE agent_swarm_runs SET turns_used = turns_used + 1, updated_at = ?1 WHERE id = ?2", params![now, run.id])?;
        append_event(
            &tx,
            &run.id,
            "turn_started",
            &serde_json::json!({ "agent_id": agent_id, "turn_id": turn_id, "lane": run_lane_name(lane) }),
            &now,
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn finish_turn(&self, context_id: &str, turn_id: &str) -> Result<()> {
        let _guard = self.commands.lock();
        let now = Utc::now().to_rfc3339();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        let completion = tx
            .query_row(
                "SELECT status, completion_turn_id, summary FROM agent_swarm_runs WHERE id = ?1",
                params![context_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let removed = tx.execute(
            "DELETE FROM agent_swarm_active_turns WHERE run_id = ?1 AND turn_id = ?2",
            params![context_id, turn_id],
        )?;
        if removed == 1
            && completion.as_ref().is_some_and(|(status, completion_turn_id, _)| {
                status == "completing" && completion_turn_id.as_deref() == Some(turn_id)
            })
        {
            tx.execute(
                "UPDATE agent_swarm_runs SET status = 'completed', updated_at = ?1, completed_at = ?1 WHERE id = ?2",
                params![now, context_id],
            )?;
            append_event(
                &tx,
                context_id,
                "swarm_completed",
                &serde_json::json!({
                    "completion_turn_id": turn_id,
                    "summary": completion.as_ref().map(|(_, _, summary)| summary.as_str()).unwrap_or_default(),
                }),
                &now,
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn finalize_completion(&self, context_id: &str, task_id: &str) -> Result<()> {
        let _guard = self.commands.lock();
        let run = self.run(context_id)?;
        if run.status != SwarmStatus::Completing
            || run.completion_task_id.as_deref() != Some(task_id)
        {
            return Ok(());
        }
        let task = self.messaging.task(task_id)?;
        if task.status != crate::AgentTaskStatus::Completed {
            bail!("completion task is not complete");
        }
        let now = Utc::now().to_rfc3339();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        tx.execute(
            "UPDATE agent_swarm_runs SET status = 'completed', updated_at = ?1, completed_at = ?1 WHERE id = ?2",
            params![now, context_id],
        )?;
        append_event(
            &tx,
            context_id,
            "swarm_completed",
            &serde_json::json!({ "completion_task_id": task_id, "summary": run.summary }),
            &now,
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn mark_needs_attention(&self, context_id: &str, reason: &str) -> Result<()> {
        let _guard = self.commands.lock();
        if let Some(run) = self.try_run(context_id)? {
            match run.status {
                SwarmStatus::Running | SwarmStatus::Completing | SwarmStatus::NeedsAttention => {
                    self.needs_attention(context_id, reason)?;
                }
                SwarmStatus::Completed | SwarmStatus::Cancelled => {}
            }
        }
        Ok(())
    }

    /// Reconciles swarm lifecycle state after the messaging layer has marked
    /// interrupted deliveries as needing attention during application startup.
    pub fn recover_interrupted(&self, reason: &str) -> Result<Vec<SwarmRun>> {
        let _guard = self.commands.lock();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        let mut statement = tx.prepare(
            "SELECT DISTINCT run.id, run.status, completion_task.status
             FROM agent_swarm_runs AS run
             LEFT JOIN agent_messages AS message ON message.context_id = run.id
             LEFT JOIN agent_message_tasks AS task ON task.message_id = message.id
             LEFT JOIN agent_message_tasks AS completion_task ON completion_task.id = run.completion_task_id
             LEFT JOIN agent_swarm_active_turns AS turn ON turn.run_id = run.id
             WHERE (run.status = 'running'
                    AND (task.status IN ('needs_attention', 'failed') OR turn.turn_id IS NOT NULL))
                OR run.status = 'completing'",
        )?;
        let interrupted = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let now = Utc::now().to_rfc3339();
        for (run_id, status, completion_task_status) in &interrupted {
            let completion_finished =
                status == "completing" && completion_task_status.as_deref() == Some("completed");
            if completion_finished {
                tx.execute(
                    "UPDATE agent_swarm_runs SET status = 'completed', error = '', updated_at = ?1, completed_at = ?1 WHERE id = ?2",
                    params![now, run_id],
                )?;
                append_event(
                    &tx,
                    run_id,
                    "swarm_completed",
                    &serde_json::json!({ "recovered": true }),
                    &now,
                )?;
            } else {
                tx.execute(
                    "UPDATE agent_swarm_runs
                     SET status = 'needs_attention', error = ?1, updated_at = ?2
                     WHERE id = ?3",
                    params![reason, now, run_id],
                )?;
                append_event(
                    &tx,
                    run_id,
                    "swarm_needs_attention",
                    &serde_json::json!({ "reason": reason, "recovered": true, "completion_interrupted": status == "completing" }),
                    &now,
                )?;
            }
            tx.execute(
                "DELETE FROM agent_swarm_active_turns WHERE run_id = ?1",
                params![run_id],
            )?;
        }
        tx.commit()?;
        drop(db);
        interrupted
            .into_iter()
            .map(|(run_id, _, _)| self.run(&run_id))
            .collect()
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
        let idempotency_key = format!("agent-reply:{}", original.id);
        let replay = self
            .messaging
            .has_delivery_for_idempotency_key(&idempotency_key)?;
        if let Some(run) = &run {
            let final_completion_reply = if run.status == SwarmStatus::Completing {
                let task = self.messaging.delivery(&original.id)?.task;
                run.completion_task_id.as_deref() == Some(task.id.as_str())
            } else {
                false
            };
            if run.status != SwarmStatus::Running && !final_completion_reply {
                bail!("swarm run '{}' is not running", run.id);
            }
            if !replay && run.messages_used >= run.max_messages {
                self.needs_attention(&run.id, "message budget exhausted before reply")?;
                bail!("swarm message budget exhausted");
            }
            if !replay && original.hop_count.saturating_add(1) > run.max_hops {
                self.needs_attention(&run.id, "hop budget exhausted before reply")?;
                bail!("swarm hop budget exhausted");
            }
        }
        let next_hop = original.hop_count.saturating_add(1);
        let run_id = run.map(|run| run.id);
        self.messaging.send_with_transaction(
            SendAgentMessage {
                source_conversation_id: source_conversation_id.to_string(),
                to_agent_id: original.from_agent_id.clone(),
                kind: MessageKind::Reply,
                parts: vec![MessagePart::text(output)],
                context_id: Some(original.context_id.clone()),
                correlation_id: Some(original.correlation_id.clone()),
                reply_to: Some(original.id.clone()),
                idempotency_key,
                hop_count: next_hop,
                priority: false,
            },
            |tx, receipt| {
                if let Some(run_id) = &run_id {
                    attach_swarm_message(tx, run_id, receipt, next_hop)?;
                }
                Ok(())
            },
        )
    }

    fn send(
        &self,
        run: &SwarmRun,
        from_agent_id: String,
        to_agent_id: String,
        parts: Vec<MessagePart>,
        priority: bool,
        idempotency_key: String,
        hop_count: u8,
    ) -> Result<DeliveryReceipt> {
        let replay = self
            .messaging
            .has_delivery_for_idempotency_key(&idempotency_key)?;
        if !replay && run.messages_used >= run.max_messages {
            self.needs_attention(&run.id, "message budget exhausted")?;
            bail!("swarm message budget exhausted");
        }
        if !replay && (hop_count == 0 || hop_count > run.max_hops) {
            self.needs_attention(&run.id, "hop budget exhausted")?;
            bail!("swarm hop budget exhausted");
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
        let run_id = run.id.clone();
        let event_to_agent_id = to_agent_id.clone();
        self.messaging.send_with_transaction(
            SendAgentMessage {
                source_conversation_id: source.id,
                to_agent_id,
                kind: MessageKind::Request,
                parts,
                context_id: Some(run.id.clone()),
                correlation_id: Some(run.id.clone()),
                reply_to: None,
                idempotency_key,
                hop_count,
                priority,
            },
            |tx, receipt| {
                let now = Utc::now().to_rfc3339();
                let joined = tx.execute(
                    "INSERT OR IGNORE INTO agent_swarm_participants (run_id, agent_id, joined_at) VALUES (?1, ?2, ?3)",
                    params![run_id, event_to_agent_id, now],
                )?;
                if joined > 0 {
                    append_event(
                        tx,
                        &run_id,
                        "participant_joined",
                        &serde_json::json!({ "agent_id": event_to_agent_id }),
                        &now,
                    )?;
                }
                attach_swarm_message(tx, &run_id, receipt, hop_count)
            },
        )
    }

    fn complete(
        &self,
        run: &SwarmRun,
        agent_id: &str,
        summary: &str,
        current_task_id: Option<&str>,
        current_turn_id: Option<&str>,
    ) -> Result<()> {
        if agent_id != run.root_agent_id {
            bail!("only the root agent can complete a swarm");
        }
        if summary.trim().is_empty() {
            bail!("completion summary must not be empty");
        }
        let active = self.messaging.active_tasks_for_context(&run.id)?;
        let only_current_root_task = active.len() == 1
            && current_task_id.is_some_and(|task_id| active[0].id == task_id)
            && active[0].recipient_agent_id == run.root_agent_id
            && active[0].status == crate::AgentTaskStatus::Working;
        if (current_task_id.is_some() && !only_current_root_task)
            || (current_task_id.is_none() && !active.is_empty())
        {
            bail!("swarm cannot complete while other agent work is still active");
        }
        if only_current_root_task {
            let message = self.messaging.message(&active[0].message_id)?;
            if message.kind == MessageKind::Request {
                if run.messages_used >= run.max_messages {
                    bail!("swarm cannot complete without message budget for the final reply");
                }
                if message.hop_count.saturating_add(1) > run.max_hops {
                    bail!("swarm cannot complete without hop budget for the final reply");
                }
            }
        }
        let active_turns = {
            let db = self.storage.conn();
            let mut statement = db.prepare(
                "SELECT turn_id, agent_id FROM agent_swarm_active_turns WHERE run_id = ?1",
            )?;
            statement
                .query_map(params![run.id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let only_current_root_turn = active_turns.len() == 1
            && current_turn_id.is_some_and(|turn_id| active_turns[0].0 == turn_id)
            && active_turns[0].1 == run.root_agent_id;
        if (current_turn_id.is_some() && !only_current_root_turn)
            || (current_turn_id.is_none() && !active_turns.is_empty())
        {
            bail!("swarm cannot complete while other agent turns are still active");
        }
        let now = Utc::now().to_rfc3339();
        let next_status = if current_task_id.is_some() || current_turn_id.is_some() {
            SwarmStatus::Completing
        } else {
            SwarmStatus::Completed
        };
        let completion_turn_id = current_task_id.is_none().then_some(current_turn_id).flatten();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        tx.execute("UPDATE agent_swarm_runs SET status = ?1, summary = ?2, updated_at = ?3, completed_at = CASE WHEN ?1 = 'completed' THEN ?3 ELSE NULL END, completion_task_id = ?4, completion_turn_id = ?5 WHERE id = ?6", params![next_status.as_str(), summary, now, current_task_id, completion_turn_id, run.id])?;
        append_event(
            &tx,
            &run.id,
            if next_status == SwarmStatus::Completed {
                "swarm_completed"
            } else {
                "swarm_completion_requested"
            },
            &serde_json::json!({ "agent_id": agent_id, "summary": summary, "current_task_id": current_task_id, "current_turn_id": current_turn_id }),
            &now,
        )?;
        tx.commit()?;
        Ok(())
    }

    fn cancel(&self, run: &SwarmRun, reason: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        let cancelled = self
            .messaging
            .cancel_context_tasks_in(&tx, &run.id, reason)?;
        let mut active_statement =
            tx.prepare("SELECT turn_id, agent_id FROM agent_swarm_active_turns WHERE run_id = ?1")?;
        let active_turns = active_statement
            .query_map(params![run.id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(active_statement);
        tx.execute(
            "DELETE FROM agent_swarm_active_turns WHERE run_id = ?1",
            params![run.id],
        )?;
        tx.execute("UPDATE agent_swarm_runs SET status = 'cancelled', error = ?1, updated_at = ?2, completed_at = ?2 WHERE id = ?3", params![reason, now, run.id])?;
        append_event(
            &tx,
            &run.id,
            "swarm_cancelled",
            &serde_json::json!({ "reason": reason }),
            &now,
        )?;
        tx.commit()?;
        drop(db);
        for task in cancelled
            .iter()
            .filter(|task| task.status == crate::AgentTaskStatus::Working)
        {
            self.active_runs
                .cancel_peer_task(&task.recipient_agent_id, &task.id);
        }
        for (turn_id, agent_id) in active_turns {
            self.active_runs.cancel_run(&agent_id, &turn_id);
        }
        Ok(())
    }

    fn intervene(
        &self,
        run: &SwarmRun,
        instruction: &str,
        max_messages: Option<u32>,
        max_turns: Option<u32>,
        max_hops: Option<u8>,
    ) -> Result<()> {
        if !matches!(
            run.status,
            SwarmStatus::Running | SwarmStatus::NeedsAttention
        ) {
            bail!("terminal swarm runs cannot be reopened by intervention");
        }
        if instruction.trim().is_empty() {
            bail!("intervention instruction must not be empty");
        }
        let next_messages = max_messages
            .unwrap_or(run.max_messages)
            .max(run.messages_used);
        let next_turns = max_turns.unwrap_or(run.max_turns).max(run.turns_used);
        let next_hops = max_hops.unwrap_or(run.max_hops).max(run.hops_used);
        let now = Utc::now().to_rfc3339();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        tx.execute("UPDATE agent_swarm_runs SET status = 'running', max_messages = ?1, max_turns = ?2, max_hops = ?3, error = '', updated_at = ?4, completed_at = NULL, completion_task_id = NULL, completion_turn_id = NULL WHERE id = ?5", params![next_messages, next_turns, next_hops, now, run.id])?;
        append_event(
            &tx,
            &run.id,
            "user_intervened",
            &serde_json::json!({ "instruction": instruction, "max_messages": next_messages, "max_turns": next_turns, "max_hops": next_hops }),
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
                    max_turns, turns_used, max_hops, hops_used, summary, error,
                    created_at, updated_at, completed_at, completion_task_id, completion_turn_id
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
                    max_turns, turns_used, max_hops, hops_used, summary, error,
                    created_at, updated_at, completed_at, completion_task_id, completion_turn_id
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

fn run_lane_name(lane: crate::AgentRunLane) -> &'static str {
    match lane {
        crate::AgentRunLane::User => "user",
        crate::AgentRunLane::Peer => "peer",
    }
}

fn attach_swarm_message(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    receipt: &DeliveryReceipt,
    hop_count: u8,
) -> Result<()> {
    if receipt.message.context_id != run_id {
        bail!("replayed message belongs to a different swarm run");
    }
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO agent_swarm_messages (run_id, message_id) VALUES (?1, ?2)",
        params![run_id, receipt.message.id],
    )?;
    if inserted == 0 {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "UPDATE agent_swarm_runs
         SET messages_used = messages_used + 1,
             hops_used = MAX(hops_used, ?1), updated_at = ?2
         WHERE id = ?3",
        params![hop_count, now, run_id],
    )?;
    append_event(
        tx,
        run_id,
        "message_sent",
        &serde_json::json!({
            "message_id": receipt.message.id,
            "from_agent_id": receipt.message.from_agent_id,
            "to_agent_id": receipt.message.to_agent_id,
            "kind": receipt.message.kind,
        }),
        &now,
    )
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
        max_hops: row.get(9)?,
        hops_used: row.get(10)?,
        summary: row.get(11)?,
        error: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        completed_at: row.get(15)?,
        completion_task_id: row.get(16)?,
        completion_turn_id: row.get(17)?,
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
