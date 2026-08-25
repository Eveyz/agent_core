use agent_core::agent_registry::AgentDef;
use agent_core::memory::storage::Storage;
use agent_core::{
    AgentMessaging, MessagePart, SendAgentMessageTool, StartSwarm, SwarmCommand, SwarmCoordinator,
    SwarmStatus, SwarmToolContext, Tool,
};

fn coordinator_with_agents() -> (tempfile::TempDir, SwarmCoordinator) {
    let directory = tempfile::tempdir().expect("tempdir");
    let storage =
        Storage::new(directory.path().join("test.db").to_str().expect("path")).expect("storage");
    for (id, name) in [
        ("coder", "Coder"),
        ("debugger", "Debugger"),
        ("tester", "Tester"),
    ] {
        agent_core::agent_registry::create(
            &storage,
            &AgentDef {
                id: id.into(),
                name: name.into(),
                ..AgentDef::default()
            },
        )
        .expect("create agent");
    }
    let messaging = AgentMessaging::new(storage.clone());
    (directory, SwarmCoordinator::new(storage, messaging))
}

#[test]
fn start_send_and_observe_persist_the_swarm_boundary() {
    let (_directory, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Diagnose and fix the crash".into(),
            max_messages: 8,
            max_turns: 6,
        })
        .expect("start swarm");

    let snapshot = coordinator
        .command(
            &run.id,
            SwarmCommand::Send {
                from_agent_id: "coder".into(),
                to_agent_id: "debugger".into(),
                parts: vec![MessagePart::text("Find the root cause")],
                priority: false,
                idempotency_key: "swarm-send-1".into(),
            },
        )
        .expect("send from swarm");

    assert_eq!(snapshot.run.status, SwarmStatus::Running);
    assert_eq!(snapshot.run.messages_used, 1);
    assert_eq!(snapshot.participant_agent_ids, vec!["coder", "debugger"]);
    assert_eq!(snapshot.messages.len(), 1);
    assert_eq!(snapshot.messages[0].context_id, run.id);

    let observation = coordinator.observe(&run.id, 0).expect("observe swarm");
    assert_eq!(observation.snapshot, snapshot);
    assert_eq!(
        observation
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["swarm_started", "participant_joined", "message_sent"]
    );
}

#[test]
fn participants_can_expand_the_swarm_and_only_the_root_can_complete_it() {
    let (_directory, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Ship a verified fix".into(),
            max_messages: 8,
            max_turns: 6,
        })
        .expect("start");
    coordinator
        .command(
            &run.id,
            SwarmCommand::Send {
                from_agent_id: "coder".into(),
                to_agent_id: "debugger".into(),
                parts: vec![MessagePart::text("diagnose")],
                priority: false,
                idempotency_key: "expand-1".into(),
            },
        )
        .expect("invite debugger");
    let expanded = coordinator
        .command(
            &run.id,
            SwarmCommand::Send {
                from_agent_id: "debugger".into(),
                to_agent_id: "tester".into(),
                parts: vec![MessagePart::text("reproduce")],
                priority: false,
                idempotency_key: "expand-2".into(),
            },
        )
        .expect("invite tester");
    assert_eq!(
        expanded.participant_agent_ids,
        vec!["coder", "debugger", "tester"]
    );

    assert!(
        coordinator
            .command(
                &run.id,
                SwarmCommand::Complete {
                    agent_id: "debugger".into(),
                    summary: "done".into(),
                }
            )
            .expect_err("non-root completion")
            .to_string()
            .contains("root agent")
    );
    assert!(
        coordinator
            .command(
                &run.id,
                SwarmCommand::Complete {
                    agent_id: "coder".into(),
                    summary: "Crash fixed and reproduced test passes".into(),
                }
            )
            .expect_err("active work blocks completion")
            .to_string()
            .contains("still active")
    );
    coordinator
        .command(
            &run.id,
            SwarmCommand::Cancel {
                reason: "test cleanup".into(),
            },
        )
        .expect("cancel active run");
    let completion_run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Summarize completed local work".into(),
            max_messages: 2,
            max_turns: 2,
        })
        .expect("start completion run");
    let completed = coordinator
        .command(
            &completion_run.id,
            SwarmCommand::Complete {
                agent_id: "coder".into(),
                summary: "Crash fixed and reproduced test passes".into(),
            },
        )
        .expect("complete");
    assert_eq!(completed.run.status, SwarmStatus::Completed);
    assert!(
        coordinator
            .command(
                &run.id,
                SwarmCommand::Send {
                    from_agent_id: "coder".into(),
                    to_agent_id: "debugger".into(),
                    parts: vec![MessagePart::text("too late")],
                    priority: false,
                    idempotency_key: "late".into(),
                }
            )
            .expect_err("terminal run")
            .to_string()
            .contains("not running")
    );
}

