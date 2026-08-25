use std::{sync::Arc, time::Duration};

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
        }
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
        let Some(delivery) = self.messaging.claim_next(&self.worker_id)? else {
            return Ok(false);
        };

        let cancel_token = CancellationToken::new();
        let lease = self
            .active_runs
            .enter(
                delivery.task.recipient_agent_id.clone(),
                delivery.task.id.clone(),
                AgentRunLane::Peer,
                cancel_token.clone(),
            )
            .await;
        let execution = self.executor.execute(&delivery, cancel_token).await;
        let was_preempted = lease.was_preempted();
        lease.finish();
        let output = match execution {
            Ok(output) => output,
            Err(error) => {
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
                return Ok(true);
            }
        };

        let completion = (|| -> Result<()> {
            let output_message_id = if delivery.message.kind == MessageKind::Request {
                let reply = self.messaging.send(SendAgentMessage {
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
                })?;
                Some(reply.message.id)
            } else {
                None
            };

            self.messaging.command(
                &delivery.task.id,
                AgentTaskCommand::Complete { output_message_id },
            )?;
            Ok(())
        })();
        if let Err(error) = completion {
            let reason = format!("agent executed but delivery finalization failed: {error}");
            self.messaging.command(
                &delivery.task.id,
                AgentTaskCommand::NeedsAttention { reason },
            )?;
        }
        Ok(true)
    }

    pub async fn drain_available(&self) -> Result<usize> {
        let mut processed = 0;
        while self.process_next().await? {
            processed += 1;
        }
        Ok(processed)
    }

    pub async fn run(&self) {
        loop {
            if let Err(error) = self.drain_available().await {
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
}
