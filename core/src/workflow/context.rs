//! Workflow execution context — structured state passed between nodes.
//!
//! Unlike string-template interpolation, [`WorkflowContext`] holds JSON values
//! per node output and resolves a node's input by applying the incoming edges'
//! `data_mapping` rules. This avoids template-injection risks and supports
//! field-level merging from multiple upstream nodes.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use super::definition::EdgeDef;

/// Shared, mutable state flowing through a workflow execution.
pub struct WorkflowContext {
    /// Per-node outputs (node_id → JSON value).
    node_outputs: RwLock<HashMap<String, serde_json::Value>>,
    /// Shared scratch state any node can read/write.
    shared: RwLock<serde_json::Value>,
    /// The original workflow input.
    input: serde_json::Value,
}

impl WorkflowContext {
    pub fn new(input: serde_json::Value) -> Self {
        Self {
            node_outputs: RwLock::new(HashMap::new()),
            shared: RwLock::new(serde_json::Value::Object(serde_json::Map::new())),
            input,
        }
    }

    pub fn input(&self) -> &serde_json::Value {
        &self.input
    }

    pub fn set_output(&self, node_id: &str, output: serde_json::Value) {
        self.node_outputs.write().insert(node_id.to_string(), output);
    }

    pub fn get_output(&self, node_id: &str) -> Option<serde_json::Value> {
        self.node_outputs.read().get(node_id).cloned()
    }

    pub fn update_shared(&self, key: &str, value: serde_json::Value) {
        if let Some(obj) = self.shared.write().as_object_mut() {
            obj.insert(key.to_string(), value);
        }
    }

    /// Resolve a node's input from its incoming edges + the workflow input.
    ///
    /// The result is a JSON object with:
    /// - one key per incoming edge (keyed by edge label or source node id),
    /// - `_shared`: the shared state,
    /// - `_workflow_input`: the original input.
    pub fn resolve_input(
        &self,
        node_id: &str,
        incoming: &[&EdgeDef],
    ) -> serde_json::Value {
        let outputs = self.node_outputs.read();
        let mut input = serde_json::Map::new();

        for edge in incoming {
            let upstream = outputs
                .get(&edge.source_node_id)
                .cloned()
                .unwrap_or(serde_json::Value::Null);

            let mapping: DataMapping =
                serde_json::from_value(edge.data_mapping.clone()).unwrap_or_default();

            let key = if !edge.label.is_empty() {
                edge.label.clone()
            } else {
                edge.source_node_id.clone()
            };

            if mapping.pass_through {
                input.insert(key, upstream);
            } else if let Some(source_field) = mapping.source_field {
                if let Some(val) = upstream.get(&source_field) {
                    let target = mapping.target_field.unwrap_or(source_field);
                    input.insert(target, val.clone());
                }
            }
        }

        input.insert("_shared".into(), self.shared.read().clone());
        input.insert("_workflow_input".into(), self.input.clone());

        serde_json::Value::Object(input)
    }
}

/// Describes how an edge maps upstream output into the downstream input.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DataMapping {
    /// If true, pass the entire upstream output through (keyed by edge label).
    #[serde(default)]
    pass_through: bool,
    /// Source field to extract from the upstream output.
    #[serde(default)]
    source_field: Option<String>,
    /// Target key in the downstream input (defaults to source_field).
    #[serde(default)]
    target_field: Option<String>,
}

// ── Node-level Router (LangGraph-style conditional routing) ─────────

/// A router configuration stored in a node's `config` JSON under the
/// `router` key. It decides which downstream nodes to execute based on the
/// node's output.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RouterConfig {
    #[serde(default)]
    pub rules: Vec<RouterRule>,
    #[serde(default)]
    pub default: Vec<String>,
}

impl RouterConfig {
    /// Parse a router config from a node's config JSON (the `router` field).
    pub fn from_node_config(config: &serde_json::Value) -> Option<Self> {
        config.get("router").map(|r| {
            serde_json::from_value(r.clone()).unwrap_or_default()
        })
    }

    /// Decide which downstream node ids to execute based on `output`.
    ///
    /// Returns the targets of the first matching rule, or the default targets.
    /// An empty result means "terminate this branch".
    pub fn route(&self, output: &serde_json::Value) -> Vec<String> {
        for rule in &self.rules {
            if rule.evaluate(output) {
                return rule.targets.clone();
            }
        }
        self.default.clone()
    }
}

