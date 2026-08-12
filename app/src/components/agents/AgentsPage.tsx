import { useEffect, useState } from "react";
import { useAppDispatch, useAppSelector } from "../../hooks/useAppDispatch";
import { fetchAgents } from "../../features/agents/agentSlice";
import { AgentDashboard } from "./AgentDashboard";
import { AgentConversationChat } from "./AgentConversationChat";
import "./AgentsPage.css";

export function AgentsPage() {
  const dispatch = useAppDispatch();
  const selectedAgentId = useAppSelector((s) => s.agents.selectedAgentId);
  const agents = useAppSelector((s) => s.agents.agents);
  const [settingsOpen, setSettingsOpen] = useState(false);

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
          <div className="agents-empty-state">
            <div className="empty-state-content">
              <h3>No Agent Selected</h3>
              <p>Select an agent from the sidebar or create a new one to view and edit its details.</p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
