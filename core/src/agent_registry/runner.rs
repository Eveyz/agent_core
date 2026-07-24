use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{
    memory::{embedding::EmbeddingModel, storage::Storage},
    mode::AgentMode,
    permission::PermissionConfig,
    runtime::{supervisor::ProcessSupervisor, Brain},
    session::SessionManager,
    skills::SkillManager,
    subagent::Subagent,
    tools::{
        subagent::{re_wire_subagent_tools_with_skills, ApprovalRouting},
        ToolRegistry,
    },
    types::EventSender,
};

use super::{AgentDef, AgentHistoryEntry, AgentMemoryStore};

#[derive(Clone)]
pub struct CustomAgentRunner {
    storage: Storage,
    brain: Arc<Brain>,
    session_manager: Arc<SessionManager>,
}

pub struct CustomAgentInvocation {
    /// Frozen definition for this invocation. Callers that need durable
    /// execution persist this snapshot in their run manifest before invoking.
    pub agent: AgentDef,
    pub input: String,
    pub session_id: String,
    pub working_dir: Option<String>,
    pub workflow_run_id: Option<String>,
    pub trigger: String,
    pub permission_config: PermissionConfig,
    pub approval_resolver: Option<crate::runtime::ApprovalResolver>,
    pub cancel_token: CancellationToken,
    pub event_tx: Option<EventSender>,
    pub subagent_depth: u8,
    /// Whether to record this invocation in saved-agent history.
    pub record_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomAgentRunResult {
    pub output: String,
    pub success: bool,
    pub iterations_used: usize,
    pub transcript_ref: Option<String>,
    pub model_used: String,
}

struct AgentRunHistoryGuard {
    storage: Storage,
    entry: AgentHistoryEntry,
    runtime_id: Option<String>,
    started: std::time::Instant,
    finished: bool,
    enabled: bool,
}

impl AgentRunHistoryGuard {
    fn set_runtime(&mut self, runtime_id: String) {
        self.runtime_id = Some(runtime_id);
    }

    fn finish(&mut self) {
        self.finished = true;
    }
}

impl Drop for AgentRunHistoryGuard {
    fn drop(&mut self) {
        if self.finished || !self.enabled {
            return;
        }
        let recovery = self
            .runtime_id
            .as_ref()
            .and_then(|runtime_id| {
                crate::subagent::transcript::TranscriptRecorder::default_path(runtime_id)
                    .ok()
                    .filter(|path| path.exists())
                    .map(|path| {
                        format!(
                            "Runtime ID: {runtime_id}\nPartial transcript: {}",
                            path.display()
                        )
                    })
            })
            .unwrap_or_else(|| "Partial transcript could not be persisted".to_string());
        self.entry.output = format!("Custom agent was cancelled or aborted\n\n{recovery}");
        self.entry.success = false;
        self.entry.process_time_ms = self.started.elapsed().as_millis() as i64;
        let _ = super::history::record(&self.storage, &self.entry);
    }
}

impl CustomAgentRunner {
    pub fn new(storage: Storage, brain: Arc<Brain>, session_manager: Arc<SessionManager>) -> Self {
        Self {
            storage,
            brain,
            session_manager,
        }
    }

