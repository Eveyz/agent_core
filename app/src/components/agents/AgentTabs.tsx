import { AgentDef } from "../../features/agents/types";
import { AgentConfigTab } from "./tabs/AgentConfigTab";
import { AgentMemoryTab } from "./tabs/AgentMemoryTab";
import { AgentSkillsTab } from "./tabs/AgentSkillsTab";
import { AgentHistoryTab } from "./tabs/AgentHistoryTab";

interface AgentTabsProps {
  agent: AgentDef;
  activeTab: "config" | "memory" | "skills" | "history";
}

export function AgentTabs({ agent, activeTab }: AgentTabsProps) {
  switch (activeTab) {
    case "config":
      return <AgentConfigTab agent={agent} />;
    case "memory":
      return <AgentMemoryTab agent={agent} />;
    case "skills":
      return <AgentSkillsTab agent={agent} />;
    case "history":
      return <AgentHistoryTab agent={agent} />;
    default:
      return null;
  }
}
