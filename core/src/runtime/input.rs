//! Per-Run human-input (clarification) channel.
//!
//! Mirrors [`crate::runtime::approval::ApprovalResolver`]: when the agent
//! calls `ask_user`, the orchestrator inserts a oneshot sender here, emits
//! `InputRequested`, and blocks until the frontend resolves via
//! [`InputResolver::resolve`] (or the Run is cancelled).

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// A single multiple-choice option shown to the user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClarificationOption {
    pub id: String,
    pub label: String,
}

/// One clarification question (single- or multi-select).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClarificationQuestion {
    pub id: String,
    pub prompt: String,
    /// `false` = single-select, `true` = multi-select.
    #[serde(default)]
    pub allow_multiple: bool,
    pub options: Vec<ClarificationOption>,
}

/// Payload emitted with `InputRequested` / stored while waiting.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClarificationRequest {
    pub prompt_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub questions: Vec<ClarificationQuestion>,
}

/// User answers: question_id → selected option ids.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ClarificationAnswers {
    /// Map of question id → selected option id(s).
    pub answers: HashMap<String, Vec<String>>,
}

/// Type alias for the pending input map: prompt_id → oneshot sender.
pub type PendingInputMap = HashMap<String, tokio::sync::oneshot::Sender<ClarificationAnswers>>;

/// Shared map of pending clarification requests, scoped to a single Run.
#[derive(Clone)]
pub struct InputResolver {
    inner: Arc<Mutex<PendingInputMap>>,
}

impl InputResolver {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn insert(
        &self,
        prompt_id: String,
        tx: tokio::sync::oneshot::Sender<ClarificationAnswers>,
    ) {
        self.inner.lock().insert(prompt_id, tx);
    }

    pub fn remove(&self, prompt_id: &str) {
        self.inner.lock().remove(prompt_id);
    }

    /// Resolve a pending input request. Returns `true` if found.
    pub fn resolve(&self, prompt_id: &str, answers: ClarificationAnswers) -> bool {
        let mut map = self.inner.lock();
        if let Some(tx) = map.remove(prompt_id) {
            let _ = tx.send(answers);
            return true;
        }
        false
    }

    pub fn clear(&self) {
        self.inner.lock().clear();
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for InputResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse and validate `ask_user` tool arguments into a request.
pub fn parse_ask_user_args(args: &serde_json::Value) -> Result<ClarificationRequest, String> {
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let questions_val = args
        .get("questions")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "ask_user requires a non-empty 'questions' array".to_string())?;

    if questions_val.is_empty() {
        return Err("ask_user requires at least one question".to_string());
    }
    if questions_val.len() > 8 {
        return Err("ask_user supports at most 8 questions".to_string());
    }

    let mut questions = Vec::with_capacity(questions_val.len());
    let mut seen_qids = std::collections::HashSet::new();

    for (qi, qv) in questions_val.iter().enumerate() {
        let id = qv
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("q{}", qi + 1));

        if !seen_qids.insert(id.clone()) {
            return Err(format!("duplicate question id '{id}'"));
        }

        let prompt = qv
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("question '{id}' is missing a non-empty 'prompt'"))?;

        let allow_multiple = qv
            .get("allow_multiple")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let options_val = qv
            .get("options")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("question '{id}' requires an 'options' array"))?;

        if options_val.len() < 2 {
            return Err(format!(
                "question '{id}' needs at least 2 options (got {})",
                options_val.len()
            ));
        }
        if options_val.len() > 12 {
            return Err(format!("question '{id}' supports at most 12 options"));
        }

        let mut options = Vec::with_capacity(options_val.len());
        let mut seen_oids = std::collections::HashSet::new();
        for (oi, ov) in options_val.iter().enumerate() {
            let oid = ov
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("o{}", oi + 1));
            if !seen_oids.insert(oid.clone()) {
                return Err(format!("question '{id}' has duplicate option id '{oid}'"));
            }
            let label = ov
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    format!("option '{oid}' in question '{id}' needs a non-empty label")
                })?;
            options.push(ClarificationOption { id: oid, label });
        }

        questions.push(ClarificationQuestion {
            id,
            prompt,
            allow_multiple,
            options,
        });
    }

    Ok(ClarificationRequest {
        prompt_id: uuid::Uuid::new_v4().to_string(),
        title,
        questions,
    })
}

