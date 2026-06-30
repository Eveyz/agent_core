//! Workflow planner — turns a node/edge graph into an ordered execution plan.
//!
//! Uses Kahn's algorithm to produce a topological ordering grouped into
//! "stages": all nodes within a stage are independent and may run in parallel,
//! while stages must execute sequentially (every node in stage N+1 depends only
//! on nodes in stages ≤ N). Cycles are detected and rejected.

use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};

use super::definition::{EdgeDef, NodeDef};

/// A single stage of parallelizable nodes.
#[derive(Debug, Clone)]
pub struct Stage {
    /// Node ids that can execute in parallel within this stage.
    pub nodes: Vec<String>,
}

/// A validated execution plan: a sequence of parallel stages.
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    pub stages: Vec<Stage>,
}

impl ExecutionPlan {
    pub fn node_ids(&self) -> Vec<String> {
        self.stages.iter().flat_map(|s| s.nodes.clone()).collect()
    }
}

/// Build an [`ExecutionPlan`] from nodes and edges.
///
/// Returns an error if the graph contains a cycle.
pub fn plan(nodes: &[NodeDef], edges: &[EdgeDef]) -> Result<ExecutionPlan> {
    let node_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();

    // Build adjacency: for each node, the set of nodes it depends on (predecessors),
    // and the set of nodes that depend on it (successors).
    let mut predecessors: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut successors: HashMap<&str, HashSet<&str>> = HashMap::new();
    for id in &node_ids {
        predecessors.entry(id).or_default();
        successors.entry(id).or_default();
    }
    for edge in edges {
        if !node_ids.contains(edge.source_node_id.as_str())
            || !node_ids.contains(edge.target_node_id.as_str())
        {
            // Edge references a non-existent node — skip it (frontend may have
            // stale edges during editing).
            continue;
        }
        successors
            .get_mut(edge.source_node_id.as_str())
            .unwrap()
            .insert(edge.target_node_id.as_str());
        predecessors
            .get_mut(edge.target_node_id.as_str())
            .unwrap()
            .insert(edge.source_node_id.as_str());
    }

    // Kahn's algorithm, grouped into stages.
    let mut stages: Vec<Stage> = Vec::new();
    let mut remaining: HashSet<&str> = node_ids.clone();
    let mut indegree: HashMap<&str, usize> = predecessors
        .iter()
        .map(|(id, preds)| (*id, preds.len()))
        .collect();

    while !remaining.is_empty() {
        // Nodes with no unmet predecessors form the next stage.
        let stage_nodes: Vec<String> = remaining
            .iter()
            .copied()
            .filter(|id| *indegree.get(id).unwrap_or(&0) == 0)
            .map(|s| s.to_string())
            .collect();

        if stage_nodes.is_empty() {
            let cycle_members: Vec<String> =
                remaining.iter().map(|s| s.to_string()).collect();
            return Err(anyhow!(
                "workflow contains a cycle among nodes: [{}]",
                cycle_members.join(", ")
            ));
        }

        // Remove stage nodes from the graph.
        for node_id in &stage_nodes {
            remaining.remove(node_id.as_str());
            if let Some(succs) = successors.get(node_id.as_str()) {
                for succ in succs {
                    if let Some(count) = indegree.get_mut(succ) {
                        *count = count.saturating_sub(1);
                    }
                }
            }
        }

        stages.push(Stage { nodes: stage_nodes });
    }

    Ok(ExecutionPlan { stages })
}

/// Return the incoming edges for a node (for input resolution).
pub fn incoming_edges<'a>(
    node_id: &str,
    edges: &'a [EdgeDef],
) -> Vec<&'a EdgeDef> {
    edges.iter().filter(|e| e.target_node_id == node_id).collect()
}

/// Return the outgoing edges for a node.
pub fn outgoing_edges<'a>(
    node_id: &str,
    edges: &'a [EdgeDef],
) -> Vec<&'a EdgeDef> {
    edges.iter().filter(|e| e.source_node_id == node_id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::definition::NodeType;

    fn node(id: &str, ty: NodeType) -> NodeDef {
        NodeDef {
            id: id.to_string(),
            workflow_id: "wf".to_string(),
            node_type: ty,
            label: id.to_string(),
            agent_id: String::new(),
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
    fn linear_graph_one_node_per_stage() {
        let nodes = vec![node("a", NodeType::Input), node("b", NodeType::Agent), node("c", NodeType::Output)];
        let edges = vec![edge("a", "b"), edge("b", "c")];
        let plan = plan(&nodes, &edges).unwrap();
        assert_eq!(plan.stages.len(), 3);
        assert_eq!(plan.stages[0].nodes, vec!["a"]);
        assert_eq!(plan.stages[1].nodes, vec!["b"]);
        assert_eq!(plan.stages[2].nodes, vec!["c"]);
    }

    #[test]
    fn fork_join_parallel_stage() {
        // a -> b, a -> c, b -> d, c -> d
        let nodes = vec![
            node("a", NodeType::Input),
            node("b", NodeType::Agent),
            node("c", NodeType::Agent),
            node("d", NodeType::Output),
        ];
        let edges = vec![
            edge("a", "b"),
            edge("a", "c"),
            edge("b", "d"),
            edge("c", "d"),
        ];
        let plan = plan(&nodes, &edges).unwrap();
        assert_eq!(plan.stages.len(), 3);
        assert_eq!(plan.stages[0].nodes, vec!["a"]);
        // b and c are in the same stage (order within stage may vary)
        let stage1: HashSet<&str> = plan.stages[1].nodes.iter().map(|s| s.as_str()).collect();
        assert_eq!(stage1, HashSet::from(["b", "c"]));
        assert_eq!(plan.stages[2].nodes, vec!["d"]);
    }

    #[test]
    fn cycle_detected() {
        let nodes = vec![node("a", NodeType::Agent), node("b", NodeType::Agent)];
        let edges = vec![edge("a", "b"), edge("b", "a")];
        let result = plan(&nodes, &edges);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cycle"));
    }
}
