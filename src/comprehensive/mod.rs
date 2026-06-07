use anyhow::Result;
use std::sync::{Arc, Mutex};

use crate::agent::AgentBuilder;
use crate::background::BackgroundPool;
use crate::cron::CronScheduler;
use crate::hooks::HookRegistry;
use crate::permission::PermissionPolicy;
use crate::skills::SkillManager;
use crate::tasks::TaskBoard;
use crate::teams::AgentTeam;
use crate::todo::TodoList;
use crate::worktree::WorktreeManager;

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
            parts.push(format!(
                "Tasks: {} total",
                board.lock().unwrap().all_tasks().len()
            ));
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
