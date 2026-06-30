import { useEffect } from "react";
import type { AgentDef } from "../../../features/agents/types";
import { useAppDispatch, useAppSelector } from "../../../hooks/useAppDispatch";
import { fetchAgentHistory } from "../../../features/agents/agentSlice";
import CheckCircleIcon from "lucide-react/dist/esm/icons/check-circle-2.mjs";
import XCircleIcon from "lucide-react/dist/esm/icons/x-circle.mjs";
import ClockIcon from "lucide-react/dist/esm/icons/clock.mjs";

interface AgentHistoryTabProps {
  agent: AgentDef;
}

export function AgentHistoryTab({ agent }: AgentHistoryTabProps) {
  const dispatch = useAppDispatch();
  const history = useAppSelector((s) => s.agents.history);

  useEffect(() => {
    dispatch(fetchAgentHistory({ agentId: agent.id, limit: 50 }));
  }, [agent.id, dispatch]);

  return (
    <div className="agent-history-tab">
      <div style={{ marginBottom: "24px" }}>
        <h3 style={{ fontSize: "16px", fontWeight: 600, margin: "0 0 8px 0", color: "var(--text-main)" }}>Execution History</h3>
        <p style={{ fontSize: "13px", color: "var(--text-muted)", margin: 0 }}>
          Timeline of this agent's past runs and task executions.
        </p>
      </div>

      <div style={{ position: "relative", paddingLeft: "16px" }}>
        {/* Timeline Line */}
        <div style={{ 
          position: "absolute", 
          left: "27px", 
          top: "10px", 
          bottom: "10px", 
          width: "2px", 
          background: "rgba(255, 255, 255, 0.05)",
          zIndex: 0
        }} />

        {history.length === 0 ? (
          <div style={{ padding: "40px 20px", color: "var(--text-muted)", fontSize: "14px" }}>
            No execution history found.
          </div>
        ) : (
          history.map((h, idx) => (
            <div key={h.id} style={{ 
              position: "relative", 
              zIndex: 1, 
              display: "flex", 
              gap: "24px", 
              marginBottom: idx === history.length - 1 ? 0 : "32px" 
            }}>
              {/* Status Icon */}
              <div style={{ 
                width: "24px", 
                height: "24px", 
                borderRadius: "50%", 
                background: h.success ? "rgba(34, 197, 94, 0.2)" : "rgba(239, 68, 68, 0.2)",
                color: h.success ? "var(--success, #22c55e)" : "var(--danger, #ef4444)",
                display: "flex", 
                alignItems: "center", 
                justifyContent: "center",
                flexShrink: 0,
                marginTop: "2px"
              }}>
                {h.success ? <CheckCircleIcon size={14} /> : <XCircleIcon size={14} />}
              </div>

              {/* Content Card */}
              <div style={{ 
                flex: 1, 
                background: "rgba(255, 255, 255, 0.02)", 
                border: "1px solid rgba(255, 255, 255, 0.06)", 
                borderRadius: "12px", 
                padding: "20px" 
              }}>
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: "12px" }}>
                  <div>
                    <div style={{ fontSize: "14px", fontWeight: 600, color: "var(--text-main)", marginBottom: "4px" }}>
                      Trigger: {h.trigger}
                    </div>
                    <div style={{ fontSize: "12px", color: "var(--text-muted)" }}>
                      {new Date(h.created_at).toLocaleString()}
                    </div>
                  </div>
                  <div style={{ display: "flex", alignItems: "center", gap: "12px" }}>
                    <div style={{ display: "flex", alignItems: "center", gap: "4px", fontSize: "12px", color: "var(--text-muted)" }}>
                      <ClockIcon size={12} /> {h.process_time_ms}ms
                    </div>
                    <div style={{ fontSize: "12px", color: "var(--text-muted)", background: "rgba(255,255,255,0.05)", padding: "2px 8px", borderRadius: "4px" }}>
                      Iter: {h.iterations_used}
                    </div>
                  </div>
                </div>

                <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
                  <div>
                    <div style={{ fontSize: "11px", textTransform: "uppercase", fontWeight: 600, color: "var(--text-muted)", marginBottom: "6px" }}>Input Task</div>
                    <div style={{ fontSize: "13px", color: "var(--text-main)", background: "rgba(0,0,0,0.2)", padding: "12px", borderRadius: "8px", border: "1px solid rgba(255,255,255,0.03)" }}>
                      {h.input}
                    </div>
                  </div>
                  
                  {h.output && (
                    <div>
                      <div style={{ fontSize: "11px", textTransform: "uppercase", fontWeight: 600, color: "var(--text-muted)", marginBottom: "6px" }}>Output Result</div>
                      <div style={{ 
                        fontSize: "13px", 
                        color: h.success ? "var(--text-main)" : "var(--danger)", 
                        background: "rgba(0,0,0,0.2)", 
                        padding: "12px", 
                        borderRadius: "8px", 
                        border: "1px solid rgba(255,255,255,0.03)",
                        whiteSpace: "pre-wrap",
                        maxHeight: "150px",
                        overflowY: "auto"
                      }}>
                        {h.output}
                      </div>
                    </div>
                  )}
                </div>
              </div>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
