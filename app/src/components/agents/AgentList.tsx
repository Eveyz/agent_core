import { useEffect, useState } from "react";
import { useAppDispatch, useAppSelector } from "../../hooks/useAppDispatch";
import {
  fetchAgents,
  deleteAgent,
  runAgentStandalone,
  fetchAgentHistory,
  searchAgentMemory,
} from "../../features/agents/agentSlice";
import type { AgentDef } from "../../features/agents/types";
import { NewAgentModal } from "../ui/NewAgentModal";
import BotIcon from "lucide-react/dist/esm/icons/bot.mjs";
import PlusIcon from "lucide-react/dist/esm/icons/plus.mjs";
import TrashIcon from "lucide-react/dist/esm/icons/trash.mjs";
import PencilIcon from "lucide-react/dist/esm/icons/pencil.mjs";
import PlayIcon from "lucide-react/dist/esm/icons/play.mjs";
import { SkillDrafts } from "./SkillDrafts";
import XIcon from "lucide-react/dist/esm/icons/x.mjs";

export function AgentList() {
  const dispatch = useAppDispatch();
  const agents = useAppSelector((s) => s.agents.agents);
  const running = useAppSelector((s) => s.agents.running);
  const runOutput = useAppSelector((s) => s.agents.runOutput);
  const history = useAppSelector((s) => s.agents.history);
  const memories = useAppSelector((s) => s.agents.memories);

  const [editingAgent, setEditingAgent] = useState<AgentDef | null>(null);
  const [modalOpen, setModalOpen] = useState(false);
  const [runInput, setRunInput] = useState<Record<string, string>>({});
  const [viewingAgent, setViewingAgent] = useState<string | null>(null);
  const [draftsAgent, setDraftsAgent] = useState<string | null>(null);

  useEffect(() => {
    dispatch(fetchAgents());
  }, [dispatch]);

  const openNew = () => {
    setEditingAgent(null);
    setModalOpen(true);
  };

  const openEdit = (agent: AgentDef) => {
    setEditingAgent(agent);
    setModalOpen(true);
  };

  const handleDelete = async (id: string) => {
    if (confirm("Delete this agent? This cannot be undone.")) {
      await dispatch(deleteAgent(id));
    }
  };

  const handleRun = async (agent: AgentDef) => {
    const input = runInput[agent.id] ?? "";
    if (!input.trim()) return;
    await dispatch(runAgentStandalone({ agentId: agent.id, input }));
  };

  const viewMemory = (agent: AgentDef) => {
    setViewingAgent(agent.id);
    dispatch(fetchAgentHistory({ agentId: agent.id, limit: 20 }));
    dispatch(searchAgentMemory({ agentId: agent.id, query: agent.name }));
  };

  return (
    <div style={{ display: "flex", height: "100%", overflow: "hidden" }}>
      {/* Agent list */}
      <div style={{ flex: 1, overflowY: "auto", padding: "16px" }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "16px" }}>
          <h2 style={{ fontSize: "16px", fontWeight: 600, margin: 0 }}>Custom Agents</h2>
          <button className="btn-primary" style={{ fontSize: "12px" }} onClick={openNew}>
            <PlusIcon size={12} /> New Agent
          </button>
        </div>

        {agents.length === 0 && (
          <div style={{ color: "var(--text-muted)", textAlign: "center", padding: "40px", fontSize: "13px" }}>
            No custom agents yet. Click "New Agent" to create one.
          </div>
        )}

        <div style={{ display: "flex", flexDirection: "column", gap: "10px" }}>
          {agents.map((agent) => (
            <div key={agent.id} style={{
              border: "1px solid var(--border-color)",
              borderRadius: "8px",
              padding: "14px",
              background: "var(--bg-secondary, #1e1e2e)",
            }}>
              <div style={{ display: "flex", alignItems: "center", gap: "8px", marginBottom: "6px" }}>
                <BotIcon size={16} color="var(--accent, #3b82f6)" />
                <span style={{ fontWeight: 600, fontSize: "14px" }}>{agent.name}</span>
                <span style={{ fontSize: "11px", color: "var(--text-muted)", marginLeft: "auto" }}>
                  {agent.permission_mode} · mem:{agent.memory_enabled}
                </span>
              </div>
              {agent.description && (
                <div style={{ fontSize: "12px", color: "var(--text-muted)", marginBottom: "8px" }}>{agent.description}</div>
              )}
              {agent.skills.length > 0 && (
                <div style={{ display: "flex", gap: "4px", flexWrap: "wrap", marginBottom: "8px" }}>
                  {agent.skills.map(s => (
                    <span key={s} style={{ fontSize: "10px", background: "rgba(82,168,255,0.12)", color: "var(--accent)", padding: "2px 6px", borderRadius: "4px" }}>{s}</span>
                  ))}
                </div>
              )}
              {/* Run input */}
              <div style={{ display: "flex", gap: "6px", marginBottom: "8px" }}>
                <input
                  className="settings-input"
                  placeholder="Test input…"
                  value={runInput[agent.id] ?? ""}
                  onChange={(e) => setRunInput({ ...runInput, [agent.id]: e.target.value })}
                  style={{ flex: 1, fontSize: "12px" }}
                  onKeyDown={(e) => { if (e.key === "Enter") handleRun(agent); }}
                />
                <button className="btn-primary" style={{ fontSize: "12px", padding: "4px 10px" }} onClick={() => handleRun(agent)} disabled={running}>
                  <PlayIcon size={12} />
                </button>
              </div>
              {/* Actions */}
              <div style={{ display: "flex", gap: "6px" }}>
                <button className="btn-secondary" style={{ fontSize: "11px", padding: "3px 8px" }} onClick={() => openEdit(agent)}>
                  <PencilIcon size={11} /> Edit
                </button>
                <button className="btn-secondary" style={{ fontSize: "11px", padding: "3px 8px" }} onClick={() => viewMemory(agent)}>
                  Memory
                </button>
                <button className="btn-secondary" style={{ fontSize: "11px", padding: "3px 8px" }} onClick={() => setDraftsAgent(agent.id)}>
                  Drafts
                </button>
                <button className="btn-secondary" style={{ fontSize: "11px", padding: "3px 8px", color: "var(--danger)" }} onClick={() => handleDelete(agent.id)}>
                  <TrashIcon size={11} />
                </button>
              </div>
              {/* Run output */}
              {runOutput && viewingAgent === null && runInput[agent.id] && (
                <div style={{ marginTop: "8px", padding: "8px", background: "var(--bg-main, #0d0d14)", borderRadius: "4px", fontSize: "12px", maxHeight: "120px", overflowY: "auto", whiteSpace: "pre-wrap" }}>
                  {runOutput}
                </div>
              )}
            </div>
          ))}
        </div>
      </div>

      {/* Memory / history panel */}
      {viewingAgent && (
        <div style={{ width: "320px", borderLeft: "1px solid var(--border-color)", padding: "14px", overflowY: "auto" }}>
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "12px" }}>
            <span style={{ fontSize: "13px", fontWeight: 600 }}>Memory & History</span>
            <button className="icon-btn" onClick={() => setViewingAgent(null)}><XIcon size={14} /></button>
          </div>
          <div style={{ fontSize: "11px", color: "var(--text-muted)", textTransform: "uppercase", marginBottom: "6px" }}>Recent Memories</div>
          {memories.length === 0 && <div style={{ fontSize: "12px", color: "var(--text-muted)", marginBottom: "12px" }}>No memories yet.</div>}
          {memories.slice(0, 5).map((m) => (
            <div key={m.id} style={{ fontSize: "11px", padding: "6px", background: "var(--bg-main, #0d0d14)", borderRadius: "4px", marginBottom: "4px" }}>
              <div style={{ color: "var(--text-muted)", marginBottom: "2px" }}>{m.role} · {m.category}</div>
              <div style={{ whiteSpace: "pre-wrap", maxHeight: "60px", overflow: "hidden" }}>{m.content}</div>
            </div>
          ))}
          <div style={{ fontSize: "11px", color: "var(--text-muted)", textTransform: "uppercase", margin: "12px 0 6px" }}>Execution History</div>
          {history.length === 0 && <div style={{ fontSize: "12px", color: "var(--text-muted)" }}>No executions yet.</div>}
          {history.slice(0, 10).map((h) => (
            <div key={h.id} style={{ fontSize: "11px", padding: "6px", background: "var(--bg-main, #0d0d14)", borderRadius: "4px", marginBottom: "4px" }}>
              <div style={{ display: "flex", justifyContent: "space-between" }}>
                <span style={{ color: h.success ? "var(--success, #22c55e)" : "var(--danger)" }}>{h.success ? "✓" : "✗"} {h.trigger}</span>
                <span style={{ color: "var(--text-muted)" }}>{h.process_time_ms}ms</span>
              </div>
              <div style={{ color: "var(--text-muted)", marginTop: "2px", maxHeight: "40px", overflow: "hidden" }}>{h.input.slice(0, 80)}</div>
            </div>
          ))}
        </div>
      )}

      {draftsAgent && (
        <SkillDrafts agentId={draftsAgent} onClose={() => setDraftsAgent(null)} />
      )}
      <NewAgentModal
        isOpen={modalOpen}
        onClose={() => setModalOpen(false)}
        editingAgent={editingAgent}
        onSaved={() => dispatch(fetchAgents())}
      />
    </div>
  );
}
