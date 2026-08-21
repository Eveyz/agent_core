//! Structured user intent. Slash strings are parsed here, not inside `Run`.

use crate::todo::{
    ResumeTarget, is_bare_continue, is_explicit_plan_resume, is_plan_clear_cmd, is_plan_park_cmd,
    is_plan_resume_cmd, looks_like_detour, parse_resume_target,
};

/// What the user asked this Run to do. `Run` matches on this enum; it never
/// re-parses slash prefixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserIntent {
    Prompt { text: String },
    Goal { text: String },
    GoalClear,
    Learn { content: Option<String> },
    PlanPark,
    PlanClear,
    PlanResume { raw: String },
    BareContinue { text: String },
    Detour { text: String },
}

/// Parse a raw composer string. Callers (Tauri / CLI / gateway) use this
/// before `CreateRunRequest`; `Run` never re-parses slash prefixes.
pub fn parse_user_intent(text: &str) -> UserIntent {
    UserIntent::parse(text)
}

impl UserIntent {
    /// Parse a raw composer string into a structured intent.
    pub fn parse(text: &str) -> Self {
        let trimmed = text.trim();
        let is_goal_clear = trimmed == "/goal clear"
            || trimmed == "/goal stop"
            || trimmed == "/goal cancel"
            || trimmed == "/goal off";
        if is_goal_clear {
            return Self::GoalClear;
        }
        if let Some(goal) = text
            .strip_prefix("/goal ")
            .map(str::trim)
            .map(str::to_string)
            .filter(|s| !s.is_empty())
        {
            return Self::Goal { text: goal };
        }
        if trimmed == "/learn" {
            return Self::Learn { content: None };
        }
        if let Some(content) = trimmed.strip_prefix("/learn ") {
            let content = content.trim();
            return Self::Learn {
                content: (!content.is_empty()).then(|| content.to_string()),
            };
        }
        if is_plan_park_cmd(trimmed) {
            return Self::PlanPark;
        }
        if is_plan_clear_cmd(trimmed) {
            return Self::PlanClear;
        }
        if is_plan_resume_cmd(trimmed) || is_explicit_plan_resume(trimmed) {
            return Self::PlanResume {
                raw: trimmed.to_string(),
            };
        }
        if is_bare_continue(trimmed) {
            return Self::BareContinue {
                text: trimmed.to_string(),
            };
        }
        if looks_like_detour(trimmed) {
            return Self::Detour {
                text: text.to_string(),
            };
        }
        Self::Prompt {
            text: text.to_string(),
        }
    }

    /// Text stored on the user conversation message (prefixes stripped).
    pub fn display_text(&self) -> String {
        match self {
            Self::Prompt { text } | Self::Detour { text } | Self::BareContinue { text } => {
                text.clone()
            }
            Self::Goal { text } => text.clone(),
            Self::GoalClear => "/goal clear".to_string(),
            Self::Learn { .. } => "/learn".to_string(),
            Self::PlanPark => "/plan park".to_string(),
            Self::PlanClear => "/plan clear".to_string(),
            Self::PlanResume { raw } => raw.clone(),
        }
    }

    /// Text used for skill triggers and memory storage (original user wording).
    pub fn trigger_text(&self) -> String {
        match self {
            Self::Prompt { text } | Self::Detour { text } | Self::BareContinue { text } => {
                text.clone()
            }
            Self::Goal { text } => format!("/goal {text}"),
            Self::GoalClear => "/goal clear".to_string(),
            Self::Learn { content } => match content {
                Some(c) if !c.is_empty() => format!("/learn {c}"),
                _ => "/learn".to_string(),
            },
            Self::PlanPark => "/plan park".to_string(),
            Self::PlanClear => "/plan clear".to_string(),
            Self::PlanResume { raw } => raw.clone(),
        }
    }

    pub fn resume_target(&self) -> Option<ResumeTarget> {
        match self {
            Self::PlanResume { raw } => Some(parse_resume_target(raw)),
            _ => None,
        }
    }

    pub fn should_park_other_prompt_plan(&self) -> bool {
        matches!(self, Self::Prompt { text } if !text.trim().is_empty())
    }
}

/// Fallback skill directories for `/learn` (no hardcoded user home).
pub fn learn_skill_fallback_dirs() -> String {
    let home = dirs::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~".to_string());
    format!(
        "  * Workspace root: `.agents/skills/<skill_name>/SKILL.md` (applies to all agents in this project)\n\
         * Antigravity/Gemini Global: `{home}/.gemini/config/skills/<skill_name>/SKILL.md`\n\
         * Claude Code Global: `{home}/.claudecode/skills/<skill_name>/SKILL.md`\n\
         * OpenCode / Codex Global customization folders."
    )
}

