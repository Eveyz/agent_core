use agent_core::agent_registry::AgentDef;
use agent_core::memory::storage::Storage;
use agent_core::{
    AgentMessaging, MessagePart, SendAgentMessageTool, StartSwarm, SwarmCommand, SwarmCoordinator,
    SwarmStatus, SwarmToolContext, Tool,
};

fn coordinator_with_agents() -> (
    tempfile::TempDir,
    AgentMessaging,
    agent_core::ActiveAgentRuns,
    SwarmCoordinator,
) {
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
    let active_runs = agent_core::ActiveAgentRuns::new();
    (
        directory,
        messaging.clone(),
        active_runs.clone(),
        SwarmCoordinator::new(storage, messaging, active_runs),
    )
}

#[test]
fn start_send_and_observe_persist_the_swarm_boundary() {
    let (_directory, _messaging, _active_runs, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Diagnose and fix the crash".into(),
            max_messages: 8,
            max_turns: 6,
            max_hops: 8,
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
                hop_count: 1,
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
    let (_directory, _messaging, _active_runs, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Ship a verified fix".into(),
            max_messages: 8,
            max_turns: 6,
            max_hops: 8,
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
                hop_count: 1,
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
                hop_count: 2,
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
                    current_task_id: None,
                    current_turn_id: None,
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
                    current_task_id: None,
                    current_turn_id: None,
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
            max_hops: 2,
        })
        .expect("start completion run");
    let completed = coordinator
        .command(
            &completion_run.id,
            SwarmCommand::Complete {
                agent_id: "coder".into(),
                summary: "Crash fixed and reproduced test passes".into(),
                current_task_id: None,
                current_turn_id: None,
            },
        )
        .expect("complete");
    assert_eq!(completed.run.status, SwarmStatus::Completed);
    coordinator
        .mark_needs_attention(&completion_run.id, "late runner failure")
        .expect("terminal attention is ignored");
    assert_eq!(
        coordinator
            .snapshot(&completion_run.id)
            .expect("terminal snapshot")
            .run
            .status,
        SwarmStatus::Completed
    );
    assert!(
        coordinator
            .command(
                &completion_run.id,
                SwarmCommand::Intervene {
                    instruction: "reopen".into(),
                    max_messages: None,
                    max_turns: None,
                    max_hops: None,
                },
            )
            .expect_err("terminal intervention")
            .to_string()
            .contains("cannot be reopened")
    );
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
                    hop_count: 1,
                }
            )
            .expect_err("terminal run")
            .to_string()
            .contains("not running")
    );
}

