//! `ask_user` — ask the human to clarify before planning or acting.
//!
//! The tool is registered so the model sees its schema. Actual blocking
//! wait happens in [`crate::runtime::tool_orchestrator::ToolOrchestrator`],
//! which intercepts `ask_user` calls, emits `InputRequested`, and awaits
//! the per-Run [`crate::runtime::input::InputResolver`].

use crate::tools::{Tool, ToolRegistry};
use anyhow::Result;
use serde_json::Value;

pub fn register_ask_user_tool(registry: &mut ToolRegistry) {
    registry.register(Box::new(AskUserTool));
}

pub struct AskUserTool;

#[async_trait::async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the human to clarify ambiguous requirements via multiple-choice questions \
         BEFORE planning or executing. Use when goals, scope, success criteria, or \
         choices are unclear (especially under /goal). Blocks until the human answers. \
         Do not call mutating tools in the same turn — clarify first, \
         then act on the next turn with the answers."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Optional short header shown above the questions (e.g. 'Clarify goal')."
                },
                "questions": {
                    "type": "array",
                    "description": "1–8 focused multiple-choice questions.",
                    "minItems": 1,
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Stable id for this question (used in the answer map)."
                            },
                            "prompt": {
                                "type": "string",
                                "description": "The question text shown to the human."
                            },
                            "allow_multiple": {
                                "type": "boolean",
                                "description": "If true, human may select multiple options. Default false (single-select)."
                            },
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 12,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "id": { "type": "string" },
                                        "label": { "type": "string" }
                                    },
                                    "required": ["id", "label"]
                                }
                            }
                        },
                        "required": ["prompt", "options"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        // Orchestrator normally intercepts ask_user before execute().
        // Fallback path: validate args and return a structured error so the
        // model can retry rather than hanging forever without a UI.
        match crate::runtime::input::parse_ask_user_args(&args) {
            Ok(_) => Ok(
                "ask_user was invoked without a live input channel. \
                 Re-issue ask_user on the next turn so the UI can collect answers."
                    .to_string(),
            ),
            Err(e) => Err(anyhow::anyhow!(e)),
        }
    }

    fn execution_mode(&self) -> Option<crate::types::ToolExecutionMode> {
        // Always exclusive — never parallelize with other tools.
        Some(crate::types::ToolExecutionMode::Sequential)
    }
}
