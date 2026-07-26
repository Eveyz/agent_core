import { useState } from "react";
import { useAppDispatch } from "../../hooks/useAppDispatch";
import { setSelectedAgent, fetchAgents } from "../../features/agents/agentSlice";
import type { AgentDef } from "../../features/agents/types";
import { NewAgentModal } from "../ui/NewAgentModal";
import BotIcon from "lucide-react/dist/esm/icons/bot.mjs";
import PlusIcon from "lucide-react/dist/esm/icons/plus.mjs";
import SearchIcon from "lucide-react/dist/esm/icons/search.mjs";
import "./AgentList.css";

interface AgentListProps {
  agents: AgentDef[];
  selectedAgentId: string | null;
}

export function AgentList({ agents, selectedAgentId }: AgentListProps) {
  const dispatch = useAppDispatch();
  const [modalOpen, setModalOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");

  const filteredAgents = agents.filter((a) =>
    a.name.toLowerCase().includes(searchQuery.toLowerCase()) || 
    (a.description && a.description.toLowerCase().includes(searchQuery.toLowerCase()))
  );

  return (
    <div className="agent-list-wrapper">
      <div className="agent-list-header">
        <h2 className="agent-list-title">My Agents</h2>
        <button className="btn-primary new-agent-btn" onClick={() => setModalOpen(true)}>
          <PlusIcon size={14} /> New
        </button>
      </div>
      
      <div className="agent-list-search-container">
        <div className="agent-list-search-field">
          <SearchIcon size={14} className="search-icon" />
          <input
            type="text"
            placeholder="Search agents..."
            className="agent-list-search"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
          />
        </div>
      </div>

      <div className="agent-list-items">
        {filteredAgents.length === 0 ? (
          <div className="agent-list-empty">
            {searchQuery ? "No agents found matching search." : "No custom agents yet."}
          </div>
        ) : (
          filteredAgents.map((agent) => (
            <div
              key={agent.id}
              className={`agent-list-item ${selectedAgentId === agent.id ? "item-active" : ""}`}
              onClick={() => dispatch(setSelectedAgent(agent.id))}
            >
              <div className="agent-item-icon">
                <BotIcon size={18} color={agent.color || "var(--accent)"} />
              </div>
              <div className="agent-item-content">
                <div className="agent-item-name">{agent.name}</div>
                <div className="agent-item-desc">
                  {agent.description || "No description provided."}
                </div>
              </div>
            </div>
          ))
        )}
      </div>

      <NewAgentModal
        isOpen={modalOpen}
        onClose={() => setModalOpen(false)}
        editingAgent={null}
        onSaved={() => dispatch(fetchAgents())}
      />
    </div>
  );
}