#[test]
fn replies_and_agent_turns_are_accounted_against_the_same_budget() {
    let (_directory, _messaging, _active_runs, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Account for the whole exchange".into(),
            max_messages: 2,
            max_turns: 1,
            max_hops: 2,
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
                hop_count: 1,
            },
        )
        .expect("send");
    coordinator
        .begin_turn(
            &run.id,
            "debugger",
            "debugger-turn-1",
            agent_core::AgentRunLane::Peer,
        )
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
    assert_eq!(snapshot.run.hops_used, 2);
    assert!(
        coordinator
            .begin_turn(
                &run.id,
                "coder",
                "coder-turn-1",
                agent_core::AgentRunLane::User,
            )
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
    let (_directory, _messaging, _active_runs, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Bounded work".into(),
            max_messages: 1,
            max_turns: 2,
            max_hops: 3,
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
                hop_count: 1,
            },
        )
        .expect("first");
    let replay = coordinator
        .command(
            &run.id,
            SwarmCommand::Send {
                from_agent_id: "coder".into(),
                to_agent_id: "debugger".into(),
                parts: vec![MessagePart::text("first")],
                priority: false,
                idempotency_key: "budget-1".into(),
                hop_count: 1,
            },
        )
        .expect("idempotent replay at message budget");
    assert_eq!(replay.run.messages_used, 1);
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
                    hop_count: 2,
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
                max_hops: None,
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
                hop_count: 2,
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
    let (_directory, messaging, active_runs, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Coordinate autonomously".into(),
            max_messages: 4,
            max_turns: 4,
            max_hops: 4,
        })
        .expect("start");
    let tool = SendAgentMessageTool::new(SwarmToolContext {
        run_id: run.id.clone(),
        agent_id: "coder".into(),
        next_hop: 1,
        effect_scope_id: "root-turn-1".into(),
        active_task_id: None,
        active_turn_id: None,
        coordinator: coordinator.clone(),
    });
    let result = tool
        .execute(serde_json::json!({
            "to": "Debugger", "message": "Inspect the panic", "priority": true
        }))
        .await
        .expect("tool send");
    assert!(result.contains("Debugger"));
    let first_message_id = serde_json::from_str::<serde_json::Value>(&result)
        .expect("tool result json")["message_id"]
        .as_str()
        .expect("message id")
        .to_string();
    let snapshot = coordinator.snapshot(&run.id).expect("snapshot");
    assert_eq!(snapshot.messages.len(), 1);
    assert_eq!(snapshot.messages[0].to_agent_id, "debugger");
    assert!(snapshot.messages[0].priority);
    tool.execute(serde_json::json!({
        "to": "Debugger", "message": "Inspect the second panic", "priority": false
    }))
    .await
    .expect("second send");
    let replay = tool
        .execute(serde_json::json!({
            "to": "Debugger", "message": "Inspect the panic", "priority": true
        }))
        .await
        .expect("idempotent replay");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&replay).expect("replay json")["message_id"],
        first_message_id
    );
    assert_eq!(
        coordinator
            .snapshot(&run.id)
            .expect("replay snapshot")
            .messages
            .len(),
        2
    );

    let claimed = messaging
        .claim_next("test-worker")
        .expect("claim")
        .expect("queued task");
    let cancel = agent_core::CancellationToken::new();
    let lease = active_runs
        .enter(
            "debugger",
            claimed.task.id.clone(),
            agent_core::AgentRunLane::Peer,
            cancel.clone(),
        )
        .await;
    coordinator
        .command(
            &run.id,
            SwarmCommand::Cancel {
                reason: "stop active peer".into(),
            },
        )
        .expect("cancel active swarm");
    assert!(cancel.is_cancelled());
    lease.finish();
}

#[test]
fn completion_only_allows_the_current_root_delivery() {
    let (_directory, messaging, _active_runs, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Finish from the root relay".into(),
            max_messages: 4,
            max_turns: 4,
            max_hops: 4,
        })
        .expect("start");
    let first = coordinator
        .command(
            &run.id,
            SwarmCommand::Send {
                from_agent_id: "coder".into(),
                to_agent_id: "debugger".into(),
                parts: vec![MessagePart::text("join")],
                priority: false,
                idempotency_key: "complete-join".into(),
                hop_count: 1,
            },
        )
        .expect("join debugger");
    let first_task = messaging
        .delivery(&first.messages[0].id)
        .expect("first delivery")
        .task;
    messaging
        .command(
            &first_task.id,
            agent_core::AgentTaskCommand::Cancel {
                reason: "setup".into(),
            },
        )
        .expect("clear setup task");
    let reply = coordinator
        .command(
            &run.id,
            SwarmCommand::Send {
                from_agent_id: "debugger".into(),
                to_agent_id: "coder".into(),
                parts: vec![MessagePart::text("root relay")],
                priority: false,
                idempotency_key: "complete-root".into(),
                hop_count: 2,
            },
        )
        .expect("queue root relay");
    assert!(
        coordinator
            .command(
                &run.id,
                SwarmCommand::Complete {
                    agent_id: "coder".into(),
                    summary: "done".into(),
                    current_task_id: None,
                    current_turn_id: None,
                },
            )
            .expect_err("queued root work blocks completion")
            .to_string()
            .contains("still active")
    );
    let claimed = messaging
        .claim_next("root-worker")
        .expect("claim root")
        .expect("root task");
    assert_eq!(claimed.message.id, reply.messages.last().expect("reply").id);
    coordinator
        .begin_turn(
            &run.id,
            "coder",
            &claimed.task.id,
            agent_core::AgentRunLane::Peer,
        )
        .expect("begin current root turn");
    let completed = coordinator
        .command(
            &run.id,
            SwarmCommand::Complete {
                agent_id: "coder".into(),
                summary: "done".into(),
                current_task_id: Some(claimed.task.id.clone()),
                current_turn_id: Some(claimed.task.id.clone()),
            },
        )
        .expect("current root task may complete");
    assert_eq!(completed.run.status, SwarmStatus::Completing);
    let final_reply = coordinator
        .reply(
            &claimed.message,
            &claimed.target_conversation.id,
            "final result".into(),
        )
        .expect("completed root request gets final reply");
    coordinator
        .finish_turn(&run.id, &claimed.task.id)
        .expect("finish root turn");
    messaging
        .command(
            &claimed.task.id,
            agent_core::AgentTaskCommand::Complete {
                output_message_id: Some(final_reply.message.id),
            },
        )
        .expect("complete root task");
    coordinator
        .recover_interrupted("restart after task completion")
        .expect("recover finalized swarm");
    assert_eq!(
        coordinator
            .snapshot(&run.id)
            .expect("final state")
            .run
            .status,
        SwarmStatus::Completed
    );
}

