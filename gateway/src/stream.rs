//! Map Agverse `Envelope` / `RunEvent` → Cursor-like SSE events.

use agent_core::runtime::event::{Envelope, RunEvent};
use agent_core::types::MessageDelta;
use axum::response::sse::Event;
use serde_json::json;

/// Convert a runtime envelope into zero or more SSE events.
pub fn envelope_to_sse_events(env: &Envelope) -> Vec<Event> {
    let id = format!("{}-{}", env.ts.timestamp_millis(), env.seq);
    let mut out = Vec::new();

    match &env.event {
        RunEvent::StateChanged { to, .. } => {
            out.push(
                Event::default()
                    .event("status")
                    .id(id.clone())
                    .data(
                        json!({
                            "runId": env.run_id,
                            "status": map_status(to),
                        })
                        .to_string(),
                    ),
            );
        }
        RunEvent::RunStarted => {
            out.push(
                Event::default()
                    .event("status")
                    .id(id.clone())
                    .data(
                        json!({
                            "runId": env.run_id,
                            "status": "RUNNING",
                        })
                        .to_string(),
                    ),
            );
        }
        RunEvent::ModelStreaming { delta, .. } | RunEvent::MessageUpdate { delta, .. } => {
            match delta {
                MessageDelta::Text(text) if !text.is_empty() => {
                    out.push(
                        Event::default()
                            .event("assistant")
                            .id(id.clone())
                            .data(json!({ "text": text }).to_string()),
                    );
                }
                MessageDelta::Thinking(text) if !text.is_empty() => {
                    out.push(
                        Event::default()
                            .event("thinking")
                            .id(id.clone())
                            .data(json!({ "text": text }).to_string()),
                    );
                }
                _ => {}
            }
        }
        RunEvent::ToolStarted {
            call_id,
            name,
            args,
            ..
        } => {
            out.push(
                Event::default()
                    .event("tool_call")
                    .id(id.clone())
                    .data(
                        json!({
                            "callId": call_id,
                            "name": name,
                            "status": "running",
                            "args": args,
                        })
                        .to_string(),
                    ),
            );
        }
        RunEvent::ToolEnded {
            call_id,
            name,
            result,
            is_error,
            ..
        } => {
            out.push(
                Event::default()
                    .event("tool_call")
                    .id(id.clone())
                    .data(
                        json!({
                            "callId": call_id,
                            "name": name,
                            "status": if *is_error { "error" } else { "completed" },
                            "result": result,
                        })
                        .to_string(),
                    ),
            );
        }
        RunEvent::RunCompleted { final_text } => {
            out.push(
                Event::default()
                    .event("result")
                    .id(id.clone())
                    .data(
                        json!({
                            "runId": env.run_id,
                            "status": "FINISHED",
                            "text": final_text,
                        })
                        .to_string(),
                    ),
            );
            out.push(Event::default().event("done").data("{}"));
        }
        RunEvent::RunCancelled { reason } => {
            out.push(
                Event::default()
                    .event("result")
                    .id(id.clone())
                    .data(
                        json!({
                            "runId": env.run_id,
                            "status": "CANCELLED",
                            "text": reason,
                        })
                        .to_string(),
                    ),
            );
            out.push(Event::default().event("done").data("{}"));
        }
        RunEvent::RunFailed { error } => {
            out.push(
                Event::default()
                    .event("error")
                    .id(id.clone())
                    .data(
                        json!({
                            "code": "run_failed",
                            "message": error,
                        })
                        .to_string(),
                    ),
            );
            out.push(
                Event::default()
                    .event("result")
                    .data(
                        json!({
                            "runId": env.run_id,
                            "status": "FAILED",
                            "text": error,
                        })
                        .to_string(),
                    ),
            );
            out.push(Event::default().event("done").data("{}"));
        }
        RunEvent::ApprovalRequired {
            prompt_id,
            tool_name,
            explanation,
            ..
        } => {
            out.push(
                Event::default()
                    .event("interaction_update")
                    .id(id.clone())
                    .data(
                        json!({
                            "type": "approval_required",
                            "promptId": prompt_id,
                            "toolName": tool_name,
                            "summary": explanation,
                        })
                        .to_string(),
                    ),
            );
        }
        RunEvent::InputRequested { prompt_id, .. } => {
            out.push(
                Event::default()
                    .event("interaction_update")
                    .id(id.clone())
                    .data(
                        json!({
                            "type": "input_requested",
                            "promptId": prompt_id,
                        })
                        .to_string(),
                    ),
            );
        }
        // Lifecycle noise / turns — skip to keep the Cursor-like stream lean.
        RunEvent::RunCreated { .. }
        | RunEvent::TurnStarted { .. }
        | RunEvent::TurnEnded { .. }
        | RunEvent::ModelCallStarted
        | RunEvent::ModelCallEnded { .. }
        | RunEvent::MessageStart { .. }
        | RunEvent::MessageEnd { .. }
        | RunEvent::ToolPreparing { .. }
        | RunEvent::ToolUpdate { .. }
        | RunEvent::ApprovalResolved { .. }
        | RunEvent::InputResolved { .. }
        | RunEvent::ContextCompacted { .. }
        | RunEvent::Notice { .. }
        | RunEvent::RunPaused
        | RunEvent::RunResumed => {}
        // Everything else: optional rich passthrough.
        _ => {
            if let Ok(raw) = serde_json::to_value(env) {
                out.push(
                    Event::default()
                        .event("interaction_update")
                        .id(id)
                        .data(
                            json!({
                                "type": "envelope",
                                "envelope": raw,
                            })
                            .to_string(),
                        ),
                );
            }
        }
    }

    out
}

pub fn map_status(state: &agent_core::RunState) -> &'static str {
    use agent_core::RunState::*;
    match state {
        Created => "CREATING",
        Running => "RUNNING",
        AwaitingApproval | AwaitingInput | Paused => "RUNNING",
        Completed => "FINISHED",
        Cancelled => "CANCELLED",
        Failed => "FAILED",
    }
}
