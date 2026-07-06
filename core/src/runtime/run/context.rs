//! Context management — message construction, snapshots, goal decomposition,
//! and per-turn context segment refresh.

use serde_json::Value;

use crate::context::ContextEngine as Context;
use crate::runtime::event::TodoItemPayload;
use crate::types::Message;

use super::{Run, RunError, GOAL_DECOMPOSE_SYSTEM};

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

    /// Decompose a pinned goal into todo items via a lightweight LLM call.
    pub(super) async fn decompose_goal(&self, goal: &str) -> Result<Vec<TodoItemPayload>, RunError> {
        let msgs = vec![
            Message::system(GOAL_DECOMPOSE_SYSTEM),
            Message::user(&format!(
                "Break down this goal into 3-8 concrete, actionable subtasks.\n\
                 Output a JSON array of objects with a single \"description\" field.\n\
                 Example: [{{\"description\":\"...\"}}]\n\nGoal: {goal}"
            )),
        ];
        let (resp, _) = self
            .client
            .chat_completion(&msgs, &[])
            .await
            .map_err(|e| RunError::Failed(format!("goal decompose model call failed: {e}")))?;
        let json = super::extract_json_array(&resp);
        let arr: Vec<Value> = serde_json::from_str(&json)
            .map_err(|e| RunError::Failed(format!("goal decompose parse failed: {e}")))?;
        let descs: Vec<String> = arr
            .iter()
            .filter_map(|v| v.get("description").and_then(|d| d.as_str()).map(|s| s.to_string()))
            .collect();
        if descs.is_empty() {
            return Err(RunError::Failed("goal decomposition produced no tasks".to_string()));
        }
        {
            let mut list = self.brain.todo_list.lock();
            list.replace_all(descs.clone());
        }
        Ok(descs
            .into_iter()
            .enumerate()
            .map(|(i, d)| TodoItemPayload {
                id: (i + 1).to_string(),
                description: d,
                status: "pending".to_string(),
            })
            .collect())
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

        // Segment 5: ACTIVE MEMORY — project instructions + core memory + recall search
        {
            let mut mem_str = String::new();

            // Memory mode prompt (guides agent on how to use memory in this mode)
            let mode = self.brain.memory_mode();
            let mode_prompt = crate::prompt::memory_mode_prompt(&mode);
            mem_str.push_str(mode_prompt);
            mem_str.push_str("\n\n");

            // In Stateless mode, skip all project instructions and memory injection
            if mode != crate::config::MemoryMode::Stateless {
            // Layered project instructions (agverse.md):
            //   1. Global:   ~/.agverse/agverse.md
            //   2. Project:  {cwd}/agverse.md  (or AGENTS.md)
            //   3. Local:    {cwd}/agverse.local.md  (gitignore, personal)
            //   4. Rules:    {cwd}/.agverse/rules/*.md  (modular, path-scoped later)
            let cwd = self.working_dir.clone().or_else(|| {
                std::env::current_dir()
                    .ok()
                    .and_then(|p| p.to_str().map(|s| s.to_string()))
            });
            let mut instructions = Vec::new();

            // 1. Global
            let global_path = crate::paths::get_global_agverse_md_path();
            if let Ok(content) = std::fs::read_to_string(&global_path) {
                instructions.push(("Global Project".to_string(), content));
            }

            let mut global_local_path = global_path.clone();
            global_local_path.set_file_name("agverse.local.md");
            if let Ok(content) = std::fs::read_to_string(&global_local_path) {
                instructions.push(("Global User Preferences".to_string(), content));
            }

            // Extract recent conversation text to match path-scoped rules
            let conversation_text = self.context.messages().iter().rev().take(5)
                .filter_map(|m| m.content.as_deref())
                .collect::<Vec<_>>()
                .join("\n")
                .to_lowercase();

            // 2. Project root
            if let Some(ref dir) = cwd {
                for name in &["agverse.md", "AGENTS.md"] {
                    let path = std::path::Path::new(dir).join(name);
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        instructions.push((format!("Project ({name})"), content));
                        break;
                    }
                }

                // 3. Local (personal, gitignored)
                let local_path = std::path::Path::new(dir).join("agverse.local.md");
                if let Ok(content) = std::fs::read_to_string(&local_path) {
                    instructions.push(("Project Local".to_string(), content));
                }

                // 4. Rules directory (.agverse/rules/*.md) - Path Scoped
                let rules_dir = std::path::Path::new(dir).join(".agverse/rules");
                if rules_dir.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&rules_dir) {
                        let mut rule_files: Vec<_> = entries
                            .filter_map(|e| e.ok())
                            .filter(|e| {
                                e.path().extension().is_some_and(|ext| ext == "md")
                            })
                            .collect();
                        rule_files.sort_by_key(|e| e.path());
                        for entry in rule_files {
                            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                                let path = entry.path();
                                let name = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("rule");

                                let name_lower = name.to_lowercase();
                                if name_lower == "global" || name_lower == "default" || conversation_text.contains(&name_lower) {
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
                mem_str.push_str(&format!("Project Instructions:\n{}\n\n", parts.join("\n\n")));
            }

            } // end non-stateless block

            if !mem_str.is_empty() {
                self.context.set_active_memory(&mem_str);
            }
        }

        // Segment 6: LOADED SKILLS — catalog + active skill content
        if let Some(ref sm) = self.brain.skill_manager {
            let mgr = sm.lock();
            let catalog = mgr.build_catalog();
            let active = mgr.build_active_context();
            let mut skills_str = String::new();
            if !catalog.is_empty() {
                skills_str.push_str(&catalog);
            }
            if !active.is_empty() {
                if !skills_str.is_empty() {
                    skills_str.push_str("\n\n");
                }
                skills_str.push_str(&active);
            }
            if !skills_str.is_empty() {
                self.context.set_loaded_skills(&skills_str);
            }
        }

        // Segment 7: EXECUTION PLAN — inject pinned goal (if any) + current todo list
        let todo_str = self.brain.todo_list.lock().to_context_string();
        let plan_str = if let Some(ref g) = self.goal {
            if self.goal_completed {
                todo_str
            } else {
                let mut s = format!(
                    "## PRIMARY GOAL (pinned)\n{g}\n\nYou MUST keep this goal in mind. \
                     Break it into subtasks, track progress, and drive toward completion. \
                     If the conversation drifts, remind the user of this goal.\n\n"
                );
                if !todo_str.is_empty() {
                    s.push_str(&todo_str);
                }
                s
            }
        } else {
            todo_str
        };
        if !plan_str.is_empty() {
            self.context.set_execution_plan(&plan_str);
        }
    }

    /// Register / unregister `skill.<name>.<script>` tools so they match the
    /// currently active skills. Called once per turn from refresh_context_segments.
    fn sync_skill_scripts(&mut self) {
        
        use crate::tools::script::SkillScriptTool;

        let sm = match self.brain.skill_manager.as_ref() {
            Some(sm) => sm,
            None => return,
        };

        let mgr = sm.lock();
        let active_scripts = mgr.get_active_scripts();

        // Build the set of tool names we *should* have registered.
        let expected: std::collections::HashSet<String> = active_scripts
            .iter()
            .map(|(skill_name, script)| {
                format!("skill.{}.{}", skill_name, script.name)
            })
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
        let to_remove: Vec<String> = current
            .difference(&expected)
            .cloned()
            .collect();

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