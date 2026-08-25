use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::{
    ActiveAgentRuns, AgentMessaging, AgentRunLane, AgentTaskCommand, ClaimedAgentMessage,
    MessageKind, MessagePart, PeerMessageRoute, SendAgentMessage,
};

#[async_trait]
pub trait AgentMessageExecutor: Send + Sync {
    async fn execute(
        &self,
        delivery: &ClaimedAgentMessage,
        cancel_token: CancellationToken,
    ) -> Result<String>;
}

/// Drains the durable agent inbox without coupling protocol state to Tauri.
///
/// `Notify` makes local sends responsive; the periodic wake is the durable
/// fallback for startup work and notifications lost around an empty-queue
/// check.
pub struct AgentInboxDispatcher {
    messaging: AgentMessaging,
    executor: Arc<dyn AgentMessageExecutor>,
    active_runs: ActiveAgentRuns,
    worker_id: String,
    wake: Notify,
    recipient_workers: parking_lot::Mutex<HashSet<String>>,
    swarm: Option<crate::SwarmCoordinator>,
}

impl AgentInboxDispatcher {
    pub fn new(
        messaging: AgentMessaging,
        executor: Arc<dyn AgentMessageExecutor>,
        active_runs: ActiveAgentRuns,
        worker_id: impl Into<String>,
    ) -> Self {
        Self {
            messaging,
            executor,
            active_runs,
            worker_id: worker_id.into(),
            wake: Notify::new(),
            recipient_workers: parking_lot::Mutex::new(HashSet::new()),
            swarm: None,
        }
    }

