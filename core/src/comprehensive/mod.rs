use anyhow::{Context as _, Result};
use parking_lot::Mutex;
use std::sync::Arc;

use crate::agent::AgentBuilder;
use crate::background::BackgroundPool;
use crate::cron::CronScheduler;
use crate::hooks::HookRegistry;
use crate::permission::PermissionPolicy;
use crate::reflector::{Reflector, Suggestion, SuggestionAction};
use crate::skills::SkillManager;
use crate::tasks::TaskBoard;
use crate::teams::AgentTeam;
use crate::todo::TodoList;
use crate::worktree::WorktreeManager;
use std::path::PathBuf;

pub struct ComprehensiveAgentBuilder {
    config_path: Option<String>,
    enable_memory: bool,
    enable_permission: bool,
    enable_hooks: bool,
    enable_todo: bool,
    enable_tasks: bool,
    enable_background: bool,
    enable_cron: bool,
    enable_skills: bool,
    skills_dir: Option<String>,
    enable_teams: bool,
    team_name: Option<String>,
    enable_worktree: bool,
    repo_root: Option<String>,
    /// Enable offline reflection (Phase F). Off by default.
    enable_reflector: bool,
    /// Directory where auto-applied skills are written.
    reflector_skills_dir: Option<String>,
}

impl Default for ComprehensiveAgentBuilder {
    fn default() -> Self {
        Self {
            config_path: None,
            enable_memory: false,
            enable_permission: true,
            enable_hooks: true,
            enable_todo: true,
            enable_tasks: false,
            enable_background: false,
            enable_cron: false,
            enable_skills: false,
            skills_dir: None,
            enable_teams: false,
            team_name: None,
            enable_worktree: false,
            repo_root: None,
            enable_reflector: false,
            reflector_skills_dir: None,
        }
    }
}

impl ComprehensiveAgentBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn config(mut self, path: &str) -> Self {
        self.config_path = Some(path.to_string());
        self
    }

    pub fn memory(mut self, enable: bool) -> Self {
        self.enable_memory = enable;
        self
    }

    pub fn permission(mut self, enable: bool) -> Self {
        self.enable_permission = enable;
        self
    }

    pub fn hooks(mut self, enable: bool) -> Self {
        self.enable_hooks = enable;
        self
    }

    pub fn todo(mut self, enable: bool) -> Self {
        self.enable_todo = enable;
        self
    }

    pub fn tasks(mut self, enable: bool) -> Self {
        self.enable_tasks = enable;
        self
    }

    pub fn background(mut self, enable: bool) -> Self {
        self.enable_background = enable;
        self
    }

    pub fn cron(mut self, enable: bool) -> Self {
        self.enable_cron = enable;
        self
    }

    pub fn skills(mut self, enable: bool, dir: Option<&str>) -> Self {
        self.enable_skills = enable;
        self.skills_dir = dir.map(String::from);
        self
    }

    pub fn teams(mut self, enable: bool, team_name: Option<&str>) -> Self {
        self.enable_teams = enable;
        self.team_name = team_name.map(String::from);
        self
    }

    pub fn worktree(mut self, enable: bool, repo_root: Option<&str>) -> Self {
        self.enable_worktree = enable;
        self.repo_root = repo_root.map(String::from);
        self
    }

    /// Enable offline reflection. When enabled, the agent exposes
    /// [`ComprehensiveAgent::reflect_on`] which analyzes a trace file and
    /// auto-applies safe suggestions (append-only skills) while surfacing
    /// everything else for approval. Off by default.
    pub fn reflector(mut self, enable: bool, skills_dir: Option<&str>) -> Self {
        self.enable_reflector = enable;
        self.reflector_skills_dir = skills_dir.map(String::from);
        self
    }

    pub fn build(self) -> Result<ComprehensiveAgent> {
        let mut agent_builder = if let Some(ref path) = self.config_path {
            AgentBuilder::from_config(path)?
        } else {
            AgentBuilder::from_env()?
        };

        agent_builder = agent_builder.with_memory(self.enable_memory);
        let agent = agent_builder.build()?;

        let permission_policy = if self.enable_permission {
            Some(PermissionPolicy::new())
        } else {
            None
        };

        let hook_registry = if self.enable_hooks {
            Some(HookRegistry::new())
        } else {
            None
        };

        let todo_list = if self.enable_todo {
            Some(TodoList::new())
        } else {
            None
        };

        let task_board = if self.enable_tasks {
            Some(Arc::new(Mutex::new(TaskBoard::new())))
        } else {
            None
        };

        let background_pool = if self.enable_background {
            Some(BackgroundPool::new())
        } else {
            None
        };

        let cron_scheduler = if self.enable_cron {
            Some(CronScheduler::new())
        } else {
            None
        };

        let skill_manager = if self.enable_skills {
            let mut loader = if let Some(dir) = self.skills_dir {
                SkillManager::new(std::path::PathBuf::from(dir))
            } else {
                SkillManager::with_defaults()
            };
            let _ = loader.scan();
            Some(loader)
        } else {
            None
        };

        let team = if self.enable_teams {
            let name = self.team_name.as_deref().unwrap_or("default");
            Some(AgentTeam::new(name))
        } else {
            None
        };

        let worktree_manager = if self.enable_worktree {
            let root = self
                .repo_root
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            Some(WorktreeManager::new(root))
        } else {
            None
        };

        let reflector = if self.enable_reflector {
            let dir = self
                .reflector_skills_dir
                .map(PathBuf::from)
                .unwrap_or_else(crate::paths::get_skills_dir);
            Some(Reflector::new(dir))
        } else {
            None
        };

        Ok(ComprehensiveAgent {
            agent,
            permission_policy,
            hook_registry,
            todo_list,
            task_board,
            background_pool,
            cron_scheduler,
            skill_manager,
            team,
            worktree_manager,
            reflector,
        })
    }
}

