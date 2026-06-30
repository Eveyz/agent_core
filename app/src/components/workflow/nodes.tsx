import { memo } from "react";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import BotIcon from "lucide-react/dist/esm/icons/bot.mjs";
import ArrowDownToLineIcon from "lucide-react/dist/esm/icons/arrow-down-to-line.mjs";
import ArrowUpFromLineIcon from "lucide-react/dist/esm/icons/arrow-up-from-line.mjs";
import ShuffleIcon from "lucide-react/dist/esm/icons/shuffle.mjs";
import CheckCircleIcon from "lucide-react/dist/esm/icons/check-circle.mjs";

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
  const isTerminal = d.nodeType === "input" || d.nodeType === "output";
  const statusColor =
    d.status === "running" ? "#f59e0b" :
    d.status === "completed" ? "#22c55e" :
    d.status === "failed" ? "#ef4444" :
    d.status === "skipped" ? "#64748b" : undefined;

  return (
    <div
      style={{
        background: "var(--bg-secondary, #1e1e2e)",
        border: `2px solid ${selected ? color : statusColor ?? "var(--border-color, #333)"}`,
        borderRadius: "10px",
        padding: "10px 14px",
        minWidth: "160px",
        boxShadow: statusColor ? `0 0 12px ${statusColor}55` : "0 2px 8px rgba(0,0,0,0.3)",
        position: "relative",
      }}
    >
      {!isTerminal && (
        <Handle type="target" position={Position.Left} style={{ background: color }} />
      )}
      <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
        <Icon size={16} color={color} />
        <div style={{ display: "flex", flexDirection: "column" }}>
          <span style={{ fontSize: "11px", color: "var(--text-muted, #888)", textTransform: "uppercase", letterSpacing: "0.5px" }}>
            {d.nodeType}
          </span>
          <span style={{ fontSize: "13px", fontWeight: 600, color: "var(--text-main, #e0e0e0)" }}>
            {d.label || d.agentName || "Untitled"}
          </span>
        </div>
      </div>
      {isTerminal && (
        <Handle type="source" position={Position.Right} style={{ background: color }} />
      )}
      {!isTerminal && (
        <Handle type="source" position={Position.Right} style={{ background: color }} />
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