#[test]
fn direct_root_completion_waits_for_the_durable_user_turn() {
    let (_directory, _messaging, _active_runs, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Finish a direct root turn safely".into(),
            max_messages: 2,
            max_turns: 2,
            max_hops: 2,
        })
        .expect("start");
    assert!(
        coordinator
            .command(
                &run.id,
                SwarmCommand::Complete {
                    agent_id: "coder".into(),
                    summary: "forged task".into(),
                    current_task_id: Some("missing-task".into()),
                    current_turn_id: None,
                },
            )
            .expect_err("missing task identity is rejected")
            .to_string()
            .contains("work is still active")
    );
    assert!(
        coordinator
            .command(
                &run.id,
                SwarmCommand::Complete {
                    agent_id: "coder".into(),
                    summary: "forged turn".into(),
                    current_task_id: None,
                    current_turn_id: Some("missing-turn".into()),
                },
            )
            .expect_err("missing turn identity is rejected")
            .to_string()
            .contains("turns are still active")
    );
    coordinator
        .begin_turn(
            &run.id,
            "coder",
            "user-turn-complete",
            agent_core::AgentRunLane::User,
        )
        .expect("begin root turn");
    let completing = coordinator
        .command(
            &run.id,
            SwarmCommand::Complete {
                agent_id: "coder".into(),
                summary: "done".into(),
                current_task_id: None,
                current_turn_id: Some("user-turn-complete".into()),
            },
        )
        .expect("request completion");
    assert_eq!(completing.run.status, SwarmStatus::Completing);
    assert_eq!(
        completing.run.completion_turn_id.as_deref(),
        Some("user-turn-complete")
    );
    coordinator
        .finish_turn(&run.id, "user-turn-complete")
        .expect("persisted runner result finalizes completion");
    assert_eq!(
        coordinator.snapshot(&run.id).expect("snapshot").run.status,
        SwarmStatus::Completed
    );

    let interrupted = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Recover an interrupted direct completion".into(),
            max_messages: 2,
            max_turns: 2,
            max_hops: 2,
        })
        .expect("start interrupted run");
    coordinator
        .begin_turn(
            &interrupted.id,
            "coder",
            "user-turn-interrupted",
            agent_core::AgentRunLane::User,
        )
        .expect("begin interrupted turn");
    coordinator
        .command(
            &interrupted.id,
            SwarmCommand::Complete {
                agent_id: "coder".into(),
                summary: "not durable yet".into(),
                current_task_id: None,
                current_turn_id: Some("user-turn-interrupted".into()),
            },
        )
        .expect("request interrupted completion");
    coordinator
        .recover_interrupted("restart before runner result was persisted")
        .expect("recover interrupted completion");
    assert_eq!(
        coordinator
            .snapshot(&interrupted.id)
            .expect("recovered snapshot")
            .run
            .status,
        SwarmStatus::NeedsAttention
    );
}