    pub fn with_swarm(mut self, swarm: crate::SwarmCoordinator) -> Self {
        self.swarm = Some(swarm);
        self
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// Routes a newly durable peer message against the recipient's active lane,
    /// then wakes the inbox consumer. User turns are never interrupted.
    pub fn route_peer_message(&self, agent_id: &str, priority: bool) -> PeerMessageRoute {
        let route = self.active_runs.route_peer_message(agent_id, priority);
        self.wake();
        route
    }

    pub async fn process_next(&self) -> Result<bool> {
        let Some(recipient_agent_id) = self.messaging.queued_recipient_ids()?.into_iter().next()
        else {
            return Ok(false);
        };
        self.process_next_for(&recipient_agent_id).await
    }

    async fn process_next_for(&self, recipient_agent_id: &str) -> Result<bool> {
        let cancel_token = CancellationToken::new();
        let mut lease = self.active_runs.reserve(recipient_agent_id).await;
        let Some(delivery) = self
            .messaging
            .claim_next_for(&self.worker_id, recipient_agent_id)?
        else {
            lease.finish();
            return Ok(false);
        };
        lease.activate(
            delivery.task.id.clone(),
            AgentRunLane::Peer,
            cancel_token.clone(),
        );
        self.process_claimed(delivery, cancel_token, lease).await?;
        Ok(true)
    }

    async fn process_claimed(
        &self,
        delivery: ClaimedAgentMessage,
        cancel_token: CancellationToken,
        lease: super::ActiveAgentRunLease,
    ) -> Result<()> {
        if let Some(swarm) = &self.swarm
            && let Err(error) = swarm.begin_turn(
                &delivery.message.context_id,
                &delivery.target_conversation.agent_id,
                &delivery.task.id,
                AgentRunLane::Peer,
            )
        {
            lease.finish();
            self.messaging.command(
                &delivery.task.id,
                AgentTaskCommand::NeedsAttention {
                    reason: error.to_string(),
                },
            )?;
            return Ok(());
        }
        let execution = self.executor.execute(&delivery, cancel_token).await;
        if let Some(swarm) = &self.swarm
            && let Err(error) = swarm.finish_turn(&delivery.message.context_id, &delivery.task.id)
        {
            lease.finish();
            self.messaging.command(
                &delivery.task.id,
                AgentTaskCommand::NeedsAttention {
                    reason: format!("agent executed but turn finalization failed: {error}"),
                },
            )?;
            return Ok(());
        }
        let was_preempted = lease.was_preempted();
        lease.finish();
        let output = match execution {
            Ok(output) => output,
            Err(error) => {
                if self.messaging.task(&delivery.task.id)?.status
                    == super::AgentTaskStatus::Cancelled
                {
                    return Ok(());
                }
                if let Some(swarm) = &self.swarm {
                    swarm.mark_needs_attention(&delivery.message.context_id, &error.to_string())?;
                }
                let command = if was_preempted {
                    AgentTaskCommand::NeedsAttention {
                        reason: format!(
                            "preempted by a priority peer message; execution may have produced side effects: {error}"
                        ),
                    }
                } else {
                    AgentTaskCommand::Fail {
                        error: error.to_string(),
                    }
                };
                self.messaging.command(&delivery.task.id, command)?;
                return Ok(());
            }
        };

        let completion = (|| -> Result<()> {
            let output_message_id = if delivery.message.kind == MessageKind::Request {
                let reply = if let Some(swarm) = &self.swarm {
                    swarm.reply(&delivery.message, &delivery.target_conversation.id, output)?
                } else {
                    self.messaging.send(SendAgentMessage {
                        source_conversation_id: delivery.target_conversation.id.clone(),
                        to_agent_id: delivery.message.from_agent_id.clone(),
                        kind: MessageKind::Reply,
                        parts: vec![MessagePart::text(output)],
                        context_id: Some(delivery.message.context_id.clone()),
                        correlation_id: Some(delivery.message.correlation_id.clone()),
                        reply_to: Some(delivery.message.id.clone()),
                        idempotency_key: format!("agent-reply:{}", delivery.message.id),
                        hop_count: delivery.message.hop_count + 1,
                        priority: false,
                    })?
                };
                Some(reply.message.id)
            } else {
                None
            };

            self.messaging.command(
                &delivery.task.id,
                AgentTaskCommand::Complete { output_message_id },
            )?;
            if let Some(swarm) = &self.swarm {
                swarm.finalize_completion(&delivery.message.context_id, &delivery.task.id)?;
            }
            Ok(())
        })();
        if let Err(error) = completion {
            let reason = format!("agent executed but delivery finalization failed: {error}");
            if self.messaging.task(&delivery.task.id)?.status.is_terminal() {
                // The reply and task completion are already durable. Keep a
                // Completing swarm recoverable so startup reconciliation can
                // promote it instead of turning a successful delivery ambiguous.
                eprintln!("[agent-messaging] {reason}");
            } else {
                self.messaging.command(
                    &delivery.task.id,
                    AgentTaskCommand::NeedsAttention {
                        reason: reason.clone(),
                    },
                )?;
                if let Some(swarm) = &self.swarm {
                    swarm.mark_needs_attention(&delivery.message.context_id, &reason)?;
                }
            }
        }
        Ok(())
    }

    pub async fn drain_available(&self) -> Result<usize> {
        let mut processed = 0;
        while self.process_next().await? {
            processed += 1;
        }
        Ok(processed)
    }

    async fn drain_recipient(&self, recipient_agent_id: &str) -> Result<()> {
        while self.process_next_for(recipient_agent_id).await? {
            self.wake();
        }
        Ok(())
    }

    fn schedule_recipient_workers(self: &Arc<Self>) -> Result<()> {
        for recipient_agent_id in self.messaging.queued_recipient_ids()? {
            if !self
                .recipient_workers
                .lock()
                .insert(recipient_agent_id.clone())
            {
                continue;
            }
            let dispatcher = self.clone();
            tokio::spawn(async move {
                let result = dispatcher.drain_recipient(&recipient_agent_id).await;
                dispatcher
                    .recipient_workers
                    .lock()
                    .remove(&recipient_agent_id);
                dispatcher.wake();
                if let Err(error) = result {
                    eprintln!(
                        "[agent-messaging] recipient {recipient_agent_id} dispatcher error: {error}"
                    );
                }
            });
        }
        Ok(())
    }

    pub async fn run(self: Arc<Self>) {
        loop {
            if let Err(error) = self.schedule_recipient_workers() {
                eprintln!("[agent-messaging] dispatcher error: {error}");
            }
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::sync::Notify as TokioNotify;

    use super::*;
    use crate::{
        AgentConversation,
        agent_registry::{self, AgentDef},
        memory::storage::Storage,
    };

    struct RecordingExecutor {
        deliveries: Mutex<Vec<MessageKind>>,
    }

    struct BlockingExecutor {
        started: TokioNotify,
    }

    struct PriorityRecordingExecutor {
        priorities: Mutex<Vec<bool>>,
    }

    struct RecipientConcurrencyExecutor {
        debugger_started: TokioNotify,
        release_debugger: TokioNotify,
        coder_processed: TokioNotify,
    }

    #[async_trait]
    impl AgentMessageExecutor for BlockingExecutor {
        async fn execute(
            &self,
            _delivery: &ClaimedAgentMessage,
            cancel_token: CancellationToken,
        ) -> Result<String> {
            self.started.notify_one();
            cancel_token.cancelled().await;
            anyhow::bail!("cancelled")
        }
    }

    #[async_trait]
    impl AgentMessageExecutor for PriorityRecordingExecutor {
        async fn execute(
            &self,
            delivery: &ClaimedAgentMessage,
            _cancel_token: CancellationToken,
        ) -> Result<String> {
            self.priorities
                .lock()
                .unwrap()
                .push(delivery.message.priority);
            Ok("processed".into())
        }
    }

    #[async_trait]
    impl AgentMessageExecutor for RecipientConcurrencyExecutor {
        async fn execute(
            &self,
            delivery: &ClaimedAgentMessage,
            _cancel_token: CancellationToken,
        ) -> Result<String> {
            if delivery.task.recipient_agent_id == "debugger" {
                self.debugger_started.notify_one();
                self.release_debugger.notified().await;
            } else if delivery.task.recipient_agent_id == "coder" {
                self.coder_processed.notify_one();
            }
            Ok("processed".into())
        }
    }

    #[async_trait]
    impl AgentMessageExecutor for RecordingExecutor {
        async fn execute(
            &self,
            delivery: &ClaimedAgentMessage,
            _cancel_token: CancellationToken,
        ) -> Result<String> {
            self.deliveries.lock().unwrap().push(delivery.message.kind);
            Ok(match delivery.message.kind {
                MessageKind::Request => "Debugger found the panic".to_string(),
                MessageKind::Reply | MessageKind::Notification => "relayed".to_string(),
            })
        }
    }

    fn setup() -> (tempfile::TempDir, AgentMessaging, AgentConversation) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Storage::new(directory.path().join("test.db").to_str().unwrap()).unwrap();
        for (id, name) in [("coder", "Coder"), ("debugger", "Debugger")] {
            agent_registry::create(
                &storage,
                &AgentDef {
                    id: id.into(),
                    name: name.into(),
                    ..AgentDef::default()
                },
            )
            .unwrap();
        }
        let messaging = AgentMessaging::new(storage);
        let source = messaging
            .open_conversation("coder", Some("__adhoc_chat__"))
            .unwrap();
        (directory, messaging, source)
    }

    #[tokio::test]
    async fn request_and_reply_are_separate_durable_dispatch_steps() {
        let (_directory, messaging, source) = setup();
        messaging
            .send(SendAgentMessage {
                source_conversation_id: source.id.clone(),
                to_agent_id: "debugger".into(),
                kind: MessageKind::Request,
                parts: vec![MessagePart::text("inspect panic")],
                context_id: None,
                correlation_id: None,
                reply_to: None,
                idempotency_key: "request".into(),
                hop_count: 1,
                priority: false,
            })
            .unwrap();
        let executor = Arc::new(RecordingExecutor {
            deliveries: Mutex::new(Vec::new()),
        });
        let dispatcher = AgentInboxDispatcher::new(
            messaging.clone(),
            executor.clone(),
            ActiveAgentRuns::new(),
            "test-worker",
        );

        assert!(dispatcher.process_next().await.unwrap());
        assert_eq!(
            *executor.deliveries.lock().unwrap(),
            vec![MessageKind::Request]
        );
        let source_after_reply = messaging.observe(&source.id, 0).unwrap();
        assert!(
            source_after_reply
                .events
                .iter()
                .any(|event| event.event_type == "message_received")
        );

        assert!(dispatcher.process_next().await.unwrap());
        assert_eq!(
            *executor.deliveries.lock().unwrap(),
            vec![MessageKind::Request, MessageKind::Reply]
        );
        assert!(!dispatcher.process_next().await.unwrap());
    }

    #[tokio::test]
    async fn finalization_failure_does_not_leave_the_task_working() {
        let (_directory, messaging, source) = setup();
        let delivery = messaging
            .send(SendAgentMessage {
                source_conversation_id: source.id.clone(),
                to_agent_id: "debugger".into(),
                kind: MessageKind::Request,
                parts: vec![MessagePart::text("inspect panic")],
                context_id: None,
                correlation_id: None,
                reply_to: None,
                idempotency_key: "request-at-hop-limit".into(),
                hop_count: super::super::MAX_AGENT_MESSAGE_HOPS,
                priority: false,
            })
            .unwrap();
        let executor = Arc::new(RecordingExecutor {
            deliveries: Mutex::new(Vec::new()),
        });
        let dispatcher = AgentInboxDispatcher::new(
            messaging.clone(),
            executor,
            ActiveAgentRuns::new(),
            "test-worker",
        );

        assert!(dispatcher.process_next().await.unwrap());

        let events = messaging
            .observe(&delivery.target_conversation.id, 0)
            .unwrap();
        assert!(events.events.iter().any(|event| {
            event.task_id.as_deref() == Some(delivery.task.id.as_str())
                && event.event_type == "task_needs_attention"
        }));
        assert!(!dispatcher.process_next().await.unwrap());
    }

    #[tokio::test]
    async fn priority_preemption_marks_the_interrupted_peer_task_for_attention() {
        let (_directory, messaging, source) = setup();
        let interrupted = messaging
            .send(SendAgentMessage {
                source_conversation_id: source.id.clone(),
                to_agent_id: "debugger".into(),
                kind: MessageKind::Request,
                parts: vec![MessagePart::text("long inspection")],
                context_id: None,
                correlation_id: None,
                reply_to: None,
                idempotency_key: "long-request".into(),
                hop_count: 1,
                priority: false,
            })
            .unwrap();
        let active_runs = ActiveAgentRuns::new();
        let executor = Arc::new(BlockingExecutor {
            started: TokioNotify::new(),
        });
        let dispatcher = Arc::new(AgentInboxDispatcher::new(
            messaging.clone(),
            executor.clone(),
            active_runs,
            "test-worker",
        ));
        let processing = tokio::spawn({
            let dispatcher = dispatcher.clone();
            async move { dispatcher.process_next().await }
        });
        executor.started.notified().await;

        assert!(matches!(
            dispatcher.route_peer_message("debugger", true),
            PeerMessageRoute::PreemptedPeer { .. }
        ));
        assert!(processing.await.unwrap().unwrap());

        let observation = messaging
            .observe(&interrupted.target_conversation.id, 0)
            .unwrap();
        assert!(observation.events.iter().any(|event| {
            event.task_id.as_deref() == Some(interrupted.task.id.as_str())
                && event.event_type == "task_needs_attention"
        }));
    }

    #[tokio::test]
    async fn priority_is_reselected_after_a_user_lane_becomes_available() {
        let (_directory, messaging, source) = setup();
        messaging
            .send(SendAgentMessage {
                source_conversation_id: source.id.clone(),
                to_agent_id: "debugger".into(),
                kind: MessageKind::Notification,
                parts: vec![MessagePart::text("normal")],
                context_id: None,
                correlation_id: None,
                reply_to: None,
                idempotency_key: "normal-notification".into(),
                hop_count: 1,
                priority: false,
            })
            .unwrap();
        let active_runs = ActiveAgentRuns::new();
        let user_lease = active_runs
            .enter(
                "debugger",
                "user-run",
                AgentRunLane::User,
                CancellationToken::new(),
            )
            .await;
        let executor = Arc::new(PriorityRecordingExecutor {
            priorities: Mutex::new(Vec::new()),
        });
        let dispatcher = Arc::new(AgentInboxDispatcher::new(
            messaging.clone(),
            executor.clone(),
            active_runs,
            "test-worker",
        ));
        let processing = tokio::spawn({
            let dispatcher = dispatcher.clone();
            async move { dispatcher.process_next().await }
        });
        tokio::task::yield_now().await;
        messaging
            .send(SendAgentMessage {
                source_conversation_id: source.id,
                to_agent_id: "debugger".into(),
                kind: MessageKind::Notification,
                parts: vec![MessagePart::text("priority")],
                context_id: None,
                correlation_id: None,
                reply_to: None,
                idempotency_key: "priority-notification".into(),
                hop_count: 1,
                priority: true,
            })
            .unwrap();

        user_lease.finish();
        assert!(processing.await.unwrap().unwrap());
        assert_eq!(*executor.priorities.lock().unwrap(), vec![true]);
    }

    #[tokio::test]
    async fn a_busy_recipient_does_not_block_an_unrelated_recipient() {
        let (_directory, messaging, coder_source) = setup();
        let debugger_source = messaging
            .open_conversation("debugger", Some("__adhoc_chat__"))
            .unwrap();
        for (source_conversation_id, recipient, key) in [
            (coder_source.id, "debugger", "to-debugger"),
            (debugger_source.id, "coder", "to-coder"),
        ] {
            messaging
                .send(SendAgentMessage {
                    source_conversation_id,
                    to_agent_id: recipient.into(),
                    kind: MessageKind::Notification,
                    parts: vec![MessagePart::text("notification")],
                    context_id: None,
                    correlation_id: None,
                    reply_to: None,
                    idempotency_key: key.into(),
                    hop_count: 1,
                    priority: false,
                })
                .unwrap();
        }
        let executor = Arc::new(RecipientConcurrencyExecutor {
            debugger_started: TokioNotify::new(),
            release_debugger: TokioNotify::new(),
            coder_processed: TokioNotify::new(),
        });
        let dispatcher = Arc::new(AgentInboxDispatcher::new(
            messaging,
            executor.clone(),
            ActiveAgentRuns::new(),
            "test-worker",
        ));
        let run = tokio::spawn(dispatcher.run());

        tokio::time::timeout(Duration::from_secs(1), executor.debugger_started.notified())
            .await
            .expect("debugger execution should start");
        tokio::time::timeout(Duration::from_secs(1), executor.coder_processed.notified())
            .await
            .expect("coder should execute while debugger is still busy");

        executor.release_debugger.notify_one();
        run.abort();
    }
}
