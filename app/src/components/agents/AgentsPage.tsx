import { useEffect } from "react";
import { useAppDispatch, useAppSelector } from "../../hooks/useAppDispatch";
import { fetchAgents } from "../../features/agents/agentSlice";
import { AgentList } from "./AgentList";
import { AgentDashboard } from "./AgentDashboard";
import "./AgentsPage.css";

export function AgentsPage() {
  const dispatch = useAppDispatch();
  const selectedAgentId = useAppSelector((s) => s.agents.selectedAgentId);
  const agents = useAppSelector((s) => s.agents.agents);

  useEffect(() => {
    dispatch(fetchAgents());
  }, [dispatch]);

  const selectedAgent = agents.find((a) => a.id === selectedAgentId);

  return (
    <div className="agents-page-container">
      <div className="agents-sidebar">
        <AgentList agents={agents} selectedAgentId={selectedAgentId} />
      </div>
      <div className="agents-main">
        {selectedAgent ? (
          <AgentDashboard agent={selectedAgent} />
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
