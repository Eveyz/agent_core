//! Workflow validation — checks a workflow definition for structural problems
//! before saving or running.
//!
//! Validates: cycle detection (via the planner), orphan nodes (no edges),
//! missing agent assignments on agent nodes, and missing input/output nodes.

use serde::{Deserialize, Serialize};

use super::definition::{NodeType, WorkflowDef};
use super::planner;

/// A single validation issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    /// Node ids involved in this issue (for UI highlighting).
    #[serde(default)]
    pub node_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// Result of validating a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub valid: bool,
    pub issues: Vec<ValidationIssue>,
}

/// Validate a workflow definition.
pub fn validate(wf: &WorkflowDef) -> ValidationResult {
    let mut issues = Vec::new();

    if wf.nodes.is_empty() {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            code: "empty_workflow".to_string(),
            message: "Workflow must contain at least one node".to_string(),
            node_ids: vec![],
        });
    }

    // 1. Cycle detection (delegates to the planner's Kahn algorithm).
    match planner::plan(&wf.nodes, &wf.edges) {
        Ok(plan) => {
            // 2. Orphan nodes: nodes with no incoming or outgoing edges
            //    (except input/output nodes which are allowed to be endpoints).
            for node in &wf.nodes {
                let has_incoming = wf.edges.iter().any(|e| e.target_node_id == node.id);
                let has_outgoing = wf.edges.iter().any(|e| e.source_node_id == node.id);
                if !has_incoming && !has_outgoing && wf.nodes.len() > 1 {
                    issues.push(ValidationIssue {
                        severity: Severity::Warning,
                        code: "orphan_node".to_string(),
                        message: format!(
                            "Node '{}' is not connected to any other node",
                            node.label
                        ),
                        node_ids: vec![node.id.clone()],
                    });
                }
            }

            // 3. Check that the plan has at least one stage.
            if plan.stages.is_empty() && !wf.nodes.is_empty() {
                issues.push(ValidationIssue {
                    severity: Severity::Error,
                    code: "empty_plan".to_string(),
                    message: "Workflow has nodes but the execution plan is empty".to_string(),
                    node_ids: vec![],
                });
            }
        }
        Err(e) => {
            issues.push(ValidationIssue {
                severity: Severity::Error,
                code: "cycle".to_string(),
                message: e.to_string(),
                node_ids: wf.nodes.iter().map(|n| n.id.clone()).collect(),
            });
        }
    }

    // 4. Missing input/output nodes.
    let has_input = wf.nodes.iter().any(|n| n.node_type == NodeType::Input);
    let has_output = wf.nodes.iter().any(|n| n.node_type == NodeType::Output);
    if !wf.nodes.is_empty() {
        if !has_input {
            issues.push(ValidationIssue {
                severity: Severity::Warning,
                code: "no_input_node".to_string(),
                message: "Workflow has no Input node".to_string(),
                node_ids: vec![],
            });
        }
        if !has_output {
            issues.push(ValidationIssue {
                severity: Severity::Warning,
                code: "no_output_node".to_string(),
                message: "Workflow has no Output node".to_string(),
                node_ids: vec![],
            });
        }
    }

    // 5. Agent nodes without an agent_id.
    for node in &wf.nodes {
        if node.node_type == NodeType::Agent && node.agent_id.is_empty() {
            issues.push(ValidationIssue {
                severity: Severity::Error,
                code: "missing_agent".to_string(),
                message: format!("Agent node '{}' has no agent assigned", node.label),
                node_ids: vec![node.id.clone()],
            });
        }
    }

    // 6. Edges referencing non-existent nodes.
    let node_ids: std::collections::HashSet<&str> =
        wf.nodes.iter().map(|n| n.id.as_str()).collect();
    for edge in &wf.edges {
        if !node_ids.contains(edge.source_node_id.as_str()) {
            issues.push(ValidationIssue {
                severity: Severity::Error,
                code: "dangling_edge".to_string(),
                message: format!(
                    "Edge '{}' references non-existent source node '{}'",
                    edge.id, edge.source_node_id
                ),
                node_ids: vec![edge.source_node_id.clone()],
            });
        }
        if !node_ids.contains(edge.target_node_id.as_str()) {
            issues.push(ValidationIssue {
                severity: Severity::Error,
                code: "dangling_edge".to_string(),
                message: format!(
                    "Edge '{}' references non-existent target node '{}'",
                    edge.id, edge.target_node_id
                ),
                node_ids: vec![edge.target_node_id.clone()],
            });
        }
    }

    let valid = !issues.iter().any(|i| i.severity == Severity::Error);
    ValidationResult { valid, issues }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::definition::{EdgeDef, NodeDef, OnNodeFailure, TrustMode};

    fn make_wf(nodes: Vec<NodeDef>, edges: Vec<EdgeDef>) -> WorkflowDef {
        WorkflowDef {
            nodes,
            edges,
            trust_mode: TrustMode::Inherit,
            max_concurrent: 3,
            on_node_failure: OnNodeFailure::Abort,
            ..Default::default()
        }
    }

    fn node(id: &str, ty: NodeType, agent_id: &str) -> NodeDef {
        NodeDef {
            id: id.to_string(),
            workflow_id: "wf".to_string(),
            node_type: ty,
            label: id.to_string(),
            agent_id: agent_id.to_string(),
            config: serde_json::Value::Null,
            position_x: 0.0,
            position_y: 0.0,
            created_at: String::new(),
        }
    }

    fn edge(src: &str, tgt: &str) -> EdgeDef {
        EdgeDef {
            id: format!("{src}-{tgt}"),
            workflow_id: "wf".to_string(),
            source_node_id: src.to_string(),
            target_node_id: tgt.to_string(),
            source_handle: String::new(),
            target_handle: String::new(),
            label: String::new(),
            condition: String::new(),
            data_mapping: serde_json::json!({"pass_through": true}),
            created_at: String::new(),
        }
    }

    #[test]
    fn valid_workflow_passes() {
        let wf = make_wf(
            vec![
                node("in", NodeType::Input, ""),
                node("agent", NodeType::Agent, "agent-1"),
                node("out", NodeType::Output, ""),
            ],
            vec![edge("in", "agent"), edge("agent", "out")],
        );
        let result = validate(&wf);
        assert!(result.valid, "issues: {:?}", result.issues);
    }

    #[test]
    fn empty_workflow_is_invalid() {
        let result = validate(&make_wf(vec![], vec![]));
        assert!(!result.valid);
        assert!(
            result
                .issues
                .iter()
                .any(|issue| issue.code == "empty_workflow")
        );
    }

    #[test]
    fn cycle_detected() {
        let wf = make_wf(
            vec![
                node("a", NodeType::Agent, "a1"),
                node("b", NodeType::Agent, "b1"),
            ],
            vec![edge("a", "b"), edge("b", "a")],
        );
        let result = validate(&wf);
        assert!(!result.valid);
        assert!(result.issues.iter().any(|i| i.code == "cycle"));
    }

    #[test]
    fn missing_agent_detected() {
        let wf = make_wf(vec![node("a", NodeType::Agent, "")], vec![]);
        let result = validate(&wf);
        assert!(!result.valid);
        assert!(result.issues.iter().any(|i| i.code == "missing_agent"));
    }

    #[test]
    fn orphan_node_warning() {
        let wf = make_wf(
            vec![
                node("in", NodeType::Input, ""),
                node("out", NodeType::Output, ""),
                node("orphan", NodeType::Agent, "x"),
            ],
            vec![edge("in", "out")],
        );
        let result = validate(&wf);
        // orphan is a warning, not an error — valid should still be true
        assert!(result.valid);
        assert!(result.issues.iter().any(|i| i.code == "orphan_node"));
    }

    #[test]
    fn dangling_edge_detected() {
        let wf = make_wf(
            vec![node("in", NodeType::Input, "")],
            vec![edge("in", "nonexistent")],
        );
        let result = validate(&wf);
        assert!(!result.valid);
        assert!(result.issues.iter().any(|i| i.code == "dangling_edge"));
    }
}