/// Validate that answers reference real question/option ids and respect
/// single- vs multi-select. Returns a cleaned [`ClarificationAnswers`].
pub fn validate_answers(
    request: &ClarificationRequest,
    raw: &ClarificationAnswers,
) -> Result<ClarificationAnswers, String> {
    let mut cleaned = ClarificationAnswers::default();

    for q in &request.questions {
        let selected = raw.answers.get(&q.id).cloned().unwrap_or_default();
        if selected.is_empty() {
            return Err(format!("question '{}' has no selection", q.id));
        }
        if !q.allow_multiple && selected.len() > 1 {
            return Err(format!(
                "question '{}' is single-select but got {} options",
                q.id,
                selected.len()
            ));
        }
        let valid_ids: std::collections::HashSet<&str> =
            q.options.iter().map(|o| o.id.as_str()).collect();
        for oid in &selected {
            if !valid_ids.contains(oid.as_str()) {
                return Err(format!("question '{}' has unknown option id '{oid}'", q.id));
            }
        }
        cleaned.answers.insert(q.id.clone(), selected);
    }

    Ok(cleaned)
}

/// Format answers as a tool-result string for the model.
pub fn format_answers_for_model(
    request: &ClarificationRequest,
    answers: &ClarificationAnswers,
) -> String {
    let mut readable = Vec::new();
    for q in &request.questions {
        let selected = answers.answers.get(&q.id).cloned().unwrap_or_default();
        let labels: Vec<String> = selected
            .iter()
            .filter_map(|oid| {
                q.options
                    .iter()
                    .find(|o| &o.id == oid)
                    .map(|o| format!("{} ({})", o.label, o.id))
            })
            .collect();
        readable.push(format!(
            "- [{}] {}: {}",
            q.id,
            q.prompt,
            if labels.is_empty() {
                "(none)".to_string()
            } else {
                labels.join(", ")
            }
        ));
    }

    let payload = serde_json::json!({
        "title": request.title,
        "answers": answers.answers,
        "summary": readable.join("\n"),
    });
    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_valid_args() {
        let args = json!({
            "title": "Clarify goal",
            "questions": [{
                "id": "scope",
                "prompt": "What scope?",
                "allow_multiple": false,
                "options": [
                    {"id": "mvp", "label": "MVP only"},
                    {"id": "full", "label": "Full feature"}
                ]
            }]
        });
        let req = parse_ask_user_args(&args).unwrap();
        assert_eq!(req.title.as_deref(), Some("Clarify goal"));
        assert_eq!(req.questions.len(), 1);
        assert_eq!(req.questions[0].options.len(), 2);
    }

    #[test]
    fn reject_single_option() {
        let args = json!({
            "questions": [{
                "prompt": "Only one?",
                "options": [{"id": "a", "label": "A"}]
            }]
        });
        assert!(parse_ask_user_args(&args).is_err());
    }

    #[test]
    fn validate_single_select() {
        let req = parse_ask_user_args(&json!({
            "questions": [{
                "id": "q1",
                "prompt": "Pick one",
                "options": [
                    {"id": "a", "label": "A"},
                    {"id": "b", "label": "B"}
                ]
            }]
        }))
        .unwrap();

        let bad = ClarificationAnswers {
            answers: HashMap::from([("q1".into(), vec!["a".into(), "b".into()])]),
        };
        assert!(validate_answers(&req, &bad).is_err());

        let good = ClarificationAnswers {
            answers: HashMap::from([("q1".into(), vec!["b".into()])]),
        };
        assert!(validate_answers(&req, &good).is_ok());
    }

    #[tokio::test]
    async fn resolver_roundtrip() {
        let resolver = InputResolver::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        resolver.insert("p1".into(), tx);
        assert!(resolver.resolve(
            "p1",
            ClarificationAnswers {
                answers: HashMap::from([("q".into(), vec!["a".into()])]),
            },
        ));
        let got = rx.await.unwrap();
        assert_eq!(got.answers.get("q").unwrap(), &vec!["a".to_string()]);
    }
}
