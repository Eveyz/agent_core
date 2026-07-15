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

impl SubagentHandoff {
    pub fn from_runtime_result(
        runtime_id: impl Into<String>,
        success: bool,
        summary: impl Into<String>,
        tool_summary: impl Into<String>,
        transcript_ref: Option<String>,
        iterations_used: usize,
        tool_count: usize,
    ) -> Self {
        let summary = summary.into();
        let tool_summary = tool_summary.into();
        let has_summary = !summary.trim().is_empty();
        let status = if success {
            HandoffStatus::Succeeded
        } else if has_summary {
            HandoffStatus::Incomplete
        } else {
            HandoffStatus::Failed
        };
        let mut missing = Vec::new();
        let mut unresolved = Vec::new();
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
                sufficient: success && has_summary,
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
            output.push_str(&format!("\n\nMissing context: {}", self.context.missing.join(", ")));
        }
        if !self.evidence.is_empty() {
            output.push_str("\n\nEvidence:\n");
            output.push_str(&self.evidence.join("\n"));
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
        assert!(handoff.context.missing.contains(&"terminal completion".to_string()));
        assert!(handoff.context.detail_available);
        assert!(handoff.render_for_parent().contains("context_sufficient: false"));
    }
}
