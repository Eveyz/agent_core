import type { WorkflowDef, NodeType } from "../../features/workflow/types";
import PlusIcon from "lucide-react/dist/esm/icons/plus.mjs";
import BotIcon from "lucide-react/dist/esm/icons/bot.mjs";
import { useAppSelector } from "../../hooks/useAppDispatch";
import "./WorkflowSidebar.css";

interface WorkflowSidebarProps {
  workflows: WorkflowDef[];
  activeWorkflowId: string | undefined;
  creatingWorkflow?: boolean;
  onNewWorkflow: () => void;
  onSelectWorkflow: (wf: WorkflowDef) => void;
}

const PALETTE: { type: NodeType; label: string }[] = [
  { type: "input", label: "Input" },
  { type: "transform", label: "Transform" },
  { type: "human_approval", label: "Approval" },
  { type: "output", label: "Output" },
];

export function WorkflowSidebar({
  workflows,
  activeWorkflowId,
  creatingWorkflow = false,
  onNewWorkflow,
  onSelectWorkflow,
}: WorkflowSidebarProps) {
  const agents = useAppSelector((s) => s.agents.agents);

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
        <div className="workflow-sidebar-section-title">Saved Workflows</div>
        {workflows.map((wf) => (
          <div
            key={wf.id}
            onClick={() => onSelectWorkflow(wf)}
            className={`workflow-sidebar-item ${activeWorkflowId === wf.id ? "active" : ""}`}
          >
            {wf.name || "Untitled"}
          </div>
        ))}
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
