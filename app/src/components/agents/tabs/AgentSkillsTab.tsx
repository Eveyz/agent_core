import type { AgentDef } from "../../../features/agents/types";
import WandIcon from "lucide-react/dist/esm/icons/wand-2.mjs";

interface AgentSkillsTabProps {
  agent: AgentDef;
}

export function AgentSkillsTab({ agent }: AgentSkillsTabProps) {
  const skills = agent.skills || [];

  return (
    <div className="agent-skills-tab">
      <div style={{ marginBottom: "24px" }}>
        <h3 style={{ fontSize: "16px", fontWeight: 600, margin: "0 0 8px 0", color: "var(--text-main)" }}>Acquired Skills</h3>
        <p style={{ fontSize: "13px", color: "var(--text-muted)", margin: 0 }}>
          Skills are specialized capabilities this agent has acquired or been explicitly taught.
        </p>
      </div>

      {skills.length === 0 ? (
        <div style={{ 
          textAlign: "center", 
          padding: "60px", 
          background: "rgba(255,255,255,0.02)", 
          border: "1px dashed rgba(255, 255, 255, 0.1)", 
          borderRadius: "16px" 
        }}>
          <WandIcon size={32} color="var(--text-muted)" style={{ marginBottom: "16px", opacity: 0.5 }} />
          <div style={{ color: "var(--text-main)", fontSize: "14px", fontWeight: 500, marginBottom: "8px" }}>No Skills Acquired</div>
          <div style={{ color: "var(--text-muted)", fontSize: "13px" }}>Go to the Configuration tab to attach skills to this agent.</div>
        </div>
      ) : (
        <div style={{ 
          display: "grid", 
          gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))", 
          gap: "20px" 
        }}>
          {skills.map((skillName) => (
            <div key={skillName} style={{ 
              background: "linear-gradient(145deg, rgba(255,255,255,0.05) 0%, rgba(255,255,255,0.02) 100%)",
              border: "1px solid rgba(255, 255, 255, 0.08)", 
              borderRadius: "12px", 
              padding: "20px",
              display: "flex",
              flexDirection: "column",
              gap: "12px"
            }}>
              <div style={{ display: "flex", alignItems: "center", gap: "12px" }}>
                <div style={{ 
                  width: "40px", 
                  height: "40px", 
                  borderRadius: "10px", 
                  background: "rgba(59, 130, 246, 0.15)",
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  color: "var(--accent)"
                }}>
                  <WandIcon size={20} />
                </div>
                <div>
                  <div style={{ fontSize: "15px", fontWeight: 600, color: "var(--text-main)" }}>{skillName}</div>
                  <div style={{ fontSize: "12px", color: "var(--accent)", marginTop: "2px" }}>Active</div>
                </div>
              </div>
              <div style={{ fontSize: "13px", color: "var(--text-muted)", lineHeight: "1.5" }}>
                This skill provides specialized context and tool sets to enhance the agent's performance in related tasks.
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
