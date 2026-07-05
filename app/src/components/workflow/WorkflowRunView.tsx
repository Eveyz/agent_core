import { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAppSelector } from "../../hooks/useAppDispatch";
import type { WorkflowRun, WorkflowRunNodeResult } from "../../features/workflow/types";
import XIcon from "lucide-react/dist/esm/icons/x.mjs";
import CheckCircleIcon from "lucide-react/dist/esm/icons/check-circle.mjs";
import XCircleIcon from "lucide-react/dist/esm/icons/x-circle.mjs";
import ClockIcon from "lucide-react/dist/esm/icons/clock.mjs";
import LoaderIcon from "lucide-react/dist/esm/icons/loader.mjs";

const RUN_HISTORY_LIMIT = 20;
const POLL_INTERVAL_MS = 1500;

const STATUS_META: Record<string, { color: string; icon: typeof CheckCircleIcon }> = {
  completed: { color: "var(--success)", icon: CheckCircleIcon },
  failed: { color: "var(--danger)", icon: XCircleIcon },
  running: { color: "var(--warning)", icon: LoaderIcon },
  skipped: { color: "var(--slate-500)", icon: XCircleIcon },
  pending: { color: "var(--slate-500)", icon: ClockIcon },
};

/**
 * WorkflowRunView — displays run history + per-node results with
 * token/cost/latency observability.
 */
