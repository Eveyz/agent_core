import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAppSelector } from "../../hooks/useAppDispatch";
import XIcon from "lucide-react/dist/esm/icons/x.mjs";
import CheckIcon from "lucide-react/dist/esm/icons/check.mjs";
import SparklesIcon from "lucide-react/dist/esm/icons/sparkles.mjs";
import LoaderIcon from "lucide-react/dist/esm/icons/loader.mjs";

interface SkillDraft {
  name: string;
  description: string;
  rationale: string;
  body: string;
  triggers: string[];
  agent_id: string;
  samples_analyzed: number;
  generated_at: string;
}

interface DraftGenerationResult {
  agent_id: string;
  drafts: SkillDraft[];
  samples_analyzed: number;
}

/**
 * SkillDrafts — experimental (PLAN-0009 Phase 6).
 *
 * Lets the user generate skill drafts from an agent's execution history,
 * review them, and approve (promote to live skills) or reject (delete).
 */
export function SkillDrafts({
  agentId,
  onClose,
}: {
  agentId: string;
  onClose: () => void;
}) {
  const agents = useAppSelector((s) => s.agents.agents);
  const agent = agents.find((a) => a.id === agentId);
  const [drafts, setDrafts] = useState<SkillDraft[]>([]);
  const [generating, setGenerating] = useState(false);
  const [genResult, setGenResult] = useState<DraftGenerationResult | null>(null);
  const [actioning, setActioning] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [expandedDraft, setExpandedDraft] = useState<string | null>(null);

  const loadDrafts = async () => {
    try {
      const data = await invoke<SkillDraft[]>("list_skill_drafts");
      setDrafts(data);
    } catch (e) {
      console.error(e);
    }
  };

  useEffect(() => {
    loadDrafts();
  }, []);

  const handleGenerate = async () => {
    setGenerating(true);
    setError(null);
    try {
      const result = await invoke<DraftGenerationResult>("generate_agent_skill_drafts", {
        agentId,
        limit: 100,
      });
      setGenResult(result);
      await loadDrafts();
    } catch (e) {
      setError(String(e));
    } finally {
      setGenerating(false);
    }
  };

  const handleApprove = async (name: string) => {
    setActioning(name);
    try {
      await invoke("approve_skill_draft", { name });
      await loadDrafts();
    } catch (e) {
      setError(String(e));
    } finally {
      setActioning(null);
    }
  };

  const handleReject = async (name: string) => {
    setActioning(name);
    try {
      await invoke("reject_skill_draft", { name });
      await loadDrafts();
    } catch (e) {
      setError(String(e));
    } finally {
      setActioning(null);
    }
  };

  return (
    <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.6)", zIndex: 9998, display: "flex", alignItems: "center", justifyContent: "center" }} onClick={onClose}>
      <div style={{ width: "640px", maxHeight: "80vh", background: "var(--bg-secondary, #1e1e2e)", borderRadius: "12px", display: "flex", flexDirection: "column", overflow: "hidden" }} onClick={(e) => e.stopPropagation()}>
        {/* Header */}
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "14px 18px", borderBottom: "1px solid var(--border-color)" }}>
          <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
            <SparklesIcon size={16} color="var(--accent)" />
            <h3 style={{ margin: 0, fontSize: "15px", fontWeight: 600 }}>Skill Drafts — {agent?.name ?? agentId}</h3>
          </div>
          <button className="icon-btn" onClick={onClose}><XIcon size={16} /></button>
        </div>

        <div style={{ padding: "14px 18px", overflowY: "auto", flex: 1 }}>
          {/* Experimental banner */}
          <div style={{ padding: "8px 12px", background: "rgba(245,158,11,0.1)", border: "1px solid rgba(245,158,11,0.3)", borderRadius: "6px", fontSize: "12px", color: "var(--warning, #f59e0b)", marginBottom: "12px" }}>
            ⚡ Experimental: Drafts are generated from execution history and require human review before activation.
          </div>

          {/* Generate button */}
          <button
            className="btn-primary"
            style={{ width: "100%", fontSize: "13px", marginBottom: "12px" }}
            onClick={handleGenerate}
            disabled={generating}
          >
            {generating ? <><LoaderIcon size={14} className="spin" /> Analyzing history…</> : <><SparklesIcon size={14} /> Generate Drafts from History</>}
          </button>

          {genResult && (
            <div style={{ fontSize: "12px", color: "var(--text-muted)", marginBottom: "12px" }}>
              Analyzed {genResult.samples_analyzed} execution(s) → generated {genResult.drafts.length} draft(s).
            </div>
          )}

          {error && (
            <div style={{ fontSize: "12px", color: "var(--danger)", marginBottom: "12px" }}>{error}</div>
          )}

          {/* Drafts list */}
          {drafts.length === 0 && (
            <div style={{ fontSize: "13px", color: "var(--text-muted)", textAlign: "center", padding: "24px" }}>
              No skill drafts yet. Click "Generate Drafts" to analyze this agent's execution history.
            </div>
          )}

          {drafts.map((draft) => (
            <div key={draft.name} style={{ border: "1px solid var(--border-color)", borderRadius: "8px", padding: "12px", marginBottom: "10px" }}>
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: "6px" }}>
                <div>
                  <div style={{ fontSize: "13px", fontWeight: 600 }}>{draft.name}</div>
                  <div style={{ fontSize: "11px", color: "var(--text-muted)" }}>{draft.description}</div>
                </div>
                <span style={{ fontSize: "10px", color: "var(--text-muted)", background: "var(--bg-main, #0d0d14)", padding: "2px 6px", borderRadius: "4px" }}>
                  {draft.samples_analyzed} samples
                </span>
              </div>

              <div style={{ fontSize: "11px", color: "var(--text-muted)", marginBottom: "8px", fontStyle: "italic" }}>
                {draft.rationale}
              </div>

              {draft.triggers.length > 0 && (
                <div style={{ display: "flex", gap: "4px", flexWrap: "wrap", marginBottom: "8px" }}>
                  {draft.triggers.map(t => (
                    <span key={t} style={{ fontSize: "10px", background: "rgba(82,168,255,0.12)", color: "var(--accent)", padding: "2px 6px", borderRadius: "4px" }}>{t}</span>
                  ))}
                </div>
              )}

              {/* Expandable body */}
              {expandedDraft === draft.name && (
                <pre style={{ background: "var(--bg-main, #0d0d14)", padding: "8px", borderRadius: "4px", fontSize: "10px", maxHeight: "200px", overflowY: "auto", margin: "0 0 8px 0", whiteSpace: "pre-wrap" }}>
                  {draft.body}
                </pre>
              )}

              <div style={{ display: "flex", gap: "6px" }}>
                <button
                  className="btn-secondary"
                  style={{ fontSize: "11px", padding: "3px 8px" }}
                  onClick={() => setExpandedDraft(expandedDraft === draft.name ? null : draft.name)}
                >
                  {expandedDraft === draft.name ? "Hide" : "Preview"}
                </button>
                <button
                  className="btn-primary"
                  style={{ fontSize: "11px", padding: "3px 8px", background: "var(--success, #22c55e)" }}
                  onClick={() => handleApprove(draft.name)}
                  disabled={actioning === draft.name}
                >
                  <CheckIcon size={11} /> Approve
                </button>
                <button
                  className="btn-secondary"
                  style={{ fontSize: "11px", padding: "3px 8px", color: "var(--danger)" }}
                  onClick={() => handleReject(draft.name)}
                  disabled={actioning === draft.name}
                >
                  <XIcon size={11} /> Reject
                </button>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
