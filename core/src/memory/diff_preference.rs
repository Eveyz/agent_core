use crate::client::OpenAIClient;
use crate::runtime::event::{Envelope, RunEvent};
use crate::reflector::diff_observer::UserEditDiffEvent;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct DiffPreferenceEngine;

impl DiffPreferenceEngine {
    pub fn spawn_analysis(
        client: OpenAIClient,
        diffs: Vec<UserEditDiffEvent>,
        event_tx: mpsc::UnboundedSender<Envelope>,
        seq: Arc<AtomicU64>,
        run_id: String,
    ) {
        tokio::spawn(async move {
            for diff in diffs {
                match Self::analyze_diff(&client, &diff).await {
                    Ok(Some(preference)) => {
                        let _ = event_tx.send(Envelope {
                            seq: seq.fetch_add(1, Ordering::Relaxed),
                            event_id: uuid::Uuid::new_v4().to_string(),
                            run_id: run_id.clone(),
                            session_id: None,
                            turn_id: None,
                            parent_call_id: None,
                            ts: chrono::Utc::now(),
                            event: RunEvent::ApprovalRequired {
                                subagent_id: None,
                                prompt_id: uuid::Uuid::new_v4().to_string(),
                                tool_name: "diff_observer".into(),
                                tool_input: serde_json::json!({
                                    "file": diff.file_path,
                                    "diff": diff.diff,
                                    "preference": &preference
                                }),
                                danger_level: "low".into(),
                                explanation: format!("Diff Observer extracted a new preference: {}", preference),
                            }
                        });
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!("Diff preference analysis failed: {}", e);
                    }
                }
            }
        });
    }

    async fn analyze_diff(client: &OpenAIClient, diff: &UserEditDiffEvent) -> anyhow::Result<Option<String>> {
        let prompt = format!(
            r#"You are a continuous learning engine. 
The Agent edited a file, but the User immediately modified the file afterwards.
This is the unified diff of the User's manual correction:

File: {}
Diff:
{}

Extract a concise, durable programming preference or rule from this correction.
If this looks like a typo fix or a one-off bug fix, return exactly {{"confidence": 0, "rule": ""}}.
If this reflects a real stylistic preference, convention, or architectural rule, return a JSON object with a confidence score (0.0 to 1.0) and the extracted rule.
Format: {{"confidence": 0.9, "rule": "Use early returns instead of nested if statements."}}"#,
            diff.file_path, diff.diff
        );

        let messages = vec![crate::types::Message::system(&prompt)];
        let (response, _) = client.chat_completion(&messages, &[]).await?;

        // Handle markdown json blocks occasionally returned by the model
        let clean_response = response.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();

        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(clean_response) {
            if let Some(conf) = parsed["confidence"].as_f64() {
                if conf > 0.7 {
                    if let Some(rule) = parsed["rule"].as_str() {
                        if !rule.is_empty() {
                            return Ok(Some(rule.to_string()));
                        }
                    }
                }
            }
        }
        
        Ok(None)
    }
}