pub struct ComprehensiveAgent {
    pub agent: crate::agent::Agent,
    pub permission_policy: Option<PermissionPolicy>,
    pub hook_registry: Option<HookRegistry>,
    pub todo_list: Option<TodoList>,
    pub task_board: Option<Arc<Mutex<TaskBoard>>>,
    pub background_pool: Option<BackgroundPool>,
    pub cron_scheduler: Option<CronScheduler>,
    pub skill_manager: Option<SkillManager>,
    pub team: Option<AgentTeam>,
    pub worktree_manager: Option<WorktreeManager>,
    /// Offline reflector (Phase F). None unless explicitly enabled.
    pub reflector: Option<Reflector>,
}

impl ComprehensiveAgent {
    pub async fn run(&mut self, input: &str) -> Result<String> {
        self.agent.run(input).await
    }

    pub fn has_permission(&self) -> bool {
        self.permission_policy.is_some()
    }

    pub fn has_hooks(&self) -> bool {
        self.hook_registry.is_some()
    }

    pub fn has_todo(&self) -> bool {
        self.todo_list.is_some()
    }

    pub fn has_tasks(&self) -> bool {
        self.task_board.is_some()
    }

    pub fn has_background(&self) -> bool {
        self.background_pool.is_some()
    }

    pub fn has_cron(&self) -> bool {
        self.cron_scheduler.is_some()
    }

    pub fn has_skills(&self) -> bool {
        self.skill_manager.is_some()
    }

    pub fn has_team(&self) -> bool {
        self.team.is_some()
    }

    pub fn has_worktree(&self) -> bool {
        self.worktree_manager.is_some()
    }

    pub fn has_reflector(&self) -> bool {
        self.reflector.is_some()
    }

    /// Analyze a trace file and apply safe suggestions automatically.
    ///
    /// Returns a [`ReflectionReport`] summarizing what was auto-applied, what
    /// needs approval, and what was forbidden. Auto-application is limited to
    /// append-only skills; everything else is collected for human review.
    ///
    /// The optional LLM enrichment step (`enrich`) rewrites each suggestion's
    /// rationale using the agent's model. It is purely cosmetic and cannot
    /// change the safety classification. Pass `false` to skip it (e.g. when
    /// running offline).
    pub async fn reflect_on(
        &mut self,
        trace_path: &std::path::Path,
        enrich: bool,
    ) -> Result<ReflectionReport> {
        let reflector = self.reflector.as_ref().context("reflector not enabled")?;

        let records = Reflector::load_trace(trace_path).await?;
        let events: Vec<_> = records.iter().filter_map(|r| r.to_digest_event()).collect();
        let mut suggestions = reflector.analyze(&events);

        if enrich {
            // Borrow the agent's client for the cosmetic rationale rewrite.
            // The client lives behind a private field on Agent; expose it via
            // a helper. We use the current model configured on the agent.
            if let Some(client) = self.agent.client_for_reflection() {
                reflector.enrich_with_llm(&mut suggestions, client).await;
            }
        }

        let mut report = ReflectionReport::default();
        for sug in &suggestions {
            match reflector.apply(sug).await? {
                SuggestionAction::Applied => report.applied.push(sug.clone()),
                SuggestionAction::NeedsApproval(diff) => {
                    report.needs_approval.push((sug.clone(), diff));
                }
                SuggestionAction::Forbidden => report.forbidden.push(sug.clone()),
            }
        }
        Ok(report)
    }

    pub fn status(&self) -> String {
        let mut parts = vec!["Agent: ready".to_string()];

        if self.has_permission() {
            parts.push("Permission: enabled".to_string());
        }
        if self.has_hooks() {
            parts.push("Hooks: enabled".to_string());
        }
        if self.has_todo()
            && let Some(ref todo) = self.todo_list
        {
            parts.push(format!("Todo: {}", todo.summary()));
        }
        if self.has_tasks()
            && let Some(ref board) = self.task_board
        {
            parts.push(format!("Tasks: {} total", board.lock().all_tasks().len()));
        }
        if self.has_background()
            && let Some(ref pool) = self.background_pool
        {
            parts.push(format!("Background: {} running", pool.running_count()));
        }
        if self.has_cron()
            && let Some(ref scheduler) = self.cron_scheduler
        {
            parts.push(format!("Cron: {} jobs", scheduler.len()));
        }
        if self.has_skills()
            && let Some(ref loader) = self.skill_manager
        {
            parts.push(format!("Skills: {} loaded", loader.list().len()));
        }
        if self.has_team()
            && let Some(ref team) = self.team
        {
            parts.push(format!("Team: {} agents", team.agent_count()));
        }
        if self.has_worktree()
            && let Some(ref wt) = self.worktree_manager
        {
            parts.push(format!("Worktrees: {} active", wt.list_active().len()));
        }

        parts.join("\n")
    }
}

