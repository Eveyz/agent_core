import { memo } from "react";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import BotIcon from "lucide-react/dist/esm/icons/bot.mjs";
import ArrowDownToLineIcon from "lucide-react/dist/esm/icons/arrow-down-to-line.mjs";
import ArrowUpFromLineIcon from "lucide-react/dist/esm/icons/arrow-up-from-line.mjs";
import ShuffleIcon from "lucide-react/dist/esm/icons/shuffle.mjs";
import CheckCircleIcon from "lucide-react/dist/esm/icons/check-circle.mjs";
import XCircleIcon from "lucide-react/dist/esm/icons/x-circle.mjs";
import LoaderIcon from "lucide-react/dist/esm/icons/loader-2.mjs";
import "./nodes.css";

export interface WorkflowNodeData {
  label: string;
  nodeType: string;
  agentId?: string;
  agentName?: string;
  status?: string;
  [key: string]: unknown;
}

const ICONS: Record<string, typeof BotIcon> = {
  input: ArrowDownToLineIcon,
  output: ArrowUpFromLineIcon,
  agent: BotIcon,
  transform: ShuffleIcon,
  human_approval: CheckCircleIcon,
};

const COLORS: Record<string, string> = {
  input: "#22c55e",
  output: "#6366f1",
  agent: "#3b82f6",
  transform: "#f59e0b",
  human_approval: "#ec4899",
};

function NodeShell({ data, selected }: NodeProps) {
  const d = data as WorkflowNodeData;
  const Icon = ICONS[d.nodeType] ?? BotIcon;
  const color = COLORS[d.nodeType] ?? "#64748b";
  const isInput = d.nodeType === "input";
  const isOutput = d.nodeType === "output";
  
  const statusClass = 
    d.status === "running" ? "node-running" :
    d.status === "completed" ? "node-completed" :
    d.status === "failed" ? "node-error" :
    d.status === "skipped" ? "node-skipped" : 
    d.status === "cancelling" ? "node-cancelling" :
    d.status === "cancelled" ? "node-cancelled" : "";

  return (
    <div
      className={`workflow-node-shell ${selected ? 'selected' : ''} ${statusClass}`}
      style={{
        '--node-color': color,
      } as React.CSSProperties}
    >
      {!isInput && (
        <Handle type="target" position={Position.Left} className="node-handle" />
      )}
      
      <div className="node-content">
        <div className="node-icon-wrapper">
          <Icon size={16} />
        </div>
        
        <div className="node-info">
          <span className="node-type-label">
            {d.nodeType}
          </span>
          <span className="node-name">
            {d.label || d.agentName || "Untitled"}
          </span>
        </div>
        
        {/* Status indicators */}
        {d.status === "running" && <LoaderIcon size={14} className="status-icon spin" color="#f59e0b" />}
        {d.status === "completed" && <CheckCircleIcon size={14} className="status-icon" color="#22c55e" />}
        {d.status === "failed" && <XCircleIcon size={14} className="status-icon" color="#ef4444" />}
      </div>

      {!isOutput && (
        <Handle type="source" position={Position.Right} className="node-handle" />
      )}
    </div>
  );
}

export const AgentNode = memo(NodeShell);
export const InputNode = memo(NodeShell);
export const OutputNode = memo(NodeShell);
export const TransformNode = memo(NodeShell);
export const ApprovalNode = memo(NodeShell);

export const nodeTypes = {
  agent: AgentNode,
  input: InputNode,
  output: OutputNode,
  transform: TransformNode,
  human_approval: ApprovalNode,
};
