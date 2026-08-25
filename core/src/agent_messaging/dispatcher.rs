use std::{sync::Arc, time::Duration};

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Notify;

use super::{
    AgentMessaging, AgentTaskCommand, ClaimedAgentMessage, MessageKind, MessagePart,
    SendAgentMessage,
};

#[async_trait]
pub trait AgentMessageExecutor: Send + Sync {
    async fn execute(&self, delivery: &ClaimedAgentMessage) -> Result<String>;
}

/// Drains the durable agent inbox without coupling protocol state to Tauri.
///
/// `Notify` makes local sends responsive; the periodic wake is the durable
/// fallback for startup work and notifications lost around an empty-queue
/// check.
pub struct AgentInboxDispatcher {
    messaging: AgentMessaging,
    executor: Arc<dyn AgentMessageExecutor>,
    worker_id: String,
    wake: Notify,
}

impl AgentInboxDispatcher {
    pub fn new(
        messaging: AgentMessaging,
        executor: Arc<dyn AgentMessageExecutor>,
        worker_id: impl Into<String>,
    ) -> Self {
        Self {
            messaging,
            executor,
            worker_id: worker_id.into(),
            wake: Notify::new(),
        }
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub async fn process_next(&self) -> Result<bool> {
        let Some(delivery) = self.messaging.claim_next(&self.worker_id)? else {
            return Ok(false);
        };

        let output = match self.executor.execute(&delivery).await {
            Ok(output) => output,
            Err(error) => {
                self.messaging.command(
                    &delivery.task.id,
                    AgentTaskCommand::Fail {
                        error: error.to_string(),
                    },
                )?;
                return Ok(true);
            }
        };

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
            })?;
            Some(reply.message.id)
        } else {
            None
        };

        self.messaging.command(
            &delivery.task.id,
            AgentTaskCommand::Complete { output_message_id },
        )?;
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

    use super::*;
    use crate::{
        AgentConversation,
        agent_registry::{self, AgentDef},
        memory::storage::Storage,
    };

    struct RecordingExecutor {
        deliveries: Mutex<Vec<MessageKind>>,
    }

    #[async_trait]
    impl AgentMessageExecutor for RecordingExecutor {
        async fn execute(&self, delivery: &ClaimedAgentMessage) -> Result<String> {
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
            })
            .unwrap();
        let executor = Arc::new(RecordingExecutor {
            deliveries: Mutex::new(Vec::new()),
        });
        let dispatcher =
            AgentInboxDispatcher::new(messaging.clone(), executor.clone(), "test-worker");

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
}
