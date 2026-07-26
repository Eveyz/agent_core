import type { WorkflowDef, WorkflowLibraryEntry, NodeType } from "../../features/workflow/types";
import PlusIcon from "lucide-react/dist/esm/icons/plus.mjs";
import BotIcon from "lucide-react/dist/esm/icons/bot.mjs";
import { useAppSelector } from "../../hooks/useAppDispatch";
import { useState } from "react";
import "./WorkflowSidebar.css";

interface WorkflowSidebarProps {
  workflows: WorkflowDef[];
  libraryEntries: WorkflowLibraryEntry[];
  activeWorkflowId: string | undefined;
  activeLibraryId?: string;
  creatingWorkflow?: boolean;
  onNewWorkflow: () => void;
  onSelectWorkflow: (wf: WorkflowDef) => void;
  onSelectLibrary: (entry: WorkflowLibraryEntry) => void;
  onPublishLegacy: (wf: WorkflowDef) => void;
}

const PALETTE: { type: NodeType; label: string }[] = [
  { type: "input", label: "Input" },
  { type: "transform", label: "Transform" },
  { type: "human_approval", label: "Approval" },
  { type: "output", label: "Output" },
];

export function WorkflowSidebar({
  workflows,
  libraryEntries,
  activeWorkflowId,
  activeLibraryId,
  creatingWorkflow = false,
  onNewWorkflow,
  onSelectWorkflow,
  onSelectLibrary,
  onPublishLegacy,
}: WorkflowSidebarProps) {
  const agents = useAppSelector((s) => s.agents.agents);
  const [scopeFilter, setScopeFilter] = useState<"all" | "project" | "user">("all");
  const scopedEntries = libraryEntries.filter(
    (entry) => scopeFilter === "all" || entry.scope.kind === scopeFilter,
  );
  const published = scopedEntries.filter((entry) => entry.lifecycle === "published");
  const drafts = scopedEntries.filter(
    (entry) => entry.lifecycle === "draft" || entry.draft_status !== "published",
  );

  const renderLibraryGroup = (title: string, entries: WorkflowLibraryEntry[]) => (
    <>
      <div className="workflow-sidebar-section-title">{title}</div>
      {entries.length === 0 ? (
        <div className="workflow-sidebar-empty">None</div>
      ) : entries.map((entry) => (
        <button
          type="button"
          key={entry.workflow_id}
          onClick={() => onSelectLibrary(entry)}
          className={`workflow-sidebar-item workflow-sidebar-library-item ${
            activeLibraryId === entry.workflow_id ? "active" : ""
          }`}
        >
          <span>{entry.name || "Untitled"}</span>
          <small>{entry.scope.kind === "user" ? "Personal" : "Project"}</small>
        </button>
      ))}
    </>
  );

  return (
    <div className="workflow-sidebar-container">
      
      {/* Workflows List */}
      <div className="workflow-sidebar-header">
        <button
          type="button"
          className="btn-primary workflow-sidebar-new-btn"
          onClick={onNewWorkflow}
          disabled={creatingWorkflow}
        >
          <PlusIcon size={14} /> {creatingWorkflow ? "Creating..." : "New Workflow"}
        </button>
      </div>
      
      <div className="workflow-sidebar-list">
        <div className="workflow-sidebar-scope-filter">
          {(["all", "project", "user"] as const).map((scope) => (
            <button
              type="button"
              key={scope}
              className={scopeFilter === scope ? "active" : ""}
              onClick={() => setScopeFilter(scope)}
            >
              {scope === "user" ? "Personal" : scope}
            </button>
          ))}
        </div>
        {renderLibraryGroup("Published", published)}
        {renderLibraryGroup("Drafts", drafts)}
        <div className="workflow-sidebar-section-title">Legacy</div>
        {workflows.map((wf) => (
          <button
            type="button"
            key={wf.id}
            onClick={() => onSelectWorkflow(wf)}
            className={`workflow-sidebar-item ${activeWorkflowId === wf.id ? "active" : ""}`}
          >
            <span>{wf.name || "Untitled"}</span>
            <small
              role="button"
              tabIndex={0}
              onClick={(event) => {
                event.stopPropagation();
                onPublishLegacy(wf);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.stopPropagation();
                  onPublishLegacy(wf);
                }
              }}
            >
              Publish for chat reuse
            </small>
          </button>
        ))}
        {workflows.length === 0 && <div className="workflow-sidebar-empty">None</div>}
      </div>

      {/* Nodes Palette */}
      <div className="workflow-sidebar-palette">
        <div className="workflow-sidebar-palette-title">Basic Nodes</div>
        <div className="workflow-sidebar-grid">
          {PALETTE.map((p) => (
            <div
              key={p.type}
              draggable
              onDragStart={(e) => {
                const data = JSON.stringify({ type: p.type });
                e.dataTransfer.setData("application/reactflow", data);
                e.dataTransfer.setData("text/plain", data);
                e.dataTransfer.effectAllowed = "move";
              }}
              className="workflow-sidebar-palette-node"
            >
              {p.label}
            </div>
          ))}
        </div>

        <div className="workflow-sidebar-palette-title mt-24">My Agents</div>
        <div className="workflow-sidebar-agent-list">
          {agents.length === 0 ? (
            <div className="workflow-sidebar-agent-empty">No agents available</div>
          ) : (
            agents.map((agent) => (
              <div
                key={agent.id}
                draggable
                onDragStart={(e) => {
                  const data = JSON.stringify({
                    type: "agent",
                    agentId: agent.id,
                    agentName: agent.name
                  });
                  e.dataTransfer.setData("application/reactflow", data);
                  e.dataTransfer.setData("text/plain", data);
                  e.dataTransfer.effectAllowed = "move";
                }}
                className="workflow-sidebar-agent-node"
              >
                <BotIcon size={14} color={agent.color || "var(--accent)"} />
                <span className="workflow-sidebar-agent-name">{agent.name}</span>
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
