use agent_core::{
    runtime::run::ScopedToolFactory,
    workflow::runtime::{RunScope, workflow_authoring_tool_factory},
};

use crate::state::CliState;

pub fn authoring_prompt(goal: &str) -> String {
    let trimmed = goal.trim();
    let (intent, goal) = if let Some(goal) = trimmed.strip_prefix("save ") {
        ("save", goal.trim())
    } else if trimmed == "save" {
        ("save", "")
    } else if let Some(goal) = trimmed.strip_prefix("publish ") {
        ("publish", goal.trim())
    } else if trimmed == "publish" {
        ("publish", "")
    } else {
        ("temporary", trimmed)
    };
    let request = if goal.is_empty() {
        "The user opened workflow authoring without an initial goal. Ask for the desired outcome, \
         inputs, and important constraints before constructing the draft."
            .to_string()
    } else {
        format!("The user's workflow goal is:\n{goal}")
    };
    let lifecycle = match intent {
        "save" => {
            "After applying the complete draft, save it with workflow_save_draft. Do not publish it."
        }
        "publish" => {
            "After applying the complete draft, save it with workflow_save_draft and then publish the saved draft with workflow_publish."
        }
        _ => {
            "Keep the draft temporary. Do not publish or save unless the user explicitly changes their request."
        }
    };
    format!(
        "You are in workflow authoring mode. Design a reusable, explicit DAG for the durable \
         workflow runtime. Inspect workflow_catalog before choosing saved or inline agents. Reuse \
         suitable saved agents; define stateless inline agents when no saved agent fits. Express \
         dependencies with after and pass data through ValueExpr inputs. Create the complete graph \
         atomically with workflow_apply_draft. {lifecycle} Do not preview unless the user requested \
         it or confirms it.\n\n{request}"
    )
}

pub fn scoped_tool_factory(
    state: &CliState,
    session_id: Option<String>,
    workspace: String,
) -> ScopedToolFactory {
    let project_id = workspace.clone();
    workflow_authoring_tool_factory(
        state.workflow_authoring.clone(),
        state.workflow_runtime.clone(),
        state.run_manager.permissions(),
        RunScope {
            session_id: session_id.unwrap_or_default(),
            workspace,
            project_id,
            trigger: "workflow_authoring_preview".to_string(),
            ..Default::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::authoring_prompt;

    #[test]
    fn authoring_prompt_requires_catalog_and_atomic_draft() {
        let prompt = authoring_prompt("Research, write, then review");
        assert!(prompt.contains("workflow_catalog"));
        assert!(prompt.contains("workflow_apply_draft"));
        assert!(prompt.contains("Research, write, then review"));
        assert!(prompt.contains("Do not publish"));
    }
}