#[test]
fn hop_budget_stops_agent_chains() {
    let (_directory, _messaging, _active_runs, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Bound delegation depth".into(),
            max_messages: 4,
            max_turns: 4,
            max_hops: 2,
        })
        .expect("start");
    coordinator
        .command(
            &run.id,
            SwarmCommand::Send {
                from_agent_id: "coder".into(),
                to_agent_id: "debugger".into(),
                parts: vec![MessagePart::text("first hop")],
                priority: false,
                idempotency_key: "hop-1".into(),
                hop_count: 1,
            },
        )
        .expect("first hop");
    assert!(
        coordinator
            .command(
                &run.id,
                SwarmCommand::Send {
                    from_agent_id: "debugger".into(),
                    to_agent_id: "tester".into(),
                    parts: vec![MessagePart::text("too deep")],
                    priority: false,
                    idempotency_key: "hop-3".into(),
                    hop_count: 3,
                },
            )
            .expect_err("hop budget")
            .to_string()
            .contains("hop budget")
    );
    assert_eq!(
        coordinator.snapshot(&run.id).expect("snapshot").run.status,
        SwarmStatus::NeedsAttention
    );
}

#[test]
fn idempotency_keys_cannot_replay_across_swarms() {
    let (_directory, _messaging, _active_runs, coordinator) = coordinator_with_agents();
    let start = |goal: &str| {
        coordinator
            .start(StartSwarm {
                project_id: "__adhoc_chat__".into(),
                root_agent_id: "coder".into(),
                goal: goal.into(),
                max_messages: 2,
                max_turns: 2,
                max_hops: 2,
            })
            .expect("start")
    };
    let first = start("first run");
    coordinator
        .command(
            &first.id,
            SwarmCommand::Send {
                from_agent_id: "coder".into(),
                to_agent_id: "debugger".into(),
                parts: vec![MessagePart::text("first")],
                priority: false,
                idempotency_key: "shared-key".into(),
                hop_count: 1,
            },
        )
        .expect("first send");
    let second = start("second run");
    assert!(
        coordinator
            .command(
                &second.id,
                SwarmCommand::Send {
                    from_agent_id: "coder".into(),
                    to_agent_id: "tester".into(),
                    parts: vec![MessagePart::text("second")],
                    priority: false,
                    idempotency_key: "shared-key".into(),
                    hop_count: 1,
                },
            )
            .expect_err("cross-swarm replay")
            .to_string()
            .contains("different agent message")
    );
    let snapshot = coordinator.snapshot(&second.id).expect("second snapshot");
    assert_eq!(snapshot.run.messages_used, 0);
    assert_eq!(snapshot.participant_agent_ids, vec!["coder"]);
}

#[test]
fn restart_recovery_reconciles_swarm_lifecycle() {
    let (_directory, messaging, _active_runs, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Recover interrupted work".into(),
            max_messages: 4,
            max_turns: 4,
            max_hops: 4,
        })
        .expect("start");
    coordinator
        .command(
            &run.id,
            SwarmCommand::Send {
                from_agent_id: "coder".into(),
                to_agent_id: "debugger".into(),
                parts: vec![MessagePart::text("work")],
                priority: false,
                idempotency_key: "recover-1".into(),
                hop_count: 1,
            },
        )
        .expect("send");
    messaging
        .claim_next("dead-worker")
        .expect("claim")
        .expect("working task");
    messaging
        .recover_interrupted("restart")
        .expect("messaging recovery");
    let recovered = coordinator
        .recover_interrupted("restart")
        .expect("swarm recovery");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, SwarmStatus::NeedsAttention);

    let root_run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Recover an interrupted root turn".into(),
            max_messages: 2,
            max_turns: 2,
            max_hops: 2,
        })
        .expect("root run");
    coordinator
        .begin_turn(
            &root_run.id,
            "coder",
            &root_run.id,
            agent_core::AgentRunLane::User,
        )
        .expect("root turn");
    let recovered = coordinator
        .recover_interrupted("restart")
        .expect("root recovery");
    assert!(recovered.iter().any(|run| run.id == root_run.id));
}