export function WorkflowRunView({
  workflowId,
  onClose,
}: {
  workflowId: string;
  onClose: () => void;
}) {
  const lastRunResult = useAppSelector((s) => s.workflow.lastRunResult);
  const running = useAppSelector((s) => s.workflow.isExecuting);
  const [runs, setRuns] = useState<WorkflowRun[]>([]);
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);
  const [nodeResults, setNodeResults] = useState<WorkflowRunNodeResult[]>([]);
  const [loadingResults, setLoadingResults] = useState(false);

  // Load run history.
  const loadRuns = useCallback(async () => {
    try {
      const data = await invoke<WorkflowRun[]>("list_workflow_runs", { workflowId, limit: RUN_HISTORY_LIMIT });
      setRuns(data);
      if (data.length > 0 && !selectedRunId) setSelectedRunId(data[0].id);
    } catch (e) {
      console.error(e);
    }
  }, [workflowId, selectedRunId]);

  useEffect(() => {
    loadRuns();
  }, [loadRuns, lastRunResult, running]);

  // Load node results for the selected run.
  useEffect(() => {
    if (!selectedRunId) {
      setNodeResults([]);
      return;
    }

    let isMounted = true;
    setLoadingResults(true);

    const fetchResults = () => {
      invoke<WorkflowRunNodeResult[]>("get_workflow_run_results", { runId: selectedRunId })
        .then((results) => { if (isMounted) setNodeResults(results); })
        .catch((e) => { if (isMounted) console.error(e); })
        .finally(() => { if (isMounted) setLoadingResults(false); });
    };

    fetchResults();

    if (running) {
      const interval = setInterval(fetchResults, POLL_INTERVAL_MS);
      return () => {
        isMounted = false;
        clearInterval(interval);
      };
    }

    return () => { isMounted = false; };
  }, [selectedRunId, running]);

  const totalTokens = nodeResults.reduce((sum, r) => sum + r.token_input + r.token_output, 0);
  const totalLatency = nodeResults.reduce((sum, r) => sum + r.latency_ms, 0);

  return (
    <div style={{ position: "fixed", inset: 0, background: "var(--shadow-xl)", zIndex: 9998, display: "flex", alignItems: "center", justifyContent: "center" }} onClick={onClose}>
      <div style={{ width: "760px", maxHeight: "80vh", background: "var(--bg-secondary)", borderRadius: "12px", display: "flex", flexDirection: "column", overflow: "hidden" }} onClick={(e) => e.stopPropagation()}>
        {/* Header */}
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "14px 18px", borderBottom: "1px solid var(--border-color)" }}>
          <h3 style={{ margin: 0, fontSize: "15px", fontWeight: 600 }}>Workflow Run Results</h3>
          <button className="icon-btn" onClick={onClose}><XIcon size={16} /></button>
        </div>

        <div style={{ display: "flex", flex: 1, overflow: "hidden" }}>
          {/* Run list */}
          <div style={{ width: "220px", borderRight: "1px solid var(--border-color)", overflowY: "auto", padding: "6px" }}>
            {runs.length === 0 && (
              <div style={{ fontSize: "12px", color: "var(--text-muted)", padding: "12px", textAlign: "center" }}>No runs yet</div>
            )}
            {runs.map((run) => {
              const meta = STATUS_META[run.status] ?? STATUS_META.pending;
              const Icon = meta.icon;
              return (
                <div
                  key={run.id}
                  onClick={() => setSelectedRunId(run.id)}
                  style={{
                    padding: "8px 10px",
                    borderRadius: "6px",
                    cursor: "pointer",
                    background: selectedRunId === run.id ? "var(--accent-subtle)" : "transparent",
                    marginBottom: "2px",
                  }}
                >
                  <div style={{ display: "flex", alignItems: "center", gap: "6px" }}>
                    <Icon size={12} color={meta.color} />
                    <span style={{ fontSize: "12px", fontWeight: 500 }}>{run.status}</span>
                  </div>
                  <div style={{ fontSize: "10px", color: "var(--text-muted)", marginTop: "2px" }}>
                    {new Date(run.started_at).toLocaleString()}
                  </div>
                </div>
              );
            })}
          </div>

          {/* Node results detail */}
          <div style={{ flex: 1, overflowY: "auto", padding: "14px 18px" }}>
            {lastRunResult && (
              <div style={{ marginBottom: "14px", padding: "10px 12px", background: "var(--bg-app)", borderRadius: "8px" }}>
                <div style={{ fontSize: "12px", fontWeight: 600, marginBottom: "6px" }}>Last Run Summary</div>
                <div style={{ display: "flex", gap: "16px", fontSize: "12px" }}>
                  <span>Status: <strong style={{ color: STATUS_META[lastRunResult.status]?.color }}>{lastRunResult.status}</strong></span>
                  <span>Tokens in: <strong>{lastRunResult.total_token_input}</strong></span>
                  <span>Tokens out: <strong>{lastRunResult.total_token_output}</strong></span>
                </div>
                {lastRunResult.error && (
                  <div style={{ fontSize: "11px", color: "var(--danger)", marginTop: "6px" }}>{lastRunResult.error}</div>
                )}
              </div>
            )}

            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "10px" }}>
              <span style={{ fontSize: "12px", fontWeight: 600 }}>Node Results ({nodeResults.length})</span>
              <span style={{ fontSize: "11px", color: "var(--text-muted)" }}>
                Total: {totalTokens} tokens · {(totalLatency / 1000).toFixed(1)}s
              </span>
            </div>

            {loadingResults && nodeResults.length === 0 && (
              <div style={{ fontSize: "12px", color: "var(--text-muted)", textAlign: "center", padding: "20px" }}>Loading…</div>
            )}

            {nodeResults.map((result) => {
              const meta = STATUS_META[result.status] ?? STATUS_META.pending;
              const Icon = meta.icon;
              const outputStr = typeof result.output === "string" ? result.output : JSON.stringify(result.output, null, 2);
              const inputStr = typeof result.input === "string" ? result.input : JSON.stringify(result.input, null, 2);
              return (
                <div key={result.id} style={{ border: "1px solid var(--border-color)", borderRadius: "8px", padding: "10px", marginBottom: "8px" }}>
                  <div style={{ display: "flex", alignItems: "center", gap: "6px", marginBottom: "6px" }}>
                    <Icon size={13} color={meta.color} />
                    <span style={{ fontSize: "13px", fontWeight: 600 }}>{result.node_id}</span>
                    <span style={{ fontSize: "11px", color: "var(--text-muted)", marginLeft: "auto" }}>
                      {result.latency_ms}ms · {result.token_input + result.token_output} tok
                    </span>
                  </div>
                  {result.error && (
                    <div style={{ fontSize: "11px", color: "var(--danger)", marginBottom: "4px" }}>{result.error}</div>
                  )}
                  <details style={{ fontSize: "11px" }}>
                    <summary style={{ cursor: "pointer", color: "var(--text-muted)" }}>Output</summary>
                    <pre style={{ background: "var(--bg-app)", padding: "8px", borderRadius: "4px", overflowX: "auto", maxHeight: "150px", fontSize: "10px", margin: "4px 0" }}>
                      {outputStr.slice(0, 800)}
                    </pre>
                  </details>
                  <details style={{ fontSize: "11px" }}>
                    <summary style={{ cursor: "pointer", color: "var(--text-muted)" }}>Input</summary>
                    <pre style={{ background: "var(--bg-app)", padding: "8px", borderRadius: "4px", overflowX: "auto", maxHeight: "100px", fontSize: "10px", margin: "4px 0" }}>
                      {inputStr.slice(0, 500)}
                    </pre>
                  </details>
                </div>
              );
            })}

            {!loadingResults && nodeResults.length === 0 && (
              <div style={{ fontSize: "12px", color: "var(--text-muted)", textAlign: "center", padding: "20px" }}>
                Select a run to view node results.
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