/// A single routing rule: if `condition` matches the output, go to `targets`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterRule {
    #[serde(default)]
    pub condition: ConditionExpr,
    #[serde(default)]
    pub targets: Vec<String>,
}

impl RouterRule {
    fn evaluate(&self, output: &serde_json::Value) -> bool {
        self.condition.evaluate(output)
    }
}

/// A simple condition expression: `field OP value`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionExpr {
    /// Dot-path into the output JSON (e.g. "success", "issues.length").
    #[serde(default)]
    pub field: String,
    #[serde(default = "default_op")]
    pub op: String,
    #[serde(default)]
    pub value: serde_json::Value,
}

fn default_op() -> String {
    "==".to_string()
}

impl Default for ConditionExpr {
    fn default() -> Self {
        Self {
            field: String::new(),
            op: default_op(),
            value: serde_json::Value::Null,
        }
    }
}

impl ConditionExpr {
    fn evaluate(&self, output: &serde_json::Value) -> bool {
        if self.field.is_empty() {
            return true;
        }
        let actual = get_by_path(output, &self.field);
        match self.op.as_str() {
            "==" | "eq" => actual.as_ref() == Some(&self.value),
            "!=" | "ne" => actual.as_ref() != Some(&self.value),
            ">" | "gt" => num_cmp(actual.as_ref(), &self.value, |a, b| a > b),
            ">=" | "ge" => num_cmp(actual.as_ref(), &self.value, |a, b| a >= b),
            "<" | "lt" => num_cmp(actual.as_ref(), &self.value, |a, b| a < b),
            "<=" | "le" => num_cmp(actual.as_ref(), &self.value, |a, b| a <= b),
            "contains" => match (actual.as_ref(), &self.value) {
                (Some(serde_json::Value::String(s)), serde_json::Value::String(v)) => s.contains(v),
                (Some(serde_json::Value::Array(a)), v) => a.contains(v),
                _ => false,
            },
            _ => {
                // Unknown operator → default to true (pass through) + warn.
                tracing::warn!("unknown router operator '{}'; defaulting to true", self.op);
                true
            }
        }
    }
}

/// Resolve a dot-path like "issues.length" against a JSON value.
fn get_by_path(value: &serde_json::Value, path: &str) -> Option<serde_json::Value> {
    let mut current = value;
    for part in path.split('.') {
        if part == "length" {
            if let Some(arr) = current.as_array() {
                return Some(serde_json::Value::from(arr.len() as i64));
            }
            return None;
        }
        current = current.get(part)?;
    }
    Some(current.clone())
}

fn num_cmp(
    actual: Option<&serde_json::Value>,
    expected: &serde_json::Value,
    cmp: impl Fn(f64, f64) -> bool,
) -> bool {
    let a = actual.and_then(|v| v.as_f64());
    let b = expected.as_f64();
    match (a, b) {
        (Some(a), Some(b)) => cmp(a, b),
        _ => false,
    }
}

