use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    agent_registry,
    memory::storage::Storage,
    permission::{PermissionConfig, PermissionMode},
};

use super::{
    custom_agent::{FrozenCustomAgentConfig, CUSTOM_AGENT_ACTIVITY_KIND},
    legacy::classify_agent_effect,
    model::{
        EffectPolicy, NodeKey, NodeKind, NodeSpec, ResourceClaim, RetryPolicy, ValueExpr,
        WorkflowPolicy, WorkflowSpec,
    },
    reducer::validate_spec,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMention {
    pub agent_id: String,
    #[serde(default)]
    pub revision_id: String,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MentionManifest {
    #[serde(default)]
    pub mentions: Vec<AgentMention>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionTask {
    pub key: NodeKey,
    pub agent_id: String,
    pub instruction: String,
    #[serde(default)]
    pub depends_on: Vec<NodeKey>,
    #[serde(default)]
    pub inputs: BTreeMap<String, ValueExpr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionPlan {
    pub tasks: Vec<MentionTask>,
    pub result: ValueExpr,
    #[serde(default)]
    pub policy: WorkflowPolicy,
}

#[derive(Clone)]
pub struct MentionWorkflowCompiler {
    storage: Storage,
}

impl MentionWorkflowCompiler {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }

    pub fn compile(
        &self,
        manifest: &MentionManifest,
        plan: MentionPlan,
        caller_permission: &PermissionConfig,
    ) -> Result<WorkflowSpec> {
        if manifest.mentions.is_empty() {
            bail!("mention manifest must not be empty");
        }
        if plan.tasks.is_empty() {
            bail!("mention workflow plan must contain at least one task");
        }

        let mut mention_by_agent = HashMap::new();
        for mention in &manifest.mentions {
            if mention_by_agent
                .insert(mention.agent_id.as_str(), mention)
                .is_some()
            {
                bail!("agent '{}' is mentioned more than once", mention.agent_id);
            }
        }

        let mut used_agents = HashSet::new();
        let mut nodes = Vec::with_capacity(plan.tasks.len());
        for task in plan.tasks {
            if !mention_by_agent.contains_key(task.agent_id.as_str()) {
                bail!(
                    "task '{}' references agent '{}' outside the mention manifest",
                    task.key.0,
                    task.agent_id
                );
            }
            used_agents.insert(task.agent_id.clone());
            let agent = agent_registry::get(&self.storage, &task.agent_id)
                .with_context(|| format!("resolve mentioned agent '{}'", task.agent_id))?;
            if let Some(mention) = mention_by_agent.get(task.agent_id.as_str()) {
                if !mention.revision_id.is_empty() && mention.revision_id != agent.updated_at {
                    bail!(
                        "mentioned agent '{}' changed after it was selected; select it again",
                        task.agent_id
                    );
                }
            }
            let agent_permission =
                agent_registry::build_permission_config(&agent, caller_permission);
            let permission = intersect_permission_ceiling(caller_permission, agent_permission);
            let effect = classify_agent_effect(&agent.tools, &permission);
            let mut inputs = task.inputs;
            if inputs
                .insert(
                    "instruction".to_string(),
                    ValueExpr::Literal {
                        value: serde_json::Value::String(task.instruction),
                    },
                )
                .is_some()
            {
                bail!(
                    "task '{}' may not override the reserved 'instruction' input",
                    task.key.0
                );
            }
            nodes.push(NodeSpec {
                key: task.key,
                kind: NodeKind::Activity {
                    kind: CUSTOM_AGENT_ACTIVITY_KIND.to_string(),
                    config: serde_json::to_value(FrozenCustomAgentConfig {
                        agent,
                        permission,
                        record_history: true,
                    })?,
                },
                inputs,
                after: task.depends_on,
                retry: RetryPolicy {
                    max_attempts: 1,
                    backoff_ms: 0,
                },
                timeout_ms: None,
                effect,
                resources: if effect == EffectPolicy::WorkspaceWrite {
                    vec![ResourceClaim {
                        resource: "workspace".to_string(),
                        exclusive: true,
                    }]
                } else {
                    Vec::new()
                },
            });
        }

        for mention in &manifest.mentions {
            if !mention.optional && !used_agents.contains(&mention.agent_id) {
                bail!(
                    "mentioned agent '{}' is not assigned to any task",
                    mention.agent_id
                );
            }
        }
        let spec = WorkflowSpec {
            schema_version: 1,
            nodes,
            result: plan.result,
            policy: plan.policy,
        };
        validate_spec(&spec)?;
        Ok(spec)
    }
}

pub fn derive_mention_request_id(parent_prompt_id: &str, tool_call_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"mention-workflow@1\0");
    digest.update(parent_prompt_id.as_bytes());
    digest.update(b"\0");
    digest.update(tool_call_id.as_bytes());
    format!("mention:{}", hex::encode(digest.finalize()))
}

pub(crate) fn intersect_permission_ceiling(
    caller: &PermissionConfig,
    mut requested: PermissionConfig,
) -> PermissionConfig {
    if permission_rank(requested.mode) > permission_rank(caller.mode) {
        requested.mode = caller.mode;
    }
    requested.auto_allow_up_to = match (caller.auto_allow_up_to, requested.auto_allow_up_to) {
        (Some(caller), Some(requested)) => Some(caller.min(requested)),
        (Some(caller), None) => Some(caller),
        (None, Some(_)) | (None, None) => None,
    };
    // Caller denials and sandbox boundaries are never discarded by an agent.
    requested.blacklist.extend(caller.blacklist.iter().cloned());
    requested.sandbox_paths = caller.sandbox_paths.clone();
    requested
}

fn permission_rank(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::Paranoid => 0,
        PermissionMode::Standard => 1,
        PermissionMode::Developer => 2,
        PermissionMode::Permissive => 3,
        PermissionMode::Yolo => 4,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent_registry::AgentDef;
    use crate::permission::DangerLevel;

    fn compiler_with_agents() -> (tempfile::TempDir, MentionWorkflowCompiler) {
        let directory = tempfile::tempdir().expect("tempdir");
        let storage =
            Storage::new(directory.path().join("db.sqlite").to_str().expect("path")).expect("db");
        for (id, name) in [("code", "Code"), ("review", "Review")] {
            agent_registry::create(
                &storage,
                &AgentDef {
                    id: id.to_string(),
                    name: name.to_string(),
                    permission_mode: "yolo".to_string(),
                    ..AgentDef::default()
                },
            )
            .expect("create agent");
        }
        (directory, MentionWorkflowCompiler::new(storage))
    }

    #[test]
    fn compiles_general_dependency_graph_and_freezes_agents() {
        let (_directory, compiler) = compiler_with_agents();
        let manifest = MentionManifest {
            mentions: vec![
                AgentMention {
                    agent_id: "code".to_string(),
                    revision_id: String::new(),
                    optional: false,
                },
                AgentMention {
                    agent_id: "review".to_string(),
                    revision_id: String::new(),
                    optional: false,
                },
            ],
        };
        let plan = MentionPlan {
            tasks: vec![
                MentionTask {
                    key: NodeKey::from("implementation"),
                    agent_id: "code".to_string(),
                    instruction: "Implement it".to_string(),
                    depends_on: Vec::new(),
                    inputs: BTreeMap::new(),
                },
                MentionTask {
                    key: NodeKey::from("quality"),
                    agent_id: "review".to_string(),
                    instruction: "Review it".to_string(),
                    depends_on: vec![NodeKey::from("implementation")],
                    inputs: BTreeMap::from([(
                        "upstream".to_string(),
                        ValueExpr::NodeOutput {
                            node: NodeKey::from("implementation"),
                            pointer: "/summary".to_string(),
                        },
                    )]),
                },
            ],
            result: ValueExpr::NodeOutput {
                node: NodeKey::from("quality"),
                pointer: String::new(),
            },
            policy: WorkflowPolicy::default(),
        };
        let mut caller = PermissionConfig::default();
        caller.mode = PermissionMode::Standard;
        caller.auto_allow_up_to = Some(DangerLevel::ReadOnly);

        let spec = compiler
            .compile(&manifest, plan, &caller)
            .expect("compile mention workflow");
        assert_eq!(spec.nodes.len(), 2);
        assert_eq!(spec.nodes[1].after, vec![NodeKey::from("implementation")]);
        let NodeKind::Activity { config, .. } = &spec.nodes[0].kind else {
            panic!("custom agent activity")
        };
        assert_eq!(config["agent"]["id"], json!("code"));
        assert_eq!(config["permission"]["mode"], json!("standard"));
    }

    #[test]
    fn rejects_unmentioned_agent_even_when_it_exists() {
        let (_directory, compiler) = compiler_with_agents();
        let error = compiler
            .compile(
                &MentionManifest {
                    mentions: vec![AgentMention {
                        agent_id: "code".to_string(),
                        revision_id: String::new(),
                        optional: false,
                    }],
                },
                MentionPlan {
                    tasks: vec![MentionTask {
                        key: NodeKey::from("task"),
                        agent_id: "review".to_string(),
                        instruction: "Sneak in".to_string(),
                        depends_on: Vec::new(),
                        inputs: BTreeMap::new(),
                    }],
                    result: ValueExpr::Literal { value: json!(null) },
                    policy: WorkflowPolicy::default(),
                },
                &PermissionConfig::default(),
            )
            .expect_err("must reject unmentioned agent");
        assert!(error.to_string().contains("outside the mention manifest"));
    }

    #[test]
    fn request_id_is_stable_and_scoped_to_tool_call() {
        let first = derive_mention_request_id("prompt", "tool-a");
        assert_eq!(first, derive_mention_request_id("prompt", "tool-a"));
        assert_ne!(first, derive_mention_request_id("prompt", "tool-b"));
    }
}
