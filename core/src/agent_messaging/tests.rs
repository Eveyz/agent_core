use tempfile::TempDir;

use super::*;
use crate::agent_registry::AgentDef;

fn service_with_agents() -> (TempDir, AgentMessaging, AgentDef, AgentDef) {
    let directory = tempfile::tempdir().expect("tempdir");
    let storage =
        Storage::new(directory.path().join("test.db").to_str().expect("path")).expect("storage");
    let coder = agent_registry::create(
        &storage,
        &AgentDef {
            id: "coder".into(),
            name: "Coder".into(),
            ..AgentDef::default()
        },
    )
    .expect("coder");
    let debugger = agent_registry::create(
        &storage,
        &AgentDef {
            id: "debugger".into(),
            name: "Debugger".into(),
            ..AgentDef::default()
        },
    )
    .expect("debugger");
    (directory, AgentMessaging::new(storage), coder, debugger)
}

fn request(source: &AgentConversation) -> SendAgentMessage {
    SendAgentMessage {
        source_conversation_id: source.id.clone(),
        to_agent_id: "debugger".into(),
        kind: MessageKind::Request,
        parts: vec![MessagePart::text("Please inspect the panic")],
        context_id: None,
        correlation_id: None,
        reply_to: None,
        idempotency_key: "request-1".into(),
        hop_count: 1,
    }
}

#[test]
fn opening_the_same_agent_scope_resumes_the_same_conversation() {
    let (_directory, messaging, coder, _) = service_with_agents();

    let first = messaging
        .open_conversation(&coder.id, Some("__adhoc_chat__"))
        .expect("open first");
    let second = messaging
        .open_conversation(&coder.id, Some("__adhoc_chat__"))
        .expect("open second");

    assert_eq!(first, second);
    let project_sessions = crate::ProjectManager::new(messaging.storage.clone())
        .list_sessions("__adhoc_chat__")
        .expect("project sessions");
    assert!(project_sessions.is_empty());
}

#[test]
fn sending_is_durable_idempotent_and_visible_to_both_conversations() {
    let (_directory, messaging, coder, debugger) = service_with_agents();
    let source = messaging
        .open_conversation(&coder.id, Some("__adhoc_chat__"))
        .expect("source");

    let first = messaging.send(request(&source)).expect("first delivery");
    let replay = messaging.send(request(&source)).expect("replayed delivery");

    assert!(!first.replayed);
    assert!(replay.replayed);
    assert_eq!(first.message, replay.message);
    assert_eq!(first.task, replay.task);
    assert_eq!(first.message.schema_version, AGENT_MESSAGE_SCHEMA_V1);
    assert_eq!(first.message.from_agent_id, coder.id);
    assert_eq!(first.message.to_agent_id, debugger.id);
    assert_eq!(first.task.status, AgentTaskStatus::Queued);

    let source_events = messaging.observe(&source.id, 0).expect("source events");
    let target_events = messaging
        .observe(&first.target_conversation.id, 0)
        .expect("target events");
    assert_eq!(
        source_events
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["message_sent"]
    );
    assert_eq!(
        target_events
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["message_received", "task_queued"]
    );
    assert_eq!(target_events.conversation.unread_count, 1);
    let conversations = messaging
        .list_conversations("__adhoc_chat__")
        .expect("conversation list");
    assert_eq!(conversations.len(), 2);
    assert_eq!(
        conversations
            .iter()
            .find(|conversation| conversation.agent_id == debugger.id)
            .expect("debugger conversation")
            .unread_count,
        1
    );
}

#[test]
fn reply_reverses_the_route_and_preserves_context_and_correlation() {
    let (_directory, messaging, coder, _) = service_with_agents();
    let source = messaging
        .open_conversation(&coder.id, Some("__adhoc_chat__"))
        .expect("source");
    let original = messaging.send(request(&source)).expect("request");

    let reply = messaging
        .send(SendAgentMessage {
            source_conversation_id: original.target_conversation.id.clone(),
            to_agent_id: coder.id.clone(),
            kind: MessageKind::Reply,
            parts: vec![MessagePart::text("The panic comes from an unchecked index")],
            context_id: None,
            correlation_id: Some(original.message.correlation_id.clone()),
            reply_to: Some(original.message.id.clone()),
            idempotency_key: "reply-1".into(),
            hop_count: 2,
        })
        .expect("reply");

    assert_eq!(reply.message.from_agent_id, original.message.to_agent_id);
    assert_eq!(reply.message.to_agent_id, original.message.from_agent_id);
    assert_eq!(reply.message.context_id, original.message.context_id);
    assert_eq!(
        reply.message.correlation_id,
        original.message.correlation_id
    );
    assert_eq!(reply.message.reply_to, Some(original.message.id));
    assert_eq!(reply.target_conversation.id, source.id);
}

