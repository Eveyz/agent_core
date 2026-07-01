import { useEffect, useState } from "react";
import type { AgentDef } from "../../../features/agents/types";
import { useAppDispatch, useAppSelector } from "../../../hooks/useAppDispatch";
import { searchAgentMemory } from "../../../features/agents/agentSlice";
import SearchIcon from "lucide-react/dist/esm/icons/search.mjs";
import BrainIcon from "lucide-react/dist/esm/icons/brain.mjs";

interface AgentMemoryTabProps {
  agent: AgentDef;
}

export function AgentMemoryTab({ agent }: AgentMemoryTabProps) {
  const dispatch = useAppDispatch();
  const memories = useAppSelector((s) => s.agents.memories);
  const [searchQuery, setSearchQuery] = useState("");

  useEffect(() => {
    // Initial fetch of recent memories
    dispatch(searchAgentMemory({ agentId: agent.id, query: "" }));
  }, [agent.id, dispatch]);

  const handleSearch = (e: React.FormEvent) => {
    e.preventDefault();
    dispatch(searchAgentMemory({ agentId: agent.id, query: searchQuery }));
  };

  return (
    <div className="agent-memory-tab" style={{ display: "flex", gap: "32px", height: "100%" }}>
      {/* Left: Core Memory (agverse.md concept) */}
      <div style={{ flex: 1, display: "flex", flexDirection: "column", gap: "16px" }}>
        <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
          <BrainIcon size={20} color="var(--accent)" />
          <h3 style={{ fontSize: "16px", fontWeight: 600, margin: 0, color: "var(--text-main)" }}>Core Memory / agverse.md</h3>
        </div>
        <div style={{ 
          background: "var(--overlay-0_02)", 
          border: "1px solid var(--overlay-0_08)", 
          borderRadius: "12px", 
          padding: "20px",
          flex: 1,
          color: "var(--text-muted)",
          fontSize: "13px",
          fontFamily: "var(--font-mono)",
          whiteSpace: "pre-wrap",
          overflowY: "auto"
        }}>
          {/* In a real app, this would be fetched from the backend */}
          {`# ${agent.name} Core Memory\n\nAgent definition and core directives are initialized.\nPermissions: ${agent.permission_mode}\nMemory Enabled: ${agent.memory_enabled > 0 ? "Yes" : "No"}\n\n// Awaiting deeper integration for raw agverse.md streaming...`}
        </div>
      </div>

      {/* Right: Long Term Memory Search */}
      <div style={{ width: "400px", display: "flex", flexDirection: "column", gap: "16px" }}>
        <h3 style={{ fontSize: "16px", fontWeight: 600, margin: 0, color: "var(--text-main)" }}>Long-term Memory Retrieval</h3>
        
        <form onSubmit={handleSearch} style={{ position: "relative" }}>
          <SearchIcon size={14} style={{ position: "absolute", left: "12px", top: "50%", transform: "translateY(-50%)", color: "var(--text-muted)" }} />
          <input
            type="text"
            className="settings-input"
            placeholder="Search memories..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            style={{ width: "100%", paddingLeft: "36px" }}
          />
        </form>

        <div style={{ display: "flex", flexDirection: "column", gap: "12px", overflowY: "auto", paddingRight: "8px" }}>
          {memories.length === 0 ? (
            <div style={{ textAlign: "center", padding: "40px", color: "var(--text-muted)", fontSize: "13px", background: "var(--overlay-0_02)", borderRadius: "12px" }}>
              No memories found. The agent needs to run more tasks to form long-term memories.
            </div>
          ) : (
            memories.map((m) => (
              <div key={m.id} style={{ 
                background: "var(--overlay-0_03)", 
                border: "1px solid var(--overlay-0_06)", 
                borderRadius: "8px", 
                padding: "16px" 
              }}>
                <div style={{ display: "flex", justifyContent: "space-between", marginBottom: "8px" }}>
                  <span style={{ fontSize: "11px", color: "var(--accent)", textTransform: "uppercase", fontWeight: 600, letterSpacing: "0.05em" }}>{m.role}</span>
                  <span style={{ fontSize: "11px", color: "var(--text-muted)" }}>{m.category}</span>
                </div>
                <div style={{ fontSize: "13px", color: "var(--text-main)", lineHeight: "1.5", whiteSpace: "pre-wrap" }}>
                  {m.content}
                </div>
                <div style={{ fontSize: "10px", color: "var(--text-muted)", marginTop: "12px", textAlign: "right" }}>
                  {new Date(m.created_at).toLocaleString()}
                </div>
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