/// A simple convenience wrapper for sharing context across async tasks.
pub type SharedContext = Arc<WorkflowContext>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_eq_rule() {
        let cfg = RouterConfig {
            rules: vec![RouterRule {
                condition: ConditionExpr {
                    field: "success".into(),
                    op: "==".into(),
                    value: serde_json::json!(true),
                },
                targets: vec!["fixer".into()],
            }],
            default: vec!["reporter".into()],
        };
        assert_eq!(
            cfg.route(&serde_json::json!({"success": true})),
            vec!["fixer"]
        );
        assert_eq!(
            cfg.route(&serde_json::json!({"success": false})),
            vec!["reporter"]
        );
    }

    #[test]
    fn router_length_gt() {
        let cfg = RouterConfig {
            rules: vec![RouterRule {
                condition: ConditionExpr {
                    field: "issues.length".into(),
                    op: ">".into(),
                    value: serde_json::json!(5),
                },
                targets: vec!["fixer".into(), "reporter".into()],
            }],
            default: vec!["done".into()],
        };
        let issues: Vec<i32> = vec![1, 2, 3, 4, 5, 6];
        assert_eq!(
            cfg.route(&serde_json::json!({"issues": issues})),
            vec!["fixer", "reporter"]
        );
        assert_eq!(
            cfg.route(&serde_json::json!({"issues": [1, 2]})),
            vec!["done"]
        );
    }

    #[test]
    fn resolve_input_pass_through() {
        let ctx = WorkflowContext::new(serde_json::json!({"q": "hi"}));
        ctx.set_output("a", serde_json::json!({"result": "done"}));
        let edge = EdgeDef {
            id: "e1".into(),
            workflow_id: "wf".into(),
            source_node_id: "a".into(),
            target_node_id: "b".into(),
            source_handle: String::new(),
            target_handle: String::new(),
            label: "from_a".into(),
            condition: String::new(),
            data_mapping: serde_json::json!({"pass_through": true}),
            created_at: String::new(),
        };
        let input = ctx.resolve_input("b", &[&edge]);
        assert_eq!(input["from_a"]["result"], "done");
        assert_eq!(input["_workflow_input"]["q"], "hi");
    }

    // ── T29: Conditional routing integration tests ──

    #[test]
    fn router_ne_operator() {
        let cfg = RouterConfig {
            rules: vec![RouterRule {
                condition: ConditionExpr {
                    field: "status".into(),
                    op: "!=".into(),
                    value: serde_json::json!("ok"),
                },
                targets: vec!["alert".into()],
            }],
            default: vec!["proceed".into()],
        };
        assert_eq!(
            cfg.route(&serde_json::json!({"status": "error"})),
            vec!["alert"]
        );
        assert_eq!(
            cfg.route(&serde_json::json!({"status": "ok"})),
            vec!["proceed"]
        );
    }

    #[test]
    fn router_contains_operator_string() {
        let cfg = RouterConfig {
            rules: vec![RouterRule {
                condition: ConditionExpr {
                    field: "message".into(),
                    op: "contains".into(),
                    value: serde_json::json!("error"),
                },
                targets: vec!["handler".into()],
            }],
            default: vec!["skip".into()],
        };
        assert_eq!(
            cfg.route(&serde_json::json!({"message": "an error occurred"})),
            vec!["handler"]
        );
        assert_eq!(
            cfg.route(&serde_json::json!({"message": "all good"})),
            vec!["skip"]
        );
    }

    #[test]
    fn router_contains_operator_array() {
        let cfg = RouterConfig {
            rules: vec![RouterRule {
                condition: ConditionExpr {
                    field: "tags".into(),
                    op: "contains".into(),
                    value: serde_json::json!("critical"),
                },
                targets: vec!["escalate".into()],
            }],
            default: vec!["log".into()],
        };
        assert_eq!(
            cfg.route(&serde_json::json!({"tags": ["info", "critical"]})),
            vec!["escalate"]
        );
        assert_eq!(
            cfg.route(&serde_json::json!({"tags": ["info"]})),
            vec!["log"]
        );
    }

    #[test]
    fn router_first_matching_rule_wins() {
        let cfg = RouterConfig {
            rules: vec![
                RouterRule {
                    condition: ConditionExpr {
                        field: "score".into(),
                        op: ">=".into(),
                        value: serde_json::json!(90),
                    },
                    targets: vec!["promote".into()],
                },
                RouterRule {
                    condition: ConditionExpr {
                        field: "score".into(),
                        op: ">=".into(),
                        value: serde_json::json!(50),
                    },
                    targets: vec!["review".into()],
                },
            ],
            default: vec!["reject".into()],
        };
        // 95 matches the first rule
        assert_eq!(cfg.route(&serde_json::json!({"score": 95})), vec!["promote"]);
        // 70 matches the second rule (first doesn't match)
        assert_eq!(cfg.route(&serde_json::json!({"score": 70})), vec!["review"]);
        // 30 matches neither → default
        assert_eq!(cfg.route(&serde_json::json!({"score": 30})), vec!["reject"]);
    }

    #[test]
    fn router_empty_rules_uses_default() {
        let cfg = RouterConfig {
            rules: vec![],
            default: vec!["next".into()],
        };
        assert_eq!(cfg.route(&serde_json::json!({"anything": true})), vec!["next"]);
    }

    #[test]
    fn router_terminate_when_empty_targets() {
        let cfg = RouterConfig {
            rules: vec![RouterRule {
                condition: ConditionExpr {
                    field: "success".into(),
                    op: "==".into(),
                    value: serde_json::json!(false),
                },
                targets: vec![],
            }],
            default: vec!["continue".into()],
        };
        // When the rule matches and targets is empty → terminate (empty vec)
        assert!(cfg.route(&serde_json::json!({"success": false})).is_empty());
    }

    #[test]
    fn router_from_node_config_parses() {
        let node_config = serde_json::json!({
            "router": {
                "rules": [
                    {
                        "condition": {
                            "field": "success",
                            "op": "==",
                            "value": true
                        },
                        "targets": ["node-b"]
                    }
                ],
                "default": ["node-c"]
            }
        });
        let router = RouterConfig::from_node_config(&node_config);
        assert!(router.is_some());
        let router = router.unwrap();
        assert_eq!(router.rules.len(), 1);
        assert_eq!(
            router.route(&serde_json::json!({"success": true})),
            vec!["node-b"]
        );
    }

    #[test]
    fn router_unknown_operator_defaults_true() {
        let cfg = RouterConfig {
            rules: vec![RouterRule {
                condition: ConditionExpr {
                    field: "x".into(),
                    op: "invalid_op".into(),
                    value: serde_json::json!(1),
                },
                targets: vec!["fallback".into()],
            }],
            default: vec!["default".into()],
        };
        // Unknown operator → defaults to true (pass through)
        assert_eq!(cfg.route(&serde_json::json!({"x": 1})), vec!["fallback"]);
    }

    #[test]
    fn resolve_input_field_extraction() {
        let ctx = WorkflowContext::new(serde_json::json!({}));
        ctx.set_output("a", serde_json::json!({"result": "fixed", "count": 3}));
        let edge = EdgeDef {
            id: "e1".into(),
            workflow_id: "wf".into(),
            source_node_id: "a".into(),
            target_node_id: "b".into(),
            source_handle: String::new(),
            target_handle: String::new(),
            label: "from_a".into(),
            condition: String::new(),
            data_mapping: serde_json::json!({"pass_through": false, "source_field": "result"}),
            created_at: String::new(),
        };
        let input = ctx.resolve_input("b", &[&edge]);
        // Field extraction: "result" field from upstream output
        assert_eq!(input["result"], "fixed");
    }

    #[test]
    fn resolve_input_multi_upstream_merge() {
        let ctx = WorkflowContext::new(serde_json::json!({}));
        ctx.set_output("a", serde_json::json!({"val": 1}));
        ctx.set_output("b", serde_json::json!({"val": 2}));
        let edge_a = EdgeDef {
            id: "e1".into(),
            workflow_id: "wf".into(),
            source_node_id: "a".into(),
            target_node_id: "c".into(),
            source_handle: String::new(),
            target_handle: String::new(),
            label: "from_a".into(),
            condition: String::new(),
            data_mapping: serde_json::json!({"pass_through": true}),
            created_at: String::new(),
        };
        let edge_b = EdgeDef {
            id: "e2".into(),
            workflow_id: "wf".into(),
            source_node_id: "b".into(),
            target_node_id: "c".into(),
            source_handle: String::new(),
            target_handle: String::new(),
            label: "from_b".into(),
            condition: String::new(),
            data_mapping: serde_json::json!({"pass_through": true}),
            created_at: String::new(),
        };
        let input = ctx.resolve_input("c", &[&edge_a, &edge_b]);
        assert_eq!(input["from_a"]["val"], 1);
        assert_eq!(input["from_b"]["val"], 2);
    }

    #[test]
    fn context_shared_state_update() {
        let ctx = WorkflowContext::new(serde_json::json!({}));
        ctx.update_shared("counter", serde_json::json!(42));
        ctx.set_output("a", serde_json::json!({}));
        let edge = EdgeDef {
            id: "e1".into(),
            workflow_id: "wf".into(),
            source_node_id: "a".into(),
            target_node_id: "b".into(),
            source_handle: String::new(),
            target_handle: String::new(),
            label: "from_a".into(),
            condition: String::new(),
            data_mapping: serde_json::json!({"pass_through": true}),
            created_at: String::new(),
        };
        let input = ctx.resolve_input("b", &[&edge]);
        assert_eq!(input["_shared"]["counter"], 42);
    }
}
