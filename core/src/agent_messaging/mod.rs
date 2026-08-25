//! Durable local messaging between saved agents.
//!
//! This module intentionally models the A2A data lifecycle without exposing a
//! network transport. Callers send immutable messages, observe per-conversation
//! events, and advance the recipient task through explicit commands. A future
//! remote A2A adapter can satisfy the same interface without changing the UI.

mod active_runs;
pub mod dispatcher;

pub use active_runs::{ActiveAgentRunLease, ActiveAgentRuns, AgentRunLane, PeerMessageRoute};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::{agent_registry, memory::storage::Storage};

pub const AGENT_MESSAGE_SCHEMA_V1: &str = "agverse.agent-message@1";
pub const MAX_AGENT_MESSAGE_HOPS: u8 = 2;
const MAX_MESSAGE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConversation {
    pub id: String,
    pub agent_id: String,
    pub project_id: String,
    pub session_id: String,
    pub unread_count: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePart {
    Text { text: String },
    Data { value: serde_json::Value },
    File { artifact_id: String },
}

impl MessagePart {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Request,
    Reply,
    Notification,
}

impl MessageKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Reply => "reply",
            Self::Notification => "notification",
        }
    }

    fn from_str(value: &str) -> std::io::Result<Self> {
        match value {
            "request" => Ok(Self::Request),
            "reply" => Ok(Self::Reply),
            "notification" => Ok(Self::Notification),
            other => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown agent message kind '{other}'"),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub id: String,
    pub schema_version: String,
    pub context_id: String,
    pub from_agent_id: String,
    pub from_revision_id: String,
    pub from_display_name: String,
    pub to_agent_id: String,
    pub to_revision_id: String,
    pub to_display_name: String,
    pub kind: MessageKind,
    pub parts: Vec<MessagePart>,
    pub correlation_id: String,
    pub reply_to: Option<String>,
    pub source_conversation_id: String,
    pub target_conversation_id: String,
    pub project_id: String,
    pub idempotency_key: String,
    pub hop_count: u8,
    pub priority: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTaskStatus {
    Queued,
    Working,
    InputRequired,
    NeedsAttention,
    Completed,
    Failed,
    Cancelled,
}

impl AgentTaskStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Working => "working",
            Self::InputRequired => "input_required",
            Self::NeedsAttention => "needs_attention",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn from_str(value: &str) -> std::io::Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "working" => Ok(Self::Working),
            "input_required" => Ok(Self::InputRequired),
            "needs_attention" => Ok(Self::NeedsAttention),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown agent task status '{other}'"),
            )),
        }
    }

    fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessageTask {
    pub id: String,
    pub message_id: String,
    pub recipient_agent_id: String,
    pub recipient_conversation_id: String,
    pub status: AgentTaskStatus,
    pub output_message_id: Option<String>,
    pub error: String,
    pub attempt_count: u32,
    pub worker_id: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMessageEvent {
    pub sequence: i64,
    pub conversation_id: String,
    pub event_type: String,
    pub message_id: Option<String>,
    pub task_id: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageObservation {
    pub conversation: AgentConversation,
    pub events: Vec<AgentMessageEvent>,
    pub next_sequence: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendAgentMessage {
    pub source_conversation_id: String,
    pub to_agent_id: String,
    pub kind: MessageKind,
    pub parts: Vec<MessagePart>,
    #[serde(default)]
    pub context_id: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub reply_to: Option<String>,
    pub idempotency_key: String,
    pub hop_count: u8,
    #[serde(default)]
    pub priority: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub message: AgentMessage,
    pub task: AgentMessageTask,
    pub target_conversation: AgentConversation,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimedAgentMessage {
    pub message: AgentMessage,
    pub task: AgentMessageTask,
    pub target_conversation: AgentConversation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentTaskCommand {
    RequireInput { reason: String },
    NeedsAttention { reason: String },
    Complete { output_message_id: Option<String> },
    Fail { error: String },
    Cancel { reason: String },
}

#[derive(Clone)]
pub struct AgentMessaging {
    storage: Storage,
}

impl AgentMessaging {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }

    pub fn open_conversation(
        &self,
        agent_id: &str,
        project_id: Option<&str>,
    ) -> Result<AgentConversation> {
        let agent = agent_registry::get(&self.storage, agent_id)
            .with_context(|| format!("open conversation for agent '{agent_id}'"))?;
        let project_id = project_id.unwrap_or("");
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        let conversation = open_conversation_tx(&tx, &agent, project_id)?;
        tx.commit()?;
        Ok(conversation)
    }

    pub fn conversation(&self, conversation_id: &str) -> Result<AgentConversation> {
        let db = self.storage.conn();
        conversation_by_id(&db, conversation_id)?
            .with_context(|| format!("agent conversation '{conversation_id}' not found"))
    }

    pub fn list_conversations(&self, project_id: &str) -> Result<Vec<AgentConversation>> {
        let db = self.storage.conn();
        let mut statement = db.prepare(
            "SELECT id, agent_id, project_id, session_id, unread_count, created_at, updated_at
             FROM agent_conversations
             WHERE project_id = ?1
             ORDER BY updated_at DESC",
        )?;
        let conversations = statement
            .query_map(params![project_id], conversation_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(conversations)
    }

    pub fn send(&self, command: SendAgentMessage) -> Result<DeliveryReceipt> {
        validate_send_command(&command)?;
        let mut db = self.storage.conn();
        let tx = db.transaction()?;

        if let Some((message, task)) = delivery_by_idempotency_key(&tx, &command.idempotency_key)? {
            let target_conversation = conversation_by_id(&tx, &message.target_conversation_id)?
                .context("target conversation for replayed message is missing")?;
            tx.commit()?;
            return Ok(DeliveryReceipt {
                message,
                task,
                target_conversation,
                replayed: true,
            });
        }

        let source =
            conversation_by_id(&tx, &command.source_conversation_id)?.with_context(|| {
                format!(
                    "source conversation '{}' not found",
                    command.source_conversation_id
                )
            })?;
        if source.agent_id == command.to_agent_id {
            bail!("an agent cannot message itself");
        }
        let from = agent_registry::get_with_conn(&tx, &source.agent_id)
            .with_context(|| format!("sender agent '{}' no longer exists", source.agent_id))?;
        let to = agent_registry::get_with_conn(&tx, &command.to_agent_id)
            .with_context(|| format!("recipient agent '{}' does not exist", command.to_agent_id))?;
        let target = open_conversation_tx(&tx, &to, &source.project_id)?;

        let (context_id, correlation_id, reply_to) = match command.kind {
            MessageKind::Reply => {
                let reply_to = command
                    .reply_to
                    .as_deref()
                    .context("reply_to is required for replies")?;
                let original = message_by_id(&tx, reply_to)?
                    .with_context(|| format!("reply target message '{reply_to}' not found"))?;
                if original.from_agent_id != to.id || original.to_agent_id != from.id {
                    bail!("reply sender and recipient must reverse the original message route");
                }
                if let Some(correlation_id) = &command.correlation_id
                    && correlation_id != &original.correlation_id
                {
                    bail!("reply correlation_id must match the original message");
                }
                (
                    original.context_id,
                    original.correlation_id,
                    Some(original.id),
                )
            }
            MessageKind::Request | MessageKind::Notification => {
                if command.reply_to.is_some() {
                    bail!("reply_to is only valid for reply messages");
                }
                let correlation = command
                    .correlation_id
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                (
                    command
                        .context_id
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    correlation,
                    None,
                )
            }
        };

        let now = Utc::now().to_rfc3339();
        let message = AgentMessage {
            id: uuid::Uuid::new_v4().to_string(),
            schema_version: AGENT_MESSAGE_SCHEMA_V1.to_string(),
            context_id,
            from_agent_id: from.id,
            from_revision_id: from.updated_at,
            from_display_name: from.name,
            to_agent_id: to.id,
            to_revision_id: to.updated_at,
            to_display_name: to.name,
            kind: command.kind,
            parts: command.parts,
            correlation_id,
            reply_to,
            source_conversation_id: source.id.clone(),
            target_conversation_id: target.id.clone(),
            project_id: source.project_id.clone(),
            idempotency_key: command.idempotency_key,
            hop_count: command.hop_count,
            priority: command.priority,
            created_at: now.clone(),
        };
        insert_message(&tx, &message)?;

        let task = AgentMessageTask {
            id: uuid::Uuid::new_v4().to_string(),
            message_id: message.id.clone(),
            recipient_agent_id: message.to_agent_id.clone(),
            recipient_conversation_id: target.id.clone(),
            status: AgentTaskStatus::Queued,
            output_message_id: None,
            error: String::new(),
            attempt_count: 0,
            worker_id: String::new(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        insert_task(&tx, &task)?;

        append_event(
            &tx,
            &source.id,
            "message_sent",
            Some(&message.id),
            Some(&task.id),
            &serde_json::json!({
                "to": message.to_display_name,
                "kind": message.kind,
                "priority": message.priority,
            }),
            &now,
        )?;
        append_event(
            &tx,
            &target.id,
            "message_received",
            Some(&message.id),
            Some(&task.id),
            &serde_json::json!({
                "from": message.from_display_name,
                "kind": message.kind,
                "priority": message.priority,
                "display_content": message_display_content(&message.parts),
            }),
            &now,
        )?;
        append_event(
            &tx,
            &target.id,
            "task_queued",
            Some(&message.id),
            Some(&task.id),
            &serde_json::json!({}),
            &now,
        )?;
        tx.execute(
            "UPDATE agent_conversations SET unread_count = unread_count + 1, updated_at = ?1 WHERE id = ?2",
            params![now, target.id],
        )?;
        tx.execute(
            "UPDATE agent_conversations SET updated_at = ?1 WHERE id = ?2",
            params![now, source.id],
        )?;
        tx.commit()?;

        Ok(DeliveryReceipt {
            message,
            task,
            target_conversation: target,
            replayed: false,
        })
    }

    pub fn observe(
        &self,
        conversation_id: &str,
        after_sequence: i64,
    ) -> Result<MessageObservation> {
        let db = self.storage.conn();
        let conversation = conversation_by_id(&db, conversation_id)?
            .with_context(|| format!("agent conversation '{conversation_id}' not found"))?;
        let mut statement = db.prepare(
            "SELECT sequence, conversation_id, event_type, message_id, task_id, payload, created_at
             FROM agent_message_events
             WHERE conversation_id = ?1 AND sequence > ?2
             ORDER BY sequence ASC",
        )?;
        let events = statement
            .query_map(params![conversation_id, after_sequence], event_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next_sequence = events
            .last()
            .map(|event| event.sequence)
            .unwrap_or(after_sequence);
        Ok(MessageObservation {
            conversation,
            events,
            next_sequence,
        })
    }

    pub fn command(&self, task_id: &str, command: AgentTaskCommand) -> Result<AgentMessageTask> {
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        let mut task = task_by_id(&tx, task_id)?
            .with_context(|| format!("agent message task '{task_id}' not found"))?;
        if task.status.is_terminal() {
            bail!("agent message task '{task_id}' is already terminal");
        }

        let (next_status, output_message_id, error) = transition(&task, command)?;
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE agent_message_tasks
             SET status = ?1, output_message_id = ?2, error = ?3, updated_at = ?4
             WHERE id = ?5",
            params![next_status.as_str(), output_message_id, error, now, task_id],
        )?;
        append_task_event(
            &tx,
            &task,
            &format!("task_{}", next_status.as_str()),
            &serde_json::json!({ "error": error }),
            &now,
        )?;
        tx.commit()?;
        task.status = next_status;
        task.output_message_id = output_message_id;
        task.error = error;
        task.updated_at = now;
        Ok(task)
    }

    /// Atomically claims the oldest queued delivery for this process.
    ///
    /// The status predicate on the update is deliberate: even if this method
    /// is later called by multiple workers, a task can only transition from
    /// queued to working once.
    pub fn claim_next(&self, worker_id: &str) -> Result<Option<ClaimedAgentMessage>> {
        self.claim_next_matching(worker_id, None)
    }

    pub(crate) fn claim_next_for(
        &self,
        worker_id: &str,
        recipient_agent_id: &str,
    ) -> Result<Option<ClaimedAgentMessage>> {
        self.claim_next_matching(worker_id, Some(recipient_agent_id))
    }

    pub(crate) fn queued_recipient_ids(&self) -> Result<Vec<String>> {
        let db = self.storage.conn();
        let mut statement = db.prepare(
            "SELECT task.recipient_agent_id
             FROM agent_message_tasks AS task
             JOIN agent_messages AS message ON message.id = task.message_id
             WHERE task.status = 'queued'
             GROUP BY task.recipient_agent_id
             ORDER BY MAX(message.priority) DESC, MIN(task.created_at) ASC",
        )?;
        Ok(statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn claim_next_matching(
        &self,
        worker_id: &str,
        recipient_agent_id: Option<&str>,
    ) -> Result<Option<ClaimedAgentMessage>> {
        if worker_id.trim().is_empty() {
            bail!("worker_id must not be empty");
        }
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        let task_id = if let Some(recipient_agent_id) = recipient_agent_id {
            tx.query_row(
                "SELECT task.id FROM agent_message_tasks AS task
                 JOIN agent_messages AS message ON message.id = task.message_id
                 WHERE task.status = 'queued' AND task.recipient_agent_id = ?1
                 ORDER BY message.priority DESC, task.created_at ASC, task.id ASC
                 LIMIT 1",
                params![recipient_agent_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        } else {
            tx.query_row(
                "SELECT task.id FROM agent_message_tasks AS task
                 JOIN agent_messages AS message ON message.id = task.message_id
                 WHERE task.status = 'queued'
                 ORDER BY message.priority DESC, task.created_at ASC, task.id ASC
                 LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        };
        let Some(task_id) = task_id else {
            tx.commit()?;
            return Ok(None);
        };
        let now = Utc::now().to_rfc3339();
        let changed = tx.execute(
            "UPDATE agent_message_tasks
             SET status = 'working', attempt_count = attempt_count + 1,
                 worker_id = ?1, error = '', updated_at = ?2
             WHERE id = ?3 AND status = 'queued'",
            params![worker_id, now, task_id],
        )?;
        if changed == 0 {
            tx.commit()?;
            return Ok(None);
        }
        let task = task_by_id(&tx, &task_id)?.context("claimed agent task disappeared")?;
        let message = message_by_id(&tx, &task.message_id)?
            .context("message for claimed agent task is missing")?;
        let target_conversation = conversation_by_id(&tx, &task.recipient_conversation_id)?
            .context("conversation for claimed agent task is missing")?;
        append_task_event(
            &tx,
            &task,
            "task_working",
            &serde_json::json!({
                "worker_id": worker_id,
                "attempt_count": task.attempt_count,
            }),
            &now,
        )?;
        tx.commit()?;
        Ok(Some(ClaimedAgentMessage {
            message,
            task,
            target_conversation,
        }))
    }

    /// Marks work that was active when the process stopped as ambiguous.
    /// It is intentionally not requeued automatically because a custom agent
    /// may have completed workspace or external side effects before shutdown.
    pub fn recover_interrupted(&self, reason: &str) -> Result<Vec<AgentMessageTask>> {
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        let mut statement = tx.prepare(
            "SELECT id, message_id, recipient_agent_id, recipient_conversation_id,
                    status, output_message_id, error, attempt_count, worker_id,
                    created_at, updated_at
             FROM agent_message_tasks WHERE status = 'working'
             ORDER BY created_at ASC, id ASC",
        )?;
        let interrupted = statement
            .query_map([], task_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let now = Utc::now().to_rfc3339();
        let mut recovered = Vec::with_capacity(interrupted.len());
        for mut task in interrupted {
            tx.execute(
                "UPDATE agent_message_tasks
                 SET status = 'needs_attention', error = ?1, worker_id = '', updated_at = ?2
                 WHERE id = ?3 AND status = 'working'",
                params![reason, now, task.id],
            )?;
            append_task_event(
                &tx,
                &task,
                "task_needs_attention",
                &serde_json::json!({ "error": reason, "recovered": true }),
                &now,
            )?;
            task.status = AgentTaskStatus::NeedsAttention;
            task.error = reason.to_string();
            task.worker_id.clear();
            task.updated_at = now.clone();
            recovered.push(task);
        }
        tx.commit()?;
        Ok(recovered)
    }

    pub fn retry(&self, task_id: &str) -> Result<AgentMessageTask> {
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        let mut task = task_by_id(&tx, task_id)?
            .with_context(|| format!("agent message task '{task_id}' not found"))?;
        if !matches!(
            task.status,
            AgentTaskStatus::NeedsAttention | AgentTaskStatus::Failed
        ) {
            bail!(
                "agent message task '{task_id}' cannot be retried from {:?}",
                task.status
            );
        }
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE agent_message_tasks
             SET status = 'queued', output_message_id = NULL, error = '',
                 worker_id = '', updated_at = ?1 WHERE id = ?2",
            params![now, task_id],
        )?;
        append_task_event(
            &tx,
            &task,
            "task_queued",
            &serde_json::json!({ "retry": true, "previous_attempts": task.attempt_count }),
            &now,
        )?;
        tx.commit()?;
        task.status = AgentTaskStatus::Queued;
        task.output_message_id = None;
        task.error.clear();
        task.worker_id.clear();
        task.updated_at = now;
        Ok(task)
    }

    pub fn mark_read(&self, conversation_id: &str) -> Result<()> {
        let db = self.storage.conn();
        let changed = db.execute(
            "UPDATE agent_conversations SET unread_count = 0 WHERE id = ?1",
            params![conversation_id],
        )?;
        if changed == 0 {
            bail!("agent conversation '{conversation_id}' not found");
        }
        Ok(())
    }

    pub fn message(&self, message_id: &str) -> Result<AgentMessage> {
        let db = self.storage.conn();
        message_by_id(&db, message_id)?
            .with_context(|| format!("agent message '{message_id}' not found"))
    }

    pub fn task(&self, task_id: &str) -> Result<AgentMessageTask> {
        let db = self.storage.conn();
        task_by_id(&db, task_id)?
            .with_context(|| format!("agent message task '{task_id}' not found"))
    }

    pub fn working_tasks_for_context(&self, context_id: &str) -> Result<Vec<AgentMessageTask>> {
        self.tasks_for_context_statuses(context_id, &["working"])
    }

    pub fn active_tasks_for_context(&self, context_id: &str) -> Result<Vec<AgentMessageTask>> {
        self.tasks_for_context_statuses(
            context_id,
            &["queued", "working", "input_required", "needs_attention"],
        )
    }

    fn tasks_for_context_statuses(
        &self,
        context_id: &str,
        statuses: &[&str],
    ) -> Result<Vec<AgentMessageTask>> {
        let db = self.storage.conn();
        let placeholders = (0..statuses.len()).map(|index| format!("?{}", index + 2)).collect::<Vec<_>>().join(", ");
        let sql = format!("SELECT task.id, task.message_id, task.recipient_agent_id,
                    task.recipient_conversation_id, task.status, task.output_message_id,
                    task.error, task.attempt_count, task.worker_id, task.created_at,
                    task.updated_at
             FROM agent_message_tasks AS task
             JOIN agent_messages AS message ON message.id = task.message_id
             WHERE message.context_id = ?1 AND task.status IN ({placeholders})");
        let mut statement = db.prepare(&sql)?;
        let mut values: Vec<&dyn rusqlite::ToSql> = vec![&context_id];
        values.extend(statuses.iter().map(|status| status as &dyn rusqlite::ToSql));
        Ok(statement.query_map(values.as_slice(), task_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delivery(&self, message_id: &str) -> Result<DeliveryReceipt> {
        let db = self.storage.conn();
        let message = message_by_id(&db, message_id)?
            .with_context(|| format!("agent message '{message_id}' not found"))?;
        let task = task_by_message_id(&db, message_id)?
            .with_context(|| format!("task for agent message '{message_id}' not found"))?;
        let target_conversation = conversation_by_id(&db, &message.target_conversation_id)?
            .context("target conversation for message is missing")?;
        Ok(DeliveryReceipt { message, task, target_conversation, replayed: false })
    }

    pub fn messages_for_context(&self, context_id: &str) -> Result<Vec<AgentMessage>> {
        let db = self.storage.conn();
        let mut statement = db.prepare(
            "SELECT id, schema_version, context_id, from_agent_id, from_revision_id,
                    from_display_name, to_agent_id, to_revision_id, to_display_name,
                    kind, parts, correlation_id, reply_to, source_conversation_id,
                    target_conversation_id, project_id, idempotency_key, hop_count,
                    priority, created_at
             FROM agent_messages WHERE context_id = ?1
             ORDER BY created_at ASC, id ASC",
        )?;
        Ok(statement
            .query_map(params![context_id], message_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn cancel_context_tasks(&self, context_id: &str, reason: &str) -> Result<usize> {
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        let now = Utc::now().to_rfc3339();
        let mut statement = tx.prepare(
            "SELECT task.id, task.message_id, task.recipient_agent_id,
                    task.recipient_conversation_id, task.status, task.output_message_id,
                    task.error, task.attempt_count, task.worker_id, task.created_at,
                    task.updated_at
             FROM agent_message_tasks AS task
             JOIN agent_messages AS message ON message.id = task.message_id
             WHERE message.context_id = ?1
               AND task.status IN ('queued', 'working', 'input_required', 'needs_attention')",
        )?;
        let tasks = statement
            .query_map(params![context_id], task_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for task in &tasks {
            tx.execute(
                "UPDATE agent_message_tasks SET status = 'cancelled', error = ?1,
                        worker_id = '', updated_at = ?2 WHERE id = ?3",
                params![reason, now, task.id],
            )?;
            append_task_event(
                &tx,
                task,
                "task_cancelled",
                &serde_json::json!({ "error": reason, "swarm_cancelled": true }),
                &now,
            )?;
        }
        tx.commit()?;
        Ok(tasks.len())
    }
}

fn validate_send_command(command: &SendAgentMessage) -> Result<()> {
    if command.idempotency_key.trim().is_empty() {
        bail!("idempotency_key must not be empty");
    }
    if command.parts.is_empty() {
        bail!("agent message must contain at least one part");
    }
    if command.hop_count == 0 || command.hop_count > MAX_AGENT_MESSAGE_HOPS {
        bail!("agent message hop_count must be between 1 and {MAX_AGENT_MESSAGE_HOPS}");
    }
    let encoded = serde_json::to_vec(&command.parts)?;
    if encoded.len() > MAX_MESSAGE_BYTES {
        bail!("agent message exceeds the {MAX_MESSAGE_BYTES}-byte limit");
    }
    Ok(())
}

fn transition(
    task: &AgentMessageTask,
    command: AgentTaskCommand,
) -> Result<(AgentTaskStatus, Option<String>, String)> {
    match (task.status, command) {
        (AgentTaskStatus::Working, AgentTaskCommand::RequireInput { reason }) => {
            Ok((AgentTaskStatus::InputRequired, None, reason))
        }
        (AgentTaskStatus::Working, AgentTaskCommand::NeedsAttention { reason }) => {
            Ok((AgentTaskStatus::NeedsAttention, None, reason))
        }
        (
            AgentTaskStatus::Working | AgentTaskStatus::InputRequired,
            AgentTaskCommand::Complete { output_message_id },
        ) => Ok((AgentTaskStatus::Completed, output_message_id, String::new())),
        (
            AgentTaskStatus::Queued
            | AgentTaskStatus::Working
            | AgentTaskStatus::InputRequired
            | AgentTaskStatus::NeedsAttention,
            AgentTaskCommand::Fail { error },
        ) => Ok((AgentTaskStatus::Failed, None, error)),
        (
            AgentTaskStatus::Queued
            | AgentTaskStatus::Working
            | AgentTaskStatus::InputRequired
            | AgentTaskStatus::NeedsAttention,
            AgentTaskCommand::Cancel { reason },
        ) => Ok((AgentTaskStatus::Cancelled, None, reason)),
        (status, command) => {
            bail!("invalid agent task transition from {status:?} using {command:?}")
        }
    }
}

fn open_conversation_tx(
    tx: &Transaction<'_>,
    agent: &agent_registry::AgentDef,
    project_id: &str,
) -> Result<AgentConversation> {
    if let Some(existing) = conversation_by_agent_project(tx, &agent.id, project_id)? {
        return Ok(existing);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let session_id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let cwd = if project_id.is_empty() || project_id == "__adhoc_chat__" {
        String::new()
    } else {
        tx.query_row(
            "SELECT path FROM projects WHERE id = ?1",
            params![project_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .with_context(|| format!("project '{project_id}' not found"))?
    };
    tx.execute(
        "INSERT INTO sessions
         (id, title, start_time, message_count, prompt_count, cwd, model_used,
          parent_session_id, session_type, project_id, mode, created_at, updated_at)
         VALUES (?1, ?2, ?3, 0, 0, ?4, ?5, '', 'agent', ?6, 'build', ?3, ?3)",
        params![session_id, agent.name, now, cwd, agent.model, project_id],
    )?;
    tx.execute(
        "INSERT INTO agent_conversations
         (id, agent_id, project_id, session_id, unread_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)",
        params![id, agent.id, project_id, session_id, now],
    )?;
    Ok(AgentConversation {
        id,
        agent_id: agent.id.clone(),
        project_id: project_id.to_string(),
        session_id,
        unread_count: 0,
        created_at: now.clone(),
        updated_at: now,
    })
}

fn conversation_by_agent_project(
    db: &rusqlite::Connection,
    agent_id: &str,
    project_id: &str,
) -> Result<Option<AgentConversation>> {
    db.query_row(
        "SELECT id, agent_id, project_id, session_id, unread_count, created_at, updated_at
         FROM agent_conversations WHERE agent_id = ?1 AND project_id = ?2",
        params![agent_id, project_id],
        conversation_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn conversation_by_id(
    db: &rusqlite::Connection,
    conversation_id: &str,
) -> Result<Option<AgentConversation>> {
    db.query_row(
        "SELECT id, agent_id, project_id, session_id, unread_count, created_at, updated_at
         FROM agent_conversations WHERE id = ?1",
        params![conversation_id],
        conversation_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn conversation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentConversation> {
    Ok(AgentConversation {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        project_id: row.get(2)?,
        session_id: row.get(3)?,
        unread_count: row.get::<_, i64>(4)? as u32,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn insert_message(tx: &Transaction<'_>, message: &AgentMessage) -> Result<()> {
    tx.execute(
        "INSERT INTO agent_messages
         (id, schema_version, context_id, from_agent_id, from_revision_id,
          from_display_name, to_agent_id, to_revision_id, to_display_name, kind,
          parts, correlation_id, reply_to, source_conversation_id,
          target_conversation_id, project_id, idempotency_key, hop_count, priority, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
        params![
            message.id,
            message.schema_version,
            message.context_id,
            message.from_agent_id,
            message.from_revision_id,
            message.from_display_name,
            message.to_agent_id,
            message.to_revision_id,
            message.to_display_name,
            message.kind.as_str(),
            serde_json::to_string(&message.parts)?,
            message.correlation_id,
            message.reply_to,
            message.source_conversation_id,
            message.target_conversation_id,
            message.project_id,
            message.idempotency_key,
            message.hop_count as i64,
            message.priority as i64,
            message.created_at,
        ],
    )?;
    Ok(())
}

fn insert_task(tx: &Transaction<'_>, task: &AgentMessageTask) -> Result<()> {
    tx.execute(
        "INSERT INTO agent_message_tasks
         (id, message_id, recipient_agent_id, recipient_conversation_id, status,
          output_message_id, error, attempt_count, worker_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            task.id,
            task.message_id,
            task.recipient_agent_id,
            task.recipient_conversation_id,
            task.status.as_str(),
            task.output_message_id,
            task.error,
            task.attempt_count,
            task.worker_id,
            task.created_at,
            task.updated_at,
        ],
    )?;
    Ok(())
}

fn append_event(
    tx: &Transaction<'_>,
    conversation_id: &str,
    event_type: &str,
    message_id: Option<&str>,
    task_id: Option<&str>,
    payload: &serde_json::Value,
    created_at: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO agent_message_events
         (conversation_id, event_type, message_id, task_id, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            conversation_id,
            event_type,
            message_id,
            task_id,
            serde_json::to_string(payload)?,
            created_at,
        ],
    )?;
    Ok(())
}

fn append_task_event(
    tx: &Transaction<'_>,
    task: &AgentMessageTask,
    event_type: &str,
    payload: &serde_json::Value,
    created_at: &str,
) -> Result<()> {
    append_event(
        tx,
        &task.recipient_conversation_id,
        event_type,
        Some(&task.message_id),
        Some(&task.id),
        payload,
        created_at,
    )?;
    let message =
        message_by_id(tx, &task.message_id)?.context("message for agent task is missing")?;
    if message.source_conversation_id != task.recipient_conversation_id {
        append_event(
            tx,
            &message.source_conversation_id,
            event_type,
            Some(&task.message_id),
            Some(&task.id),
            payload,
            created_at,
        )?;
    }
    Ok(())
}

fn message_display_content(parts: &[MessagePart]) -> String {
    parts
        .iter()
        .map(|part| match part {
            MessagePart::Text { text } => text.clone(),
            MessagePart::Data { value } => value.to_string(),
            MessagePart::File { artifact_id } => format!("[Artifact: {artifact_id}]"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn message_by_id(db: &rusqlite::Connection, id: &str) -> Result<Option<AgentMessage>> {
    db.query_row(
        "SELECT id, schema_version, context_id, from_agent_id, from_revision_id,
                from_display_name, to_agent_id, to_revision_id, to_display_name,
                kind, parts, correlation_id, reply_to, source_conversation_id,
                target_conversation_id, project_id, idempotency_key, hop_count,
                priority, created_at
         FROM agent_messages WHERE id = ?1",
        params![id],
        message_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn message_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentMessage> {
    let kind: String = row.get(9)?;
    let parts: String = row.get(10)?;
    Ok(AgentMessage {
        id: row.get(0)?,
        schema_version: row.get(1)?,
        context_id: row.get(2)?,
        from_agent_id: row.get(3)?,
        from_revision_id: row.get(4)?,
        from_display_name: row.get(5)?,
        to_agent_id: row.get(6)?,
        to_revision_id: row.get(7)?,
        to_display_name: row.get(8)?,
        kind: MessageKind::from_str(&kind).map_err(to_sql_conversion_error)?,
        parts: serde_json::from_str(&parts).map_err(to_sql_conversion_error)?,
        correlation_id: row.get(11)?,
        reply_to: row.get(12)?,
        source_conversation_id: row.get(13)?,
        target_conversation_id: row.get(14)?,
        project_id: row.get(15)?,
        idempotency_key: row.get(16)?,
        hop_count: row.get::<_, i64>(17)? as u8,
        priority: row.get::<_, i64>(18)? != 0,
        created_at: row.get(19)?,
    })
}

fn task_by_id(db: &rusqlite::Connection, id: &str) -> Result<Option<AgentMessageTask>> {
    db.query_row(
        "SELECT id, message_id, recipient_agent_id, recipient_conversation_id,
                status, output_message_id, error, attempt_count, worker_id,
                created_at, updated_at
         FROM agent_message_tasks WHERE id = ?1",
        params![id],
        task_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn task_by_message_id(
    db: &rusqlite::Connection,
    message_id: &str,
) -> Result<Option<AgentMessageTask>> {
    db.query_row(
        "SELECT id, message_id, recipient_agent_id, recipient_conversation_id,
                status, output_message_id, error, attempt_count, worker_id,
                created_at, updated_at
         FROM agent_message_tasks WHERE message_id = ?1",
        params![message_id],
        task_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentMessageTask> {
    let status: String = row.get(4)?;
    Ok(AgentMessageTask {
        id: row.get(0)?,
        message_id: row.get(1)?,
        recipient_agent_id: row.get(2)?,
        recipient_conversation_id: row.get(3)?,
        status: AgentTaskStatus::from_str(&status).map_err(to_sql_conversion_error)?,
        output_message_id: row.get(5)?,
        error: row.get(6)?,
        attempt_count: row.get::<_, i64>(7)? as u32,
        worker_id: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn delivery_by_idempotency_key(
    db: &rusqlite::Connection,
    key: &str,
) -> Result<Option<(AgentMessage, AgentMessageTask)>> {
    let message = db
        .query_row(
            "SELECT id, schema_version, context_id, from_agent_id, from_revision_id,
                    from_display_name, to_agent_id, to_revision_id, to_display_name,
                    kind, parts, correlation_id, reply_to, source_conversation_id,
                    target_conversation_id, project_id, idempotency_key, hop_count,
                    priority, created_at
             FROM agent_messages WHERE idempotency_key = ?1",
            params![key],
            message_from_row,
        )
        .optional()?;
    let Some(message) = message else {
        return Ok(None);
    };
    let task = db.query_row(
        "SELECT id, message_id, recipient_agent_id, recipient_conversation_id,
                status, output_message_id, error, attempt_count, worker_id,
                created_at, updated_at
         FROM agent_message_tasks WHERE message_id = ?1",
        params![message.id],
        task_from_row,
    )?;
    Ok(Some((message, task)))
}

fn event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentMessageEvent> {
    let payload: String = row.get(5)?;
    Ok(AgentMessageEvent {
        sequence: row.get(0)?,
        conversation_id: row.get(1)?,
        event_type: row.get(2)?,
        message_id: row.get(3)?,
        task_id: row.get(4)?,
        payload: serde_json::from_str(&payload).map_err(to_sql_conversion_error)?,
        created_at: row.get(6)?,
    })
}

fn to_sql_conversion_error(
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