#[test]
fn task_state_machine_and_hop_limit_stop_unbounded_agent_chains() {
    let (_directory, messaging, coder, _) = service_with_agents();
    let source = messaging
        .open_conversation(&coder.id, Some("__adhoc_chat__"))
        .expect("source");
    let delivery = messaging.send(request(&source)).expect("delivery");

    let working = messaging
        .claim_next("test-worker")
        .expect("claim")
        .expect("queued task")
        .task;
    let completed = messaging
        .command(
            &delivery.task.id,
            AgentTaskCommand::Complete {
                output_message_id: None,
            },
        )
        .expect("complete");

    assert_eq!(working.status, AgentTaskStatus::Working);
    assert_eq!(completed.status, AgentTaskStatus::Completed);
    assert!(
        messaging
            .command(
                &delivery.task.id,
                AgentTaskCommand::Fail {
                    error: "too late".into(),
                },
            )
            .expect_err("terminal task")
            .to_string()
            .contains("already terminal")
    );

    let mut over_limit = request(&source);
    over_limit.idempotency_key = "too-many-hops".into();
    over_limit.hop_count = MAX_AGENT_MESSAGE_HOPS + 1;
    assert!(
        messaging
            .send(over_limit)
            .expect_err("hop limit")
            .to_string()
            .contains("hop_count")
    );

    let self_message = SendAgentMessage {
        source_conversation_id: source.id,
        to_agent_id: coder.id,
        kind: MessageKind::Request,
        parts: vec![MessagePart::text("loop")],
        context_id: None,
        correlation_id: None,
        reply_to: None,
        idempotency_key: "self-message".into(),
        hop_count: 1,
    };
    assert!(
        messaging
            .send(self_message)
            .expect_err("self message")
            .to_string()
            .contains("cannot message itself")
    );
}

#[test]
fn queued_delivery_can_only_be_claimed_once() {
    let (_directory, messaging, coder, _) = service_with_agents();
    let source = messaging
        .open_conversation(&coder.id, Some("__adhoc_chat__"))
        .expect("source");
    let delivery = messaging.send(request(&source)).expect("delivery");

    let claimed = messaging
        .claim_next("desktop-worker")
        .expect("claim")
        .expect("queued delivery");

    assert_eq!(claimed.message, delivery.message);
    assert_eq!(claimed.task.status, AgentTaskStatus::Working);
    assert_eq!(claimed.task.attempt_count, 1);
    assert_eq!(claimed.task.worker_id, "desktop-worker");
    assert!(
        messaging
            .claim_next("other-worker")
            .expect("second claim")
            .is_none()
    );
}

#[test]
fn interrupted_work_requires_attention_and_can_be_explicitly_retried() {
    let (_directory, messaging, coder, _) = service_with_agents();
    let source = messaging
        .open_conversation(&coder.id, Some("__adhoc_chat__"))
        .expect("source");
    let delivery = messaging.send(request(&source)).expect("delivery");
    messaging
        .claim_next("desktop-worker")
        .expect("claim")
        .expect("queued delivery");

    let recovered = messaging
        .recover_interrupted("application restarted during agent execution")
        .expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, delivery.task.id);
    assert_eq!(recovered[0].status, AgentTaskStatus::NeedsAttention);
    assert!(
        messaging
            .claim_next("desktop-worker")
            .expect("claim after recovery")
            .is_none()
    );

    let queued = messaging.retry(&delivery.task.id).expect("explicit retry");
    assert_eq!(queued.status, AgentTaskStatus::Queued);
    let claimed_again = messaging
        .claim_next("desktop-worker")
        .expect("reclaim")
        .expect("retried delivery");
    assert_eq!(claimed_again.task.attempt_count, 2);

    let events = messaging
        .observe(&delivery.target_conversation.id, 0)
        .expect("events");
    let event_types = events
        .events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"task_needs_attention"));
    assert!(event_types.contains(&"task_queued"));
}