pub fn learn_system_prompt(content: Option<&str>) -> String {
    let dirs = learn_skill_fallback_dirs();
    let skill_block = format!(
        "2. Custom Skill: If it is a complex workflow, reusable procedure, or specialized agent task, create a Custom Skill. To create the skill:\n\
                    - Check if there is an available meta skill called `skill-creator` (by Anthropic). If it is available, use the `skill-creator` skill to build the skill.\n\n\
                    - If the `skill-creator` skill is not available, fallback to writing a `SKILL.md` file (starting with YAML frontmatter containing 'name' and 'description') under one of the customization directories:\n\
{dirs}"
    );
    match content {
        None | Some("") => format!(
            "System instruction: The user wants you to learn from this session. Please analyze the conversation history, identify any critical lessons, coding standards, user preferences, or workflows established. \
                 \n\n\
                 You have two ways to save this learning based on its complexity:\n\
                 1. Core Memory: If it is a user trait, simple preference, or rule, call the `core_memory_append` tool (with block_id: 'human') to append it.\n\
                 {skill_block}\n\n\
                 Choose the most appropriate approach, call the corresponding tools to save it, and respond explaining what you have learned and saved."
        ),
        Some(learn_content) => format!(
            "System instruction: The user wants you to save the following specific learning/rule/workflow:\n\
                     \"{learn_content}\"\n\n\
                     You have two ways to save this based on its complexity:\n\
                     1. Core Memory: If it is a user preference, habit, or simple rule, call the `core_memory_append` tool (with block_id: 'human') to append it.\n\
                     {skill_block}\n\n\
                     Choose the most appropriate approach, call the corresponding tools to save it, and respond to confirm what you have saved."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_prompt() {
        let intent = UserIntent::parse("fix the tests");
        assert_eq!(
            intent,
            UserIntent::Prompt {
                text: "fix the tests".into()
            }
        );
        assert_eq!(intent.display_text(), "fix the tests");
    }

    #[test]
    fn parse_goal_strips_prefix() {
        let intent = UserIntent::parse("/goal ship the maps card");
        assert_eq!(
            intent,
            UserIntent::Goal {
                text: "ship the maps card".into()
            }
        );
        assert_eq!(intent.display_text(), "ship the maps card");
        assert_eq!(intent.trigger_text(), "/goal ship the maps card");
    }

    #[test]
    fn parse_goal_clear_aliases() {
        for raw in ["/goal clear", "/goal stop", "/goal cancel", "/goal off"] {
            assert_eq!(UserIntent::parse(raw), UserIntent::GoalClear, "{raw}");
        }
        assert_eq!(UserIntent::GoalClear.display_text(), "/goal clear");
    }

    #[test]
    fn parse_learn_with_and_without_content() {
        assert_eq!(
            UserIntent::parse("/learn"),
            UserIntent::Learn { content: None }
        );
        assert_eq!(
            UserIntent::parse("/learn prefer tabs"),
            UserIntent::Learn {
                content: Some("prefer tabs".into())
            }
        );
        assert_eq!(UserIntent::parse("/learn").display_text(), "/learn");
    }

    #[test]
    fn parse_plan_lifecycle() {
        assert_eq!(UserIntent::parse("/plan park"), UserIntent::PlanPark);
        assert_eq!(UserIntent::parse("/plan pause"), UserIntent::PlanPark);
        assert_eq!(UserIntent::parse("/plan clear"), UserIntent::PlanClear);
        assert!(matches!(
            UserIntent::parse("/plan resume abc"),
            UserIntent::PlanResume { .. }
        ));
        assert_eq!(
            UserIntent::parse("/plan resume").resume_target(),
            Some(ResumeTarget::Unspecified)
        );
    }

    #[test]
    fn parse_bare_continue_and_detour() {
        assert!(matches!(
            UserIntent::parse("继续"),
            UserIntent::BareContinue { .. }
        ));
        assert!(matches!(
            UserIntent::parse("btw what time is it"),
            UserIntent::Detour { .. }
        ));
        assert!(UserIntent::parse("fix the tests").should_park_other_prompt_plan());
        assert!(!UserIntent::parse("/goal x").should_park_other_prompt_plan());
    }

    #[test]
    fn learn_prompt_uses_resolved_home_for_skill_dirs() {
        let prompt = learn_system_prompt(None);
        assert!(prompt.contains(".gemini/config/skills"));
        assert!(prompt.contains("core_memory_append"));
        if let Some(home) = dirs::home_dir() {
            assert!(prompt.contains(&format!("{}/.gemini/config/skills", home.display())));
        }
    }
}
