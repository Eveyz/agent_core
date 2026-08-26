use super::{AgentMemoryIdentity, MAX_SUBAGENT_DEPTH, ResultStrategy};
use crate::permission::PermissionConfig;
use anyhow::Result;
use std::path::PathBuf;

pub const SUBAGENT_PROMPT_SCHEMA: &str = "subagent-prompt/v1";
pub fn output_contract(strategy: ResultStrategy) -> &'static str {
    match strategy {
        ResultStrategy::Full => {
            "Return complete evidence and artifact references. End with <context_status>{\"sufficient\":true|false,\"missing\":[...],\"unresolved\":[...]}</context_status>."
        }
        ResultStrategy::Summary => {
            "Return concise findings, missing context, and artifact references. End with <context_status>{\"sufficient\":true|false,\"missing\":[...],\"unresolved\":[...]}</context_status>."
        }
        ResultStrategy::Auto => {
            "Return decision-ready findings and explicitly identify any missing context. End with <context_status>{\"sufficient\":true|false,\"missing\":[...],\"unresolved\":[...]}</context_status>."
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PromptLayers {
    pub base: String,
    pub persona: String,
    pub runtime: String,
    pub output_contract: String,
}

impl PromptLayers {
    pub fn render(&self) -> String {
        let mut sections = vec![
            format!("[{SUBAGENT_PROMPT_SCHEMA}]"),
            self.base.trim().to_string(),
        ];
        for (heading, body) in [
            ("Persona", self.persona.trim()),
            ("Runtime Scope", self.runtime.trim()),
            ("Output Contract", self.output_contract.trim()),
        ] {
            if !body.is_empty() {
                sections.push(format!("=== {heading} ===\n{body}"));
            }
        }
        sections.join("\n\n")
    }
}

#[derive(Debug, Clone)]
pub struct ParentAgentSpec {
    pub available_tools: Vec<String>,
    pub max_iterations: usize,
    pub max_context_tokens: usize,
    pub permission: PermissionConfig,
    pub working_dir: PathBuf,
    pub recursion_depth: u8,
}

#[derive(Debug, Clone)]
pub struct AgentSpawnRequest {
    pub role_name: String,
    pub requested_tools: Option<Vec<String>>,
    pub requested_max_iterations: Option<usize>,
    pub skills: Vec<String>,
    pub prompt: PromptLayers,
    pub result_strategy: ResultStrategy,
    pub memory_identity: Option<AgentMemoryIdentity>,
}

#[derive(Debug, Clone)]
pub struct EffectiveAgentSpec {
    pub role_name: String,
    pub system_prompt: String,
    pub tools: Vec<String>,
    pub max_iterations: usize,
    pub max_context_tokens: usize,
    pub skills: Vec<String>,
    pub permission: PermissionConfig,
    pub working_dir: PathBuf,
    pub recursion_depth: u8,
    pub result_strategy: ResultStrategy,
    pub memory_identity: Option<AgentMemoryIdentity>,
}

impl EffectiveAgentSpec {
    pub fn resolve(parent: ParentAgentSpec, request: AgentSpawnRequest) -> Result<Self> {
        if parent.recursion_depth >= MAX_SUBAGENT_DEPTH {
            anyhow::bail!(
                "subagent recursion depth {} exceeds maximum {}",
                parent.recursion_depth + 1,
                MAX_SUBAGENT_DEPTH
            );
        }

        let requested = request
            .requested_tools
            .unwrap_or_else(|| parent.available_tools.clone());
        let mut tools = Vec::new();
        for tool in requested {
            if parent.available_tools.contains(&tool)
                && !is_meta_dispatch_tool(&tool)
                && !tools.contains(&tool)
            {
                tools.push(tool);
            }
        }

        let requested_iterations = request
            .requested_max_iterations
            .unwrap_or(parent.max_iterations);

        Ok(Self {
            role_name: request.role_name,
            system_prompt: request.prompt.render(),
            tools,
            max_iterations: requested_iterations.clamp(1, parent.max_iterations.max(1)),
            max_context_tokens: parent.max_context_tokens,
            skills: request.skills,
            permission: parent.permission,
            working_dir: parent.working_dir,
            recursion_depth: parent.recursion_depth + 1,
            result_strategy: request.result_strategy,
            memory_identity: request.memory_identity,
        })
    }
}

pub fn is_meta_dispatch_tool(name: &str) -> bool {
    matches!(
        name,
        "subagent"
            | "subagents"
            | "subagent_transcript"
            | "task_execute"
            | "skill_list"
            | "skill_search"
            | "skill_load"
            | "skill_deactivate"
            | "skill_reload"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::PermissionMode;

    #[test]
    fn effective_spec_is_a_strict_child_of_parent_runtime() {
        let mut permission = PermissionConfig::default();
        permission.mode = PermissionMode::Paranoid;
        let effective = EffectiveAgentSpec::resolve(
            ParentAgentSpec {
                available_tools: vec!["read_file".into(), "shell".into(), "subagent".into()],
                max_iterations: 12,
                max_context_tokens: 128_000,
                permission,
                working_dir: PathBuf::from("/workspace"),
                recursion_depth: 0,
            },
            AgentSpawnRequest {
                role_name: "reviewer".into(),
                requested_tools: Some(vec!["shell".into(), "write_file".into(), "subagent".into()]),
                requested_max_iterations: Some(99),
                skills: vec!["review".into()],
                prompt: PromptLayers {
                    base: "Do the task".into(),
                    persona: "Be skeptical".into(),
                    runtime: "Working Directory: /workspace".into(),
                    output_contract: "Return evidence".into(),
                },
                result_strategy: ResultStrategy::Summary,
                memory_identity: None,
            },
        )
        .expect("effective spec");

        assert_eq!(effective.tools, vec!["shell"]);
        assert_eq!(effective.max_iterations, 12);
        assert_eq!(effective.max_context_tokens, 128_000);
        assert_eq!(effective.permission.mode, PermissionMode::Paranoid);
        assert_eq!(effective.recursion_depth, 1);
        assert!(effective.system_prompt.starts_with("[subagent-prompt/v1]"));
        assert!(
            effective.system_prompt.find("Persona").unwrap()
                < effective.system_prompt.find("Runtime Scope").unwrap()
        );
    }
}
