import XIcon from "lucide-react/dist/esm/icons/x.mjs";
import ActivityIcon from "lucide-react/dist/esm/icons/activity.mjs";
import "./NodeInspectorDrawer.css";
import type { WorkflowRunNodeResult } from "../../features/workflow/types";
import { NodeChatStream } from "./NodeChatStream";
import { ErrorBlock } from "./ErrorBlock";

interface NodeInspectorDrawerProps {
  nodeId: string;
  nodeLabel: string;
  result: WorkflowRunNodeResult | undefined;
  onClose: () => void;
}

const getStatusColor = (status: string) => {
  switch (status) {
    case "completed": return "var(--success)";
    case "failed": return "var(--danger)";
    case "running": return "var(--warning)";
    default: return "var(--text-muted)";
  }
};

export function NodeInspectorDrawer({
  nodeId,
  nodeLabel,
  result,
  onClose,
}: NodeInspectorDrawerProps) {
  if (!nodeId) return null;

  return (
    <div className="node-inspector-overlay">
      <div className="node-inspector-drawer">
        <div className="node-inspector-header">
          <div className="node-inspector-title">
            <ActivityIcon size={16} color="var(--accent)" />
            {nodeLabel}
            {result?.status && (
              <span className="node-inspector-status" style={{ color: getStatusColor(result.status) }}>
                {result.status}
              </span>
            )}
          </div>
          <button className="node-inspector-close-btn" onClick={onClose}>
            <XIcon size={18} />
          </button>
        </div>

        <div className="node-inspector-content">
          {!result ? (
            <div className="node-inspector-empty">Waiting for execution data...</div>
          ) : (
            <>
              {result.error && (
                <div className="node-inspector-section">
                  <ErrorBlock error={result.error} />
                </div>
              )}

              {result.live_logs && result.live_logs.length > 0 && (
                <div className="node-inspector-section">
                  <div className="node-inspector-section-title">Execution Stream</div>
                  <NodeChatStream logs={result.live_logs} />
                </div>
              )}

              {result.output && (
                <div className="node-inspector-section">
                  <div className="node-inspector-section-title">Final Output</div>
                  <div className="node-inspector-output-box">
                    {typeof result.output === "string" ? result.output : JSON.stringify(result.output, null, 2)}
                  </div>
                </div>
              )}

              {result.status === "completed" && (
                <div className="node-inspector-section" style={{ marginTop: "12px" }}>
                  <div className="node-inspector-section-title">Metrics</div>
                  <div className="node-inspector-metrics-grid">
                    <div className="node-inspector-metric-card">
                      <div className="node-inspector-metric-label">Duration</div>
                      <div className="node-inspector-metric-value">
                        {result.latency_ms ? `${result.latency_ms}ms` : "---"}
                      </div>
                    </div>
                    <div className="node-inspector-metric-card">
                      <div className="node-inspector-metric-label">Tokens Used</div>
                      <div className="node-inspector-metric-value">
                        In: {result.token_input} | Out: {result.token_output}
                      </div>
                    </div>
                    <div className="node-inspector-metric-card">
                      <div className="node-inspector-metric-label">Cost</div>
                      <div className="node-inspector-metric-value">
                        ${(result.cost_usd ?? 0).toFixed(4)}
                      </div>
                    </div>
                  </div>
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
