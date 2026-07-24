use std::sync::Arc;

use agent_core::{
    runtime::run::ScopedToolFactory,
    workflow::runtime::{
        RunScope, SqliteWorkflowStore, WorkflowApplyDraftTool, WorkflowCatalogTool,
        WorkflowPreviewTool, WorkflowPublishTool,
    },
};

use crate::state::CliState;

pub fn authoring_prompt(goal: &str) -> String {
    let request = if goal.trim().is_empty() {
        "The user opened workflow authoring without an initial goal. Ask for the desired outcome, \
         inputs, and important constraints before constructing the draft."
            .to_string()
    } else {
        format!("The user's workflow goal is:\n{}", goal.trim())
    };
    format!(
        "You are in workflow authoring mode. Design a reusable, explicit DAG for the durable \
         workflow runtime. Inspect workflow_catalog before choosing saved or inline agents. Reuse \
         suitable saved agents; define stateless inline agents when no saved agent fits. Express \
         dependencies with after and pass data through ValueExpr inputs. Create the complete graph \
         atomically with workflow_apply_draft. Do not publish or preview unless the user requested \
         it or confirms it.\n\n{request}"
    )
}

pub fn scoped_tool_factory(
    state: &CliState,
    session_id: Option<String>,
    workspace: String,
) -> ScopedToolFactory {
    let service = state.workflow_authoring.clone();
    let runtime = state.workflow_runtime.clone();
    let caller_permission = state.run_manager.brain().config.permissions.clone();
    Arc::new(move |registry, cancel_token, parent_run_id| {
        let available_tools = registry
            .list_names()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let scope = RunScope {
            session_id: session_id.clone().unwrap_or_else(|| parent_run_id.clone()),
            parent_run_id: parent_run_id.clone(),
            workspace: workspace.clone(),
            trigger: "workflow_authoring_preview".to_string(),
            ..Default::default()
        };
        registry.register(Box::new(WorkflowCatalogTool::new(
            service.clone(),
            available_tools,
        )));
        registry.register(Box::new(WorkflowApplyDraftTool::new(
            service.clone(),
            caller_permission.clone(),
            parent_run_id.clone(),
        )));
        registry.register(Box::new(WorkflowPreviewTool::<SqliteWorkflowStore>::new(
            service.clone(),
            runtime.clone(),
            scope,
            cancel_token,
            parent_run_id.clone(),
        )));
        registry.register(Box::new(WorkflowPublishTool::new(
            service.clone(),
            parent_run_id,
        )));
        // Authoring may need catalog inspection and interactive clarification
        // before a valid complete draft exists, so no single tool is forced on
        // the first model turn.
        None
    })
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
