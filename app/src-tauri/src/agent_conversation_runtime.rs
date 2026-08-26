use agent_core::permission::ApprovalChoice;
use agent_core::runtime::ApprovalResolver;
use agent_core::types::AgentEvent;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::Emitter;

#[derive(Clone)]
struct RegisteredTurn {
    registration_id: uuid::Uuid,
    conversation_id: String,
    agent_id: String,
    resolver: ApprovalResolver,
    pending: HashMap<String, AgentConversationApprovalUpdate>,
    pending_input: Option<String>,
    enqueue_sequence: u64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct AgentConversationPendingMessage {
    pub(crate) turn_id: String,
    pub(crate) content: String,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub(crate) enum AgentConversationApprovalUpdate {
    Required {
        conversation_id: String,
        agent_id: String,
        turn_id: String,
        prompt_id: String,
        tool_name: String,
        tool_input: serde_json::Value,
        danger_level: String,
        explanation: String,
    },
    Resolved {
        conversation_id: String,
        agent_id: String,
        turn_id: String,
        prompt_id: String,
        choice: String,
    },
}

#[derive(Clone, Default)]
pub(crate) struct AgentConversationRuntime {
    turns: Arc<Mutex<HashMap<String, RegisteredTurn>>>,
    next_enqueue_sequence: Arc<AtomicU64>,
}

pub(crate) struct AgentConversationTurn {
    runtime: AgentConversationRuntime,
    turn_id: String,
    registration_id: uuid::Uuid,
    resolver: ApprovalResolver,
}

pub(crate) struct AgentConversationForwardingTurn {
    _turn: AgentConversationTurn,
    resolver: ApprovalResolver,
    event_tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>,
}

impl AgentConversationRuntime {
    pub(crate) fn begin_turn(
        &self,
        turn_id: String,
        conversation_id: String,
        agent_id: String,
        pending_input: Option<String>,
    ) -> AgentConversationTurn {
        let registration_id = uuid::Uuid::new_v4();
        let resolver = ApprovalResolver::new();
        let enqueue_sequence = self.next_enqueue_sequence.fetch_add(1, Ordering::Relaxed);
        if let Some(previous) = self.turns.lock().insert(
            turn_id.clone(),
            RegisteredTurn {
                registration_id,
                conversation_id,
                agent_id,
                resolver: resolver.clone(),
                pending: HashMap::new(),
                pending_input,
                enqueue_sequence,
            },
        ) {
            previous.resolver.clear();
        }
        AgentConversationTurn {
            runtime: self.clone(),
            turn_id,
            registration_id,
            resolver,
        }
    }

    pub(crate) fn begin_forwarding_turn(
        &self,
        app_handle: tauri::AppHandle,
        turn_id: String,
        conversation_id: String,
        agent_id: String,
        pending_input: Option<String>,
    ) -> AgentConversationForwardingTurn {
        let turn = self.begin_turn(
            turn_id.clone(),
            conversation_id.clone(),
            agent_id.clone(),
            pending_input,
        );
        let resolver = turn.resolver();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let _ = app_handle.emit(
                    "agent-conversation-event",
                    serde_json::json!({
                        "conversation_id": conversation_id,
                        "agent_id": agent_id,
                        "turn_id": turn_id,
                        "event": &event,
                    }),
                );
                if let Some(update) = runtime.observe_event(&turn_id, event) {
                    let _ = app_handle.emit("agent-conversation-approval", update);
                }
            }
        });
        AgentConversationForwardingTurn {
            _turn: turn,
            resolver,
            event_tx,
        }
    }

    pub(crate) fn resolve_approval(
        &self,
        turn_id: &str,
        prompt_id: &str,
        choice: ApprovalChoice,
    ) -> bool {
        self.turns
            .lock()
            .get(turn_id)
            .map(|turn| turn.resolver.resolve(prompt_id, choice))
            .unwrap_or(false)
    }

    pub(crate) fn observe_event(
        &self,
        turn_id: &str,
        event: AgentEvent,
    ) -> Option<AgentConversationApprovalUpdate> {
        let mut turns = self.turns.lock();
        let turn = turns.get_mut(turn_id)?;
        match event {
            AgentEvent::ApprovalRequired {
                prompt_id,
                tool_name,
                tool_input,
                danger_level,
                explanation,
            }
            | AgentEvent::SubagentApprovalRequired {
                prompt_id,
                tool_name,
                tool_input,
                danger_level,
                explanation,
                ..
            } => {
                let approval = AgentConversationApprovalUpdate::Required {
                    conversation_id: turn.conversation_id.clone(),
                    agent_id: turn.agent_id.clone(),
                    turn_id: turn_id.to_string(),
                    prompt_id: prompt_id.clone(),
                    tool_name,
                    tool_input,
                    danger_level,
                    explanation,
                };
                turn.pending.insert(prompt_id, approval.clone());
                Some(approval)
            }
            AgentEvent::ApprovalResolved { prompt_id, choice } => {
                turn.pending.remove(&prompt_id);
                Some(AgentConversationApprovalUpdate::Resolved {
                    conversation_id: turn.conversation_id.clone(),
                    agent_id: turn.agent_id.clone(),
                    turn_id: turn_id.to_string(),
                    prompt_id,
                    choice,
                })
            }
            _ => None,
        }
    }

    pub(crate) fn pending_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Vec<AgentConversationApprovalUpdate> {
        self.turns
            .lock()
            .values()
            .filter(|turn| turn.conversation_id == conversation_id)
            .flat_map(|turn| turn.pending.values().cloned())
            .collect()
    }

    pub(crate) fn pending_messages_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Vec<AgentConversationPendingMessage> {
        let mut pending = self
            .turns
            .lock()
            .iter()
            .filter_map(|(turn_id, turn)| {
                (turn.conversation_id == conversation_id)
                    .then(|| turn.pending_input.as_ref())
                    .flatten()
                    .map(|content| {
                        (
                            turn.enqueue_sequence,
                            AgentConversationPendingMessage {
                                turn_id: turn_id.clone(),
                                content: content.clone(),
                            },
                        )
                    })
            })
            .collect::<Vec<_>>();
        pending.sort_by_key(|(sequence, _)| *sequence);
        pending.into_iter().map(|(_, message)| message).collect()
    }
}

