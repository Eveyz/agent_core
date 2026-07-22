//! Context management — message construction, snapshots, and per-turn
//! context segment refresh.

use std::time::Duration;

use serde_json::Value;

use crate::config::MemoryMode;
use crate::context::ContextEngine as Context;
use crate::memory::{
    RECALL_HINT, RecallIntent, format_recall_results, intent_for_mode, route_recall_intent,
};
use crate::types::{Message, Role};

use super::Run;

impl Run {
    // ── Context management ────────────────────────────────────────

    pub(super) fn build_messages(&self) -> Vec<Message> {
        let mut messages = self.context.messages();
        for processor in &self.context_processors {
            messages = (processor.transform)(messages);
        }
        messages
    }

    pub(super) fn snapshot_messages_for_hook(&self, messages: &[Message]) -> Vec<Value> {
        messages
            .iter()
            .map(|m| {
                let content = m.content.as_deref().unwrap_or("");
                let preview = truncate_for_hook_preview(content);
                serde_json::json!({
                    "role": format!("{:?}", m.role),
                    "preview": preview
                })
            })
            .collect()
    }

    pub(super) fn refresh_context_segments(&mut self) {
        // ── Sync skill script tools to the Run's ToolRegistry ──────
        // Must run BEFORE tool catalog segment so new tools appear in the
        // catalog on the same turn they are registered.
        self.sync_skill_scripts();

        // Segment 3: ENVIRONMENT — use working_dir if set
        let cwd = self.working_dir.clone().or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
        });
        let env_str = Context::build_environment_string(cwd.as_deref(), None, None);
        self.context.set_environment(&env_str);

        // Segment 4: TOOL CATALOG — use cached render when available.
        // The cache is populated in run_loop() and only invalidated when tools
        // or permissions change. This avoids rebuilding the string every turn.
        if let Some((_, ref cached)) = self.tool_catalog_cache {
            self.context.set_tool_catalog(cached);
        } else {
            let tool_defs = self.registry.tool_definitions();
            let danger_map = super::build_danger_map(&tool_defs, &self.permission_policy);
            let tool_catalog = Context::build_tool_catalog_string(&tool_defs, &danger_map);
            self.context.set_tool_catalog(&tool_catalog);
        }

        // Segment 5: ACTIVE MEMORY — core blocks + project docs + recall gate
        {
            let mut mem_str = String::new();

            let mode = self.brain.memory_mode();
            mem_str.push_str(crate::prompt::memory_mode_prompt(&mode));
            mem_str.push_str("\n\n");

            if mode != MemoryMode::Stateless {
                // Core memory blocks (SQLite) — always inject when non-empty
                if let Some(ref mem_arc) = self.brain.memory {
                    if let Some(guard) = mem_arc.try_lock_for(Duration::from_secs(1)) {
                        let core = guard.core().to_nonempty_context_string();
                        if !core.is_empty() {
                            mem_str.push_str("Core Memory:\n");
                            mem_str.push_str(&core);
                            mem_str.push('\n');
                        }
                    }
                }

                // Layered project instructions (agverse.md):
                //   1. Global:   ~/.agverse/agverse.md  (cross-project catalog/memory)
                //   2. Project:  {cwd}/agverse.md  (or AGENTS.md) — active repo
                //   3. Local:    {cwd}/agverse.local.md  (gitignore, personal)
                //   4. Rules:    {cwd}/.agverse/rules/*.md  (modular, path-scoped later)
                let cwd = self.working_dir.clone().or_else(|| {
                    std::env::current_dir()
                        .ok()
                        .and_then(|p| p.to_str().map(|s| s.to_string()))
                });
                let mut instructions = Vec::new();

                // 1. Global (cross-project — NOT the active project)
                let global_path = crate::paths::get_global_agverse_md_path();
                if let Ok(content) = std::fs::read_to_string(&global_path) {
                    let injected = crate::memory::agverse_md::prepare_for_injection(
                        &content,
                        &crate::memory::agverse_md::InjectOptions::default(),
                    );
                    if !injected.is_empty() {
                        instructions.push(("Global Memory (cross-project)".to_string(), injected));
                    }
                }

                let mut global_local_path = global_path.clone();
                global_local_path.set_file_name("agverse.local.md");
                if let Ok(content) = std::fs::read_to_string(&global_local_path) {
                    let injected = crate::memory::agverse_md::prepare_for_injection(
                        &content,
                        &crate::memory::agverse_md::InjectOptions {
                            budget_chars: 1_500,
                            pending_inject_max: 0,
                        },
                    );
                    if !injected.is_empty() {
                        instructions.push((
                            "Global User Preferences (cross-project)".to_string(),
                            injected,
                        ));
                    }
                }

                // Extract recent conversation text to match path-scoped rules
                let conversation_text = self
                    .context
                    .raw_messages()
                    .iter()
                    .rev()
                    .take(5)
                    .filter_map(|m| m.content.as_deref())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .to_lowercase();

                // 2. Project root (active repo for this Working Directory)
                if let Some(ref dir) = cwd {
                    for name in &["agverse.md", "AGENTS.md"] {
                        let path = std::path::Path::new(dir).join(name);
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            let injected = crate::memory::agverse_md::prepare_for_injection(
                                &content,
                                &crate::memory::agverse_md::InjectOptions {
                                    budget_chars: 3_000,
                                    pending_inject_max: 0,
                                },
                            );
                            if !injected.is_empty() {
                                instructions.push((
                                    format!("Project Instructions (cwd: {name})"),
                                    injected,
                                ));
                            }
                            break;
                        }
                    }

                    // 3. Local (personal, gitignored)
                    let local_path = std::path::Path::new(dir).join("agverse.local.md");
                    if let Ok(content) = std::fs::read_to_string(&local_path) {
                        instructions.push(("Project Local (cwd)".to_string(), content));
                    }

                    // 4. Rules directory (.agverse/rules/*.md) - Path Scoped
                    let rules_dir = std::path::Path::new(dir).join(".agverse/rules");
                    if rules_dir.is_dir() {
                        if let Ok(entries) = std::fs::read_dir(&rules_dir) {
                            let mut rule_files: Vec<_> = entries
                                .filter_map(|e| e.ok())
                                .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
                                .collect();
                            rule_files.sort_by_key(|e| e.path());
                            for entry in rule_files {
                                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                                    let path = entry.path();
                                    let name =
                                        path.file_stem().and_then(|s| s.to_str()).unwrap_or("rule");

                                    let name_lower = name.to_lowercase();
                                    if name_lower == "global"
                                        || name_lower == "default"
                                        || conversation_text.contains(&name_lower)
                                    {
                                        instructions.push((format!("Rule: {name}"), content));
                                    }
                                }
                            }
                        }
                    }
                }

                if !instructions.is_empty() {
                    let mut parts = Vec::new();
                    for (label, content) in &instructions {
                        parts.push(format!("## {label}\n{content}"));
                    }
                    mem_str.push_str(&format!(
                        "Project Instructions:\n{}\n\n",
                        parts.join("\n\n")
                    ));
                }

                // Memory router (P2/P3): gate recall injection by intent + mode
                let last_user = self
                    .context
                    .raw_messages()
                    .iter()
                    .rev()
                    .find(|m| m.role == Role::User)
                    .and_then(|m| m.content.as_deref());

                let intent = intent_for_mode(route_recall_intent(last_user), mode);
                self.apply_recall_intent(&mut mem_str, last_user, intent);
            }

            if !mem_str.is_empty() {
                self.context.set_active_memory(&mem_str);
            }
        }

        // Segment 6: LOADED SKILLS — catalog + active skill content
        if let Some(ref sm) = self.skill_manager {
            let mut mgr = sm.lock();
            let sid = self.session_id.as_deref();
            let catalog = mgr.build_catalog_for(sid);
            let active = mgr.build_active_context_for(sid);
            let notes = mgr.drain_notes(sid);
            self.context.set_skill_catalog(&catalog);
            let mut skills_str = active;
            if !notes.is_empty() {
                if !skills_str.is_empty() {
                    skills_str.push_str("\n\n");
                }
                skills_str.push_str("### Skill activation notes\n");
                for note in notes {
                    skills_str.push_str(&format!("- {note}\n"));
                }
            }
            self.context.set_loaded_skills(&skills_str);
        }

        // Segment 7: EXECUTION PLAN — runtime phase dashboard + todos (+ optional goal)
        {
            let todos = self.session_todos();
            let list = todos.lock();
            self.execution.sync_from_todos(&list);
            let mut plan_str = String::new();

            if let Some(ref g) = self.goal {
                if !self.goal_completed {
                    plan_str.push_str(&format!(
                        "## PRIMARY GOAL (pinned)\n{g}\n\n\
                         Drive it to completion. Prefer tools over prose. \
                         If ambiguous, ask_user first.\n\n"
                    ));
                }
            }

            plan_str.push_str(&self.execution.to_injection(&list, self.mode));
            if let Some(line) = self
                .brain
                .todo_lists
                .parked_injection_line(self.session_id.as_deref())
            {
                plan_str.push('\n');
                plan_str.push_str(&line);
                plan_str.push('\n');
            }
            // Clear one-shot resume hint after it has been injected once.
            let _ = self.execution.take_resume_hint();

            if !plan_str.is_empty() {
                self.context.set_execution_plan(&plan_str);
            }
        }
    }

    /// Register / unregister `skill.<name>.<script>` tools so they match the
    /// currently active skills. Called once per turn from refresh_context_segments.
    fn sync_skill_scripts(&mut self) {
        use crate::tools::script::SkillScriptTool;

        // Skill scripts are executable capabilities and must not bypass the
        // Run mode's read-only restrictions.
        if self.mode != crate::mode::AgentMode::Build {
            if !self.registered_script_tools.is_empty() {
                let names: Vec<&str> = self
                    .registered_script_tools
                    .iter()
                    .map(String::as_str)
                    .collect();
                self.registry.remove_all(&names);
                self.registered_script_tools.clear();
                self.tool_catalog_cache = None;
            }
            return;
        }

        let mgr = match self.skill_manager.as_ref() {
            Some(sm) => sm,
            None => return,
        };

        let mgr = mgr.lock();
        let sid = self.session_id.as_deref();
        let active_scripts = mgr.get_active_scripts_for(sid);

        // Build the set of tool names we *should* have registered.
        let expected: std::collections::HashSet<String> = active_scripts
            .iter()
            .map(|(skill_name, script)| format!("skill.{}.{}", skill_name, script.name))
            .collect();

        let current: std::collections::HashSet<String> =
            self.registered_script_tools.iter().cloned().collect();

        // Register missing tools.
        let mut changed = false;
        for (skill_name, script) in &active_scripts {
            let tool_name = format!("skill.{}.{}", skill_name, script.name);
            if !current.contains(&tool_name) {
                if let Some(source_dir) = mgr.source_dir_of(skill_name) {
                    let tool = SkillScriptTool::new(skill_name, script, source_dir.to_path_buf())
                        .with_supervisor(self.supervisor.clone());
                    self.registry.register(Box::new(tool));
                    self.registered_script_tools.push(tool_name);
                    changed = true;
                }
            }
        }

        // Unregister tools whose skills were deactivated.
        let to_remove: Vec<String> = current.difference(&expected).cloned().collect();

        if !to_remove.is_empty() {
            let names: Vec<&str> = to_remove.iter().map(|s| s.as_str()).collect();
            self.registry.remove_all(&names);
            self.registered_script_tools
                .retain(|n| expected.contains(n));
            changed = true;
        }

        // Invalidate tool catalog cache so next turn rebuilds with new tools.
        if changed {
            self.tool_catalog_cache = None;
        }
    }

    /// Apply memory-router recall intent to the active memory string.
    fn apply_recall_intent(
        &self,
        mem_str: &mut String,
        last_user: Option<&str>,
        intent: RecallIntent,
    ) {
        match intent {
            RecallIntent::None => {}
            RecallIntent::Hint => {
                mem_str.push_str(RECALL_HINT);
                mem_str.push('\n');
            }
            RecallIntent::AutoInject => {
                let Some(query) = last_user else {
                    return;
                };
                let Some(ref mem_arc) = self.brain.memory else {
                    mem_str.push_str(RECALL_HINT);
                    mem_str.push('\n');
                    return;
                };

                let model = mem_arc
                    .try_lock_for(Duration::from_secs(1))
                    .and_then(|m| m.embedding_model().cloned());
                let embedding: Option<Vec<f32>> =
                    model.and_then(|model| model.embed_single(query).ok());

                let Some(guard) = mem_arc.try_lock_for(Duration::from_secs(1)) else {
                    mem_str.push_str(RECALL_HINT);
                    mem_str.push('\n');
                    return;
                };

                let Some(ref sid) = self.session_id else {
                    mem_str.push_str(RECALL_HINT);
                    mem_str.push('\n');
                    return;
                };
                let results = if let Some(ref emb) = embedding {
                    guard
                        .search_conversation_for_session_precomputed(sid, emb, query, 3)
                        .unwrap_or_else(|_| {
                            guard
                                .search_conversation_for_session_keyword(sid, query, 3)
                                .unwrap_or_default()
                        })
                } else {
                    guard
                        .search_conversation_for_session_keyword(sid, query, 3)
                        .unwrap_or_default()
                };

                let formatted = format_recall_results(&results, 1200);
                if formatted.is_empty() {
                    mem_str.push_str(RECALL_HINT);
                } else {
                    mem_str.push_str(&formatted);
                }
                mem_str.push('\n');
            }
        }
    }
}

/// Truncate message content for hook/snapshot previews so JSON payloads
/// stay small. Keeps the first N chars and appends "… (truncated)" if the
/// original was longer.
fn truncate_for_hook_preview(content: &str) -> String {
    const MAX: usize = 200;
    if content.len() <= MAX {
        return content.to_string();
    }
    // Find the char boundary at or before MAX to avoid mid-char panic.
    let trunc_at = content
        .char_indices()
        .take(MAX)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(MAX)
        .min(content.len());
    format!("{}… (truncated)", &content[..trunc_at])
}
