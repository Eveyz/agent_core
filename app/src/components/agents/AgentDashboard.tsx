import { useState } from "react";
import type { AgentDef } from "../../features/agents/types";
import { AgentTabs } from "./AgentTabs";
import { useAppDispatch } from "../../hooks/useAppDispatch";
import { deleteAgent, setSelectedAgent } from "../../features/agents/agentSlice";
import { useConfirmDialog } from "../ui/DialogManager";
import BotIcon from "lucide-react/dist/esm/icons/bot.mjs";
import TrashIcon from "lucide-react/dist/esm/icons/trash.mjs";
import MessageSquareIcon from "lucide-react/dist/esm/icons/message-square.mjs";
import "./AgentDashboard.css";

interface AgentDashboardProps {
  agent: AgentDef;
  onBackToChat?: () => void;
}

export function AgentDashboard({ agent, onBackToChat }: AgentDashboardProps) {
  const dispatch = useAppDispatch();
  const { confirm, dialogElement } = useConfirmDialog();
  const [activeTab, setActiveTab] = useState<"config" | "memory" | "skills" | "history">("config");

  const handleDelete = async () => {
    const ok = await confirm({
      title: 'Delete Agent',
      message: `Are you sure you want to delete ${agent.name}?`,
      confirmLabel: 'Delete',
      cancelLabel: 'Cancel',
      danger: true,
    });
    if (ok) {
      await dispatch(deleteAgent(agent.id));
      dispatch(setSelectedAgent(null));
    }
  };

  return (
    <div className="agent-dashboard">
      <div className="agent-dashboard-header">
        <div className="agent-header-main">
          <div className="agent-header-icon">
            <BotIcon size={24} color={agent.color || "var(--accent)"} />
          </div>
          <div className="agent-header-info">
            <h1 className="agent-name">{agent.name}</h1>
            <div className="agent-meta">
              <span className="meta-badge">{agent.model || "Default Model"}</span>
              <span className="meta-badge">Mem: {agent.memory_enabled ? "Enabled" : "Disabled"}</span>
              <span className="meta-badge">{agent.permission_mode} mode</span>
            </div>
          </div>
          <div className="agent-header-actions">
            {onBackToChat && (
              <button className="btn-secondary" onClick={onBackToChat}>
                <MessageSquareIcon size={15} /> Chat
              </button>
            )}
            <button className="btn-secondary icon-btn-danger" onClick={handleDelete} title="Delete Agent">
              <TrashIcon size={16} />
            </button>
          </div>
        </div>

        <div className="agent-tabs-nav">
          <button 
            className={`tab-btn ${activeTab === "config" ? "active" : ""}`}
            onClick={() => setActiveTab("config")}
          >
            Configuration
          </button>
          <button 
            className={`tab-btn ${activeTab === "memory" ? "active" : ""}`}
            onClick={() => setActiveTab("memory")}
          >
            Memory
          </button>
          <button 
            className={`tab-btn ${activeTab === "skills" ? "active" : ""}`}
            onClick={() => setActiveTab("skills")}
          >
            Skills
          </button>
          <button 
            className={`tab-btn ${activeTab === "history" ? "active" : ""}`}
            onClick={() => setActiveTab("history")}
          >
            History
          </button>
        </div>
      </div>

      <div className="agent-dashboard-content">
        <AgentTabs agent={agent} activeTab={activeTab} />
      </div>
      {dialogElement}
    </div>
  );
}