impl AgentConversationTurn {
    pub(crate) fn resolver(&self) -> ApprovalResolver {
        self.resolver.clone()
    }
}

impl AgentConversationForwardingTurn {
    pub(crate) fn resolver(&self) -> ApprovalResolver {
        self.resolver.clone()
    }

    pub(crate) fn event_sender(&self) -> tokio::sync::mpsc::UnboundedSender<AgentEvent> {
        self.event_tx.clone()
    }
}

impl Drop for AgentConversationTurn {
    fn drop(&mut self) {
        let removed = {
            let mut turns = self.runtime.turns.lock();
            let is_current = turns
                .get(&self.turn_id)
                .map(|turn| turn.registration_id == self.registration_id)
                .unwrap_or(false);
            if is_current {
                turns.remove(&self.turn_id)
            } else {
                None
            }
        };
        if let Some(turn) = removed {
            turn.resolver.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn approvals_are_scoped_to_the_requesting_conversation_turn() {
        let runtime = AgentConversationRuntime::default();
        let coder = runtime.begin_turn(
            "coder-turn".to_string(),
            "coder-conversation".to_string(),
            "coder".to_string(),
            None,
        );
        let debugger = runtime.begin_turn(
            "debugger-turn".to_string(),
            "debugger-conversation".to_string(),
            "debugger".to_string(),
            None,
        );
        let (coder_tx, coder_rx) = tokio::sync::oneshot::channel();
        let (debugger_tx, mut debugger_rx) = tokio::sync::oneshot::channel();
        coder.resolver().insert("approval".to_string(), coder_tx);
        debugger
            .resolver()
            .insert("approval".to_string(), debugger_tx);

        assert!(runtime.resolve_approval("coder-turn", "approval", ApprovalChoice::AllowOnce,));
        assert!(matches!(coder_rx.await.unwrap(), ApprovalChoice::AllowOnce));
        assert!(debugger_rx.try_recv().is_err());
    }

    #[test]
    fn finishing_a_turn_removes_its_approval_route() {
        let runtime = AgentConversationRuntime::default();
        let turn = runtime.begin_turn(
            "coder-turn".to_string(),
            "coder-conversation".to_string(),
            "coder".to_string(),
            None,
        );
        drop(turn);

        assert!(!runtime.resolve_approval("coder-turn", "approval", ApprovalChoice::Deny,));
    }

    #[test]
    fn pending_approval_is_observable_from_the_requesting_contact() {
        let runtime = AgentConversationRuntime::default();
        let _turn = runtime.begin_turn(
            "debugger-turn".to_string(),
            "debugger-conversation".to_string(),
            "debugger".to_string(),
            None,
        );
        runtime.observe_event(
            "debugger-turn",
            AgentEvent::ApprovalRequired {
                prompt_id: "approval".to_string(),
                tool_name: "shell".to_string(),
                tool_input: serde_json::json!({"cmd": "pytest"}),
                danger_level: "medium".to_string(),
                explanation: "Run tests".to_string(),
            },
        );

        let pending = runtime.pending_for_conversation("debugger-conversation");
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            &pending[0],
            AgentConversationApprovalUpdate::Required { agent_id, tool_name, .. }
                if agent_id == "debugger" && tool_name == "shell"
        ));
        assert!(runtime
            .pending_for_conversation("coder-conversation")
            .is_empty());
    }

    #[test]
    fn pending_user_input_survives_contact_switching_until_the_turn_finishes() {
        let runtime = AgentConversationRuntime::default();
        let turn = runtime.begin_turn(
            "coder-turn".to_string(),
            "coder-conversation".to_string(),
            "coder".to_string(),
            Some("build the calculator".to_string()),
        );

        assert_eq!(
            runtime
                .pending_messages_for_conversation("coder-conversation")
                .first()
                .map(|message| message.content.as_str()),
            Some("build the calculator")
        );
        assert!(runtime
            .pending_messages_for_conversation("debugger-conversation")
            .is_empty());

        drop(turn);
        assert!(runtime
            .pending_messages_for_conversation("coder-conversation")
            .is_empty());
    }

    #[test]
    fn queued_user_inputs_keep_acceptance_order() {
        let runtime = AgentConversationRuntime::default();
        let _first = runtime.begin_turn(
            "later-sorting-id".to_string(),
            "coder-conversation".to_string(),
            "coder".to_string(),
            Some("first".to_string()),
        );
        let _second = runtime.begin_turn(
            "earlier-sorting-id".to_string(),
            "coder-conversation".to_string(),
            "coder".to_string(),
            Some("second".to_string()),
        );

        assert_eq!(
            runtime
                .pending_messages_for_conversation("coder-conversation")
                .into_iter()
                .map(|message| message.content)
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
    }
}