    pub async fn run(&self, invocation: CustomAgentInvocation) -> Result<CustomAgentRunResult> {
        let CustomAgentInvocation {
            agent,
            input,
            session_id,
            working_dir,
            workflow_run_id,
            trigger,
            permission_config,
            approval_resolver,
            cancel_token,
            event_tx,
            subagent_depth,
            record_history,
        } = invocation;

        let mut subagent_config = super::build_subagent_config(&agent);
        subagent_config.working_dir = working_dir.map(Into::into);
        let effective_skills = if let Some(ref skill_manager) = self.brain.skill_manager {
            skill_manager
                .lock()
                .resolve_subagent_skills(&agent.skills, Some(&session_id))
        } else {
            agent.skills.clone()
        };
        subagent_config.skills = effective_skills.clone();
        subagent_config.system_prompt = SkillManager::inject_skill_content_into(
            self.brain.skill_manager.as_ref(),
            &effective_skills,
            &subagent_config.system_prompt,
        );

        let model_config = super::build_model_config(&agent, &self.brain.config);
        let supervisor = Arc::new(Mutex::new(ProcessSupervisor::new()));
        let mut registry = if agent.tools.is_empty() {
            self.brain.build_tool_registry(AgentMode::Build)
        } else {
            ToolRegistry::from_names(&agent.tools)
        };
        let approval_routing = approval_resolver
            .clone()
            .map(ApprovalRouting::Run)
            .unwrap_or(ApprovalRouting::LegacyScoped);
        re_wire_subagent_tools_with_skills(
            &mut registry,
            model_config.clone(),
            Some(self.session_manager.clone()),
            permission_config.clone(),
            Some(supervisor.clone()),
            Some(cancel_token.clone()),
            subagent_depth,
            self.brain.skill_manager.clone(),
            approval_routing,
            Some(self.brain.todo_lists.clone()),
        );
        if registry.has("shell") {
            registry.register(Box::new(crate::tools::shell::ShellTool::with_supervisor(
                supervisor.clone(),
                None,
            )));
        }
        if registry.has("repl") {
            registry.register(Box::new(crate::tools::repl::ReplTool::with_supervisor(
                supervisor.clone(),
                None,
            )));
        }
        SkillManager::sync_skill_scripts_for_skills(
            self.brain.skill_manager.as_ref(),
            &effective_skills,
            &mut registry,
            Some(supervisor.clone()),
        );

        let memory = if agent.memory_enabled > 0 {
            Some(Arc::new(build_agent_memory_store(
                &self.brain,
                self.storage.clone(),
            )))
        } else {
            None
        };
        let mut history_guard = AgentRunHistoryGuard {
            storage: self.storage.clone(),
            entry: AgentHistoryEntry {
                agent_id: agent.id.clone(),
                session_id: session_id.clone(),
                workflow_run_id: workflow_run_id.clone().unwrap_or_default(),
                trigger: trigger.clone(),
                input: input.clone(),
                model_used: model_config.model_id.clone(),
                ..Default::default()
            },
            runtime_id: None,
            started: std::time::Instant::now(),
            finished: false,
            enabled: record_history,
        };
        let mut subagent = Subagent::new_with_memory(
            &agent.name,
            subagent_config,
            &model_config,
            registry,
            permission_config,
            memory,
            Some(agent.memory_identity()),
        )
        .with_runtime_scope(Some(session_id.clone()), None, workflow_run_id.clone())
        .with_supervisor(supervisor)
        .with_cancel_token(cancel_token);
        if let Some(resolver) = approval_resolver {
            subagent = subagent.with_approval_resolver(resolver);
        }

        let runtime_id = subagent.id().to_string();
        history_guard.set_runtime(runtime_id.clone());
        let started = std::time::Instant::now();
        let run_result = subagent.run_with_sender(&input, event_tx).await;
        let transcript_ref = subagent
            .transcript_path()
            .map(|path| path.display().to_string());
        let elapsed_ms = started.elapsed().as_millis() as i64;
        let result = match run_result {
            Ok(result) => result,
            Err(error) => {
                let recovery = transcript_ref
                    .as_ref()
                    .map(|path| format!("Partial transcript: {path}"))
                    .unwrap_or_else(|| "Partial transcript could not be persisted".to_string());
                history_guard.entry.output = format!("{error}\n\n{recovery}");
                history_guard.entry.success = false;
                history_guard.entry.process_time_ms = elapsed_ms;
                if record_history {
                    let entry = history_guard.entry.clone();
                    let storage = self.storage.clone();
                    tokio::task::spawn_blocking(move || super::history::record(&storage, &entry))
                        .await
                        .context("record custom agent failure task failed")??;
                }
                history_guard.finish();
                return Err(error.context(recovery));
            }
        };

        history_guard.entry.output = match &transcript_ref {
            Some(path) => format!("{}\n\nTranscript: {path}", result.output),
            None => result.output.clone(),
        };
        history_guard.entry.iterations_used = result.iterations_used as u32;
        history_guard.entry.success = result.success;
        history_guard.entry.process_time_ms = elapsed_ms;
        if record_history {
            let entry = history_guard.entry.clone();
            let storage = self.storage.clone();
            tokio::task::spawn_blocking(move || super::history::record(&storage, &entry))
                .await
                .context("record custom agent history task failed")??;
        }
        history_guard.finish();

        Ok(CustomAgentRunResult {
            output: result.output,
            success: result.success,
            iterations_used: result.iterations_used,
            transcript_ref,
            model_used: model_config.model_id,
        })
    }
}

fn build_agent_memory_store(brain: &Brain, storage: Storage) -> AgentMemoryStore {
    if let Some(ref memory) = brain.config.memory {
        if cfg!(feature = "embeddings") && memory.embedding_enabled {
            if let Ok(model) = EmbeddingModel::new(&memory.embedding_model) {
                return AgentMemoryStore::new(storage, Arc::new(model));
            }
        }
    }
    AgentMemoryStore::without_embedding(storage)
}
