use serde::{Deserialize, Serialize};

pub const SUBAGENT_HANDOFF_SCHEMA: &str = "subagent-handoff/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStatus {
    Succeeded,
    Incomplete,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSufficiency {
    pub sufficient: bool,
    pub missing: Vec<String>,
    pub detail_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentHandoff {
    pub schema: &'static str,
    pub runtime_id: String,
    pub status: HandoffStatus,
    pub summary: String,
    pub evidence: Vec<String>,
    pub unresolved: Vec<String>,
    pub context: ContextSufficiency,
    pub transcript_ref: Option<String>,
    pub iterations_used: usize,
    pub tool_count: usize,
}

#[derive(Debug, Deserialize)]
struct ContextDeclaration {
    sufficient: bool,
    #[serde(default)]
    missing: Vec<String>,
    #[serde(default)]
    unresolved: Vec<String>,
}

fn extract_context_declaration(summary: &str) -> (String, Option<ContextDeclaration>) {
    const OPEN: &str = "<context_status>";
    const CLOSE: &str = "</context_status>";
    let trimmed = summary.trim_end();
    let Some(body) = trimmed.strip_suffix(CLOSE) else {
        return (summary.to_string(), None);
    };
    let Some(start) = body.rfind(OPEN) else {
        return (summary.to_string(), None);
    };
    let declaration = serde_json::from_str(&body[start + OPEN.len()..]).ok();
    let visible = body[..start].trim_end().to_string();
    (visible, declaration)
}

impl SubagentHandoff {
    pub fn from_error(runtime_id: impl Into<String>, error: impl Into<String>) -> Self {
        let summary = error.into();
        let lowered = summary.to_ascii_lowercase();
        let status = if lowered.contains("cancel") || lowered.contains("abort") {
            HandoffStatus::Cancelled
        } else {
            HandoffStatus::Failed
        };
        Self {
            schema: SUBAGENT_HANDOFF_SCHEMA,
            runtime_id: runtime_id.into(),
            status,
            summary,
            evidence: Vec::new(),
            unresolved: vec!["Subagent did not produce a terminal answer".to_string()],
            context: ContextSufficiency {
                sufficient: false,
                missing: vec!["terminal answer".to_string()],
                detail_available: false,
            },
            transcript_ref: None,
            iterations_used: 0,
            tool_count: 0,
        }
    }

    pub fn from_error_with_transcript(
        runtime_id: impl Into<String>,
        error: impl Into<String>,
        transcript_ref: Option<String>,
    ) -> Self {
        let mut handoff = Self::from_error(runtime_id, error);
        handoff.context.detail_available = transcript_ref.is_some();
        handoff.transcript_ref = transcript_ref;
        handoff
    }

    pub fn from_runtime_result(
        runtime_id: impl Into<String>,
        success: bool,
        summary: impl Into<String>,
        tool_summary: impl Into<String>,
        transcript_ref: Option<String>,
        iterations_used: usize,
        tool_count: usize,
    ) -> Self {
        let (summary, declaration) = extract_context_declaration(&summary.into());
        let tool_summary = tool_summary.into();
        let has_summary = !summary.trim().is_empty();
        let declared_sufficient = declaration.as_ref().is_some_and(|value| {
            value.sufficient && value.missing.is_empty() && value.unresolved.is_empty()
        });
        let status = if success && declared_sufficient {
            HandoffStatus::Succeeded
        } else if has_summary {
            HandoffStatus::Incomplete
        } else {
            HandoffStatus::Failed
        };
        let mut missing = declaration
            .as_ref()
            .map(|value| value.missing.clone())
            .unwrap_or_else(|| vec!["subagent did not declare context sufficiency".to_string()]);
        let mut unresolved = declaration
            .as_ref()
            .map(|value| value.unresolved.clone())
            .unwrap_or_default();
        if !success {
            missing.push("terminal completion".to_string());
            unresolved.push("Subagent stopped before a successful terminal answer".to_string());
        }
        if !has_summary {
            missing.push("answer summary".to_string());
        }
        let detail_available = transcript_ref.is_some();
        Self {
            schema: SUBAGENT_HANDOFF_SCHEMA,
            runtime_id: runtime_id.into(),
            status,
            summary,
            evidence: (!tool_summary.trim().is_empty())
                .then_some(tool_summary)
                .into_iter()
                .collect(),
            unresolved,
            context: ContextSufficiency {
                sufficient: success && has_summary && declared_sufficient,
                missing,
                detail_available,
            },
            transcript_ref,
            iterations_used,
            tool_count,
        }
    }

    pub fn render_for_parent(&self) -> String {
        let status = serde_json::to_string(&self.status)
            .unwrap_or_else(|_| "\"failed\"".to_string())
            .trim_matches('"')
            .to_string();
        let mut output = format!(
            "[{}]\nruntime_id: {}\nstatus: {}\ncontext_sufficient: {}\niterations: {}\ntools: {}\n\n{}",
            self.schema,
            self.runtime_id,
            status,
            self.context.sufficient,
            self.iterations_used,
            self.tool_count,
            self.summary
        );
        if !self.context.missing.is_empty() {
            output.push_str(&format!(
                "\n\nMissing context: {}",
                self.context.missing.join(", ")
            ));
        }
        if !self.evidence.is_empty() {
            output.push_str("\n\nEvidence:\n");
            output.push_str(&self.evidence.join("\n"));
        }
        if !self.unresolved.is_empty() {
            output.push_str("\n\nUnresolved:\n");
            output.push_str(&self.unresolved.join("\n"));
        }
        if let Some(reference) = &self.transcript_ref {
            output.push_str(&format!("\n\nTranscript: {reference}"));
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_handoff_explicitly_reports_missing_context_and_detail_reference() {
        let handoff = SubagentHandoff::from_runtime_result(
            "runtime-1",
            false,
            "Found the likely cause",
            "read_file: 2 calls",
            Some("/tmp/runtime-1.transcript.json".into()),
            50,
            2,
        );
        assert_eq!(handoff.status, HandoffStatus::Incomplete);
        assert!(!handoff.context.sufficient);
        assert!(handoff
            .context
            .missing
            .contains(&"terminal completion".to_string()));
        assert!(handoff.context.detail_available);
        assert!(handoff
            .render_for_parent()
            .contains("context_sufficient: false"));
    }

    #[test]
    fn successful_handoff_requires_an_explicit_context_declaration() {
        let undeclared =
            SubagentHandoff::from_runtime_result("runtime-1", true, "Done", "", None, 1, 0);
        assert!(!undeclared.context.sufficient);
        assert_eq!(undeclared.status, HandoffStatus::Incomplete);

        let declared = SubagentHandoff::from_runtime_result(
            "runtime-2",
            true,
            "Done\n<context_status>{\"sufficient\":true,\"missing\":[],\"unresolved\":[]}</context_status>",
            "",
            None,
            1,
            0,
        );
        assert!(declared.context.sufficient);
        assert_eq!(declared.summary, "Done");

        let contradictory = SubagentHandoff::from_runtime_result(
            "runtime-3",
            true,
            "Done\n<context_status>{\"sufficient\":true,\"missing\":[\"logs\"],\"unresolved\":[\"cause\"]}</context_status>",
            "",
            None,
            1,
            0,
        );
        assert!(!contradictory.context.sufficient);
        assert_eq!(contradictory.status, HandoffStatus::Incomplete);
        assert!(contradictory.render_for_parent().contains("Unresolved:"));
    }

    #[test]
    fn failed_handoff_exposes_canonical_transcript_reference() {
        let handoff = SubagentHandoff::from_error_with_transcript(
            "runtime-4",
            "provider failed",
            Some("/tmp/runtime-4.transcript.json".into()),
        );
        assert_eq!(handoff.runtime_id, "runtime-4");
        assert!(handoff.context.detail_available);
        assert_eq!(handoff.transcript_ref.as_deref(), Some("/tmp/runtime-4.transcript.json"));
    }
}