#[tokio::test]
async fn cancel_stops_an_active_root_turn() {
    let (_directory, _messaging, active_runs, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Stop root work".into(),
            max_messages: 2,
            max_turns: 2,
            max_hops: 2,
        })
        .expect("start");
    let cancel = agent_core::CancellationToken::new();
    let lease = active_runs
        .enter(
            "coder",
            run.id.clone(),
            agent_core::AgentRunLane::User,
            cancel.clone(),
        )
        .await;
    coordinator
        .begin_turn(&run.id, "coder", &run.id, agent_core::AgentRunLane::User)
        .expect("begin root turn");
    coordinator
        .command(
            &run.id,
            SwarmCommand::Cancel {
                reason: "user stop".into(),
            },
        )
        .expect("cancel");
    assert!(cancel.is_cancelled());
    lease.finish();
}

#[tokio::test]
async fn concurrent_user_turns_block_completion_and_are_all_cancelled() {
    let (_directory, messaging, active_runs, coordinator) = coordinator_with_agents();
    let run = coordinator
        .start(StartSwarm {
            project_id: "__adhoc_chat__".into(),
            root_agent_id: "coder".into(),
            goal: "Coordinate concurrent contacts".into(),
            max_messages: 4,
            max_turns: 4,
            max_hops: 4,
        })
        .expect("start");
    let joined = coordinator
        .command(
            &run.id,
            SwarmCommand::Send {
                from_agent_id: "coder".into(),
                to_agent_id: "debugger".into(),
                parts: vec![MessagePart::text("join")],
                priority: false,
                idempotency_key: "concurrent-join".into(),
                hop_count: 1,
            },
        )
        .expect("join");
    let join_task = messaging
        .delivery(&joined.messages[0].id)
        .expect("join delivery")
        .task;
    messaging
        .command(
            &join_task.id,
            agent_core::AgentTaskCommand::Cancel {
                reason: "setup".into(),
            },
        )
        .expect("clear setup");

    let root_cancel = agent_core::CancellationToken::new();
    let peer_cancel = agent_core::CancellationToken::new();
    let root_lease = active_runs
        .enter(
            "coder",
            "user-turn-root",
            agent_core::AgentRunLane::User,
            root_cancel.clone(),
        )
        .await;
    let peer_lease = active_runs
        .enter(
            "debugger",
            "user-turn-peer",
            agent_core::AgentRunLane::User,
            peer_cancel.clone(),
        )
        .await;
    coordinator
        .begin_turn(
            &run.id,
            "coder",
            "user-turn-root",
            agent_core::AgentRunLane::User,
        )
        .expect("root turn");
    coordinator
        .begin_turn(
            &run.id,
            "debugger",
            "user-turn-peer",
            agent_core::AgentRunLane::User,
        )
        .expect("peer contact turn");
    assert!(
        coordinator
            .command(
                &run.id,
                SwarmCommand::Complete {
                    agent_id: "coder".into(),
                    summary: "too early".into(),
                    current_task_id: None,
                    current_turn_id: Some("user-turn-root".into()),
                },
            )
            .expect_err("other active turn blocks completion")
            .to_string()
            .contains("turns are still active")
    );
    coordinator
        .command(
            &run.id,
            SwarmCommand::Cancel {
                reason: "stop all".into(),
            },
        )
        .expect("cancel");
    assert!(root_cancel.is_cancelled());
    assert!(peer_cancel.is_cancelled());
    root_lease.finish();
    peer_lease.finish();
}
