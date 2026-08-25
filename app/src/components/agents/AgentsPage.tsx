import { useEffect, useState } from "react";
import { useAppDispatch, useAppSelector } from "../../hooks/useAppDispatch";
import { fetchAgents, setSelectedAgent } from "../../features/agents/agentSlice";
import { AgentDashboard } from "./AgentDashboard";
import { AgentConversationChat } from "./AgentConversationChat";
import { NewAgentModal } from "../ui/NewAgentModal";
import BotIcon from "lucide-react/dist/esm/icons/bot.mjs";
import PlusIcon from "lucide-react/dist/esm/icons/plus.mjs";
import MessageSquareIcon from "lucide-react/dist/esm/icons/message-square.mjs";
import "./AgentsPage.css";

export function AgentsPage() {
  const dispatch = useAppDispatch();
  const selectedAgentId = useAppSelector((s) => s.agents.selectedAgentId);
  const agents = useAppSelector((s) => s.agents.agents);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [newAgentOpen, setNewAgentOpen] = useState(false);

  useEffect(() => {
    dispatch(fetchAgents());
  }, [dispatch]);

  const selectedAgent = agents.find((a) => a.id === selectedAgentId);

  useEffect(() => {
    setSettingsOpen(false);
  }, [selectedAgentId]);

  return (
    <div className="agents-page-container">
      <div className="agents-main">
        {selectedAgent ? (
          settingsOpen ? (
            <AgentDashboard agent={selectedAgent} onBackToChat={() => setSettingsOpen(false)} />
          ) : (
            <AgentConversationChat
              agent={selectedAgent}
              onOpenSettings={() => setSettingsOpen(true)}
            />
          )
        ) : (
          <div className="agent-swarm-home">
            <div className="agent-swarm-home-hero">
              <div className="agent-swarm-home-mark">
                <BotIcon size={25} />
              </div>
              <span className="agent-swarm-home-eyebrow">Agent Swarm</span>
              <h1>Your team, one conversation away.</h1>
              <p>
                Open an agent like a contact. Give it work directly, or mention another agent to
                coordinate a durable multi-agent task.
              </p>
              <button className="agent-swarm-create" onClick={() => setNewAgentOpen(true)}>
                <PlusIcon size={15} /> Create agent
              </button>
            </div>

            {agents.length > 0 ? (
              <div className="agent-swarm-contact-grid">
                {agents.map((agent) => (
                  <button
                    key={agent.id}
                    className="agent-swarm-contact-card"
                    onClick={() => dispatch(setSelectedAgent(agent.id))}
                    style={{ "--agent-color": agent.color || "var(--accent)" } as React.CSSProperties}
                  >
                    <span className="agent-swarm-contact-avatar">
                      <BotIcon size={20} />
                      <span />
                    </span>
                    <span className="agent-swarm-contact-details">
                      <strong>{agent.name}</strong>
                      <small>{agent.description || "Custom agent"}</small>
                    </span>
                    <span className="agent-swarm-contact-action">
                      <MessageSquareIcon size={14} /> Chat
                    </span>
                  </button>
                ))}
              </div>
            ) : (
              <div className="agent-swarm-home-empty">Create your first agent to start a swarm.</div>
            )}

            <NewAgentModal
              isOpen={newAgentOpen}
              onClose={() => setNewAgentOpen(false)}
              editingAgent={null}
              onSaved={() => dispatch(fetchAgents())}
            />
          </div>
        )}
      </div>
    </div>
  );
}