/// Outcome of a reflection pass.
#[derive(Debug, Default)]
pub struct ReflectionReport {
    /// Suggestions auto-applied (append-only skills).
    pub applied: Vec<Suggestion>,
    /// Suggestions needing human approval, with their diff preview.
    pub needs_approval: Vec<(Suggestion, String)>,
    /// Suggestions refused because they touch security fields.
    pub forbidden: Vec<Suggestion>,
}

impl ReflectionReport {
    pub fn total(&self) -> usize {
        self.applied.len() + self.needs_approval.len() + self.forbidden.len()
    }

    /// Human-readable summary.
    pub fn summary(&self) -> String {
        format!(
            "Reflection complete: {} applied, {} need approval, {} forbidden (of {} total).",
            self.applied.len(),
            self.needs_approval.len(),
            self.forbidden.len(),
            self.total(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(dir: &TempDir) -> std::path::PathBuf {
        let cfg = dir.path().join("config.toml");
        std::fs::write(
            &cfg,
            "\ndefault_model = \"test/default\"\n\n[providers.test]\nname = \"test\"\nbase_url = \"http://127.0.0.1:1\"\napi_key = \"sk-test\"\n\n[providers.test.models]\ndefault = { model_id = \"mock\" }\n",
        )
        .unwrap();
        cfg
    }

    fn trace_with_errors(dir: &TempDir) -> std::path::PathBuf {
        let p = dir.path().join("trace.jsonl");
        let lines = vec![
            r#"{"ts":"2026-06-18T10:00:00Z","event":"TurnStart","turn_index":0}"#,
            r#"{"ts":"2026-06-18T10:00:01Z","event":{"ToolExecutionEnd":{"tool_call_id":"c1","tool_name":"bash","result":"Error: not found","is_error":true}}}"#,
            r#"{"ts":"2026-06-18T10:00:02Z","event":{"ToolExecutionEnd":{"tool_call_id":"c2","tool_name":"bash","result":"Error: not found","is_error":true}}}"#,
            r#"{"ts":"2026-06-18T10:00:03Z","event":{"ToolExecutionEnd":{"tool_call_id":"c3","tool_name":"bash","result":"Error: not found","is_error":true}}}"#,
        ];
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        p
    }

    /// Phase F #1: reflect_on analyzes a trace and auto-applies a skill,
    /// without needing a live LLM (enrich disabled).
    #[tokio::test]
    async fn reflect_on_applies_safe_skill_without_llm() {
        let dir = TempDir::new().unwrap();
        let cfg = write_config(&dir);
        let skills_dir = dir.path().join("skills");

        let mut agent = ComprehensiveAgentBuilder::new()
            .config(cfg.to_str().unwrap())
            .memory(false)
            .reflector(true, Some(skills_dir.to_str().unwrap()))
            .build()
            .unwrap();

        assert!(agent.has_reflector());

        let trace = trace_with_errors(&dir);
        // enrich=false to avoid hitting the (unreachable) model endpoint.
        let report = agent.reflect_on(&trace, false).await.unwrap();

        assert!(report.total() >= 1, "expected at least one suggestion");
        assert!(
            !report.applied.is_empty(),
            "the consecutive-error skill should be auto-applied"
        );
        assert!(
            report.forbidden.is_empty(),
            "no security-field suggestions expected from this trace"
        );
        // The skill file was actually written.
        assert!(std::fs::read_dir(&skills_dir).unwrap().count() >= 1);
        assert!(report.summary().contains("Reflection complete"));
    }

    /// Phase F #1: reflect_on with no errors produces an empty report.
    #[tokio::test]
    async fn reflect_on_clean_trace_is_empty() {
        let dir = TempDir::new().unwrap();
        let cfg = write_config(&dir);
        let trace = dir.path().join("clean.jsonl");
        std::fs::write(
            &trace,
            r##"{"ts":"2026-06-18T10:00:00Z","event":"TurnStart","turn_index":0}"##,
        )
        .unwrap();

        let mut agent = ComprehensiveAgentBuilder::new()
            .config(cfg.to_str().unwrap())
            .memory(false)
            .reflector(true, Some(dir.path().join("s").to_str().unwrap()))
            .build()
            .unwrap();

        let report = agent.reflect_on(&trace, false).await.unwrap();
        assert_eq!(report.total(), 0);
    }
}