#[test]
fn replies_and_agent_turns_are_accounted_against_the_same_budget() {
    let (_directory, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Account for the whole exchange".into(),
            max_messages: 2,
            max_turns: 1,
        })
        .expect("start");
    let sent = coordinator
        .command(
            &run.id,
            SwarmCommand::Send {
                from_agent_id: "coder".into(),
                to_agent_id: "debugger".into(),
                parts: vec![MessagePart::text("inspect")],
                priority: false,
                idempotency_key: "account-1".into(),
            },
        )
        .expect("send");
    coordinator
        .begin_turn(&run.id, "debugger")
        .expect("first turn");
    let original = sent.messages[0].clone();
    coordinator
        .reply(
            &original,
            &original.target_conversation_id,
            "found it".into(),
        )
        .expect("reply");
    let snapshot = coordinator.snapshot(&run.id).expect("snapshot");
    assert_eq!(snapshot.run.messages_used, 2);
    assert_eq!(snapshot.run.turns_used, 1);
    assert!(
        coordinator
            .begin_turn(&run.id, "coder")
            .expect_err("turn budget")
            .to_string()
            .contains("turn budget")
    );
    assert_eq!(
        coordinator.snapshot(&run.id).expect("attention").run.status,
        SwarmStatus::NeedsAttention
    );
}

#[test]
fn budgets_and_cancel_stop_unbounded_work_but_intervention_can_resume() {
    let (_directory, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Bounded work".into(),
            max_messages: 1,
            max_turns: 2,
        })
        .expect("start");
    coordinator
        .command(
            &run.id,
            SwarmCommand::Send {
                from_agent_id: "coder".into(),
                to_agent_id: "debugger".into(),
                parts: vec![MessagePart::text("first")],
                priority: false,
                idempotency_key: "budget-1".into(),
            },
        )
        .expect("first");
    assert!(
        coordinator
            .command(
                &run.id,
                SwarmCommand::Send {
                    from_agent_id: "debugger".into(),
                    to_agent_id: "tester".into(),
                    parts: vec![MessagePart::text("second")],
                    priority: false,
                    idempotency_key: "budget-2".into(),
                }
            )
            .expect_err("budget exhausted")
            .to_string()
            .contains("budget")
    );
    assert_eq!(
        coordinator.snapshot(&run.id).expect("snapshot").run.status,
        SwarmStatus::NeedsAttention
    );

    let resumed = coordinator
        .command(
            &run.id,
            SwarmCommand::Intervene {
                instruction: "Allow one more specialist".into(),
                max_messages: Some(2),
                max_turns: None,
            },
        )
        .expect("resume");
    assert_eq!(resumed.run.status, SwarmStatus::Running);
    coordinator
        .command(
            &run.id,
            SwarmCommand::Send {
                from_agent_id: "debugger".into(),
                to_agent_id: "tester".into(),
                parts: vec![MessagePart::text("second")],
                priority: false,
                idempotency_key: "budget-2".into(),
            },
        )
        .expect("send after intervention");
    let cancelled = coordinator
        .command(
            &run.id,
            SwarmCommand::Cancel {
                reason: "user stopped".into(),
            },
        )
        .expect("cancel");
    assert_eq!(cancelled.run.status, SwarmStatus::Cancelled);
}

#[tokio::test]
async fn agent_native_tool_resolves_contacts_and_sends_inside_the_current_swarm() {
    let (_directory, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Coordinate autonomously".into(),
            max_messages: 4,
            max_turns: 4,
        })
        .expect("start");
    let tool = SendAgentMessageTool::new(SwarmToolContext {
        run_id: run.id.clone(),
        agent_id: "coder".into(),
        coordinator: coordinator.clone(),
    });
    let result = tool
        .execute(serde_json::json!({
            "to": "Debugger", "message": "Inspect the panic", "priority": true
        }))
        .await
        .expect("tool send");
    assert!(result.contains("Debugger"));
    let snapshot = coordinator.snapshot(&run.id).expect("snapshot");
    assert_eq!(snapshot.messages.len(), 1);
    assert_eq!(snapshot.messages[0].to_agent_id, "debugger");
    assert!(snapshot.messages[0].priority);
}
