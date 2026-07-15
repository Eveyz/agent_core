import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSelector } from "react-redux";
import { RootState } from "../../store";
import { PERMISSION_MODES, MEMORY_MODES, type AgentDef } from "../../features/agents/types";
import XIcon from "lucide-react/dist/esm/icons/x.mjs";
import ChevronDownIcon from "lucide-react/dist/esm/icons/chevron-down.mjs";
import WandIcon from "lucide-react/dist/esm/icons/wand-2.mjs";

interface SkillInfo {
  name: string;
  description: string;
  version?: string;
}

export function NewAgentModal({
  isOpen,
  onClose,
  editingAgent,
  onSaved,
}: {
  isOpen: boolean;
  onClose: () => void;
  /** When provided, the modal edits an existing agent instead of creating one. */
  editingAgent?: AgentDef | null;
  onSaved?: () => void;
}) {
  const config = useSelector((state: RootState) => state.settings.config);
  const [skillsList, setSkillsList] = useState<{ id: string; name: string }[]>([]);
  const [toolsList, setToolsList] = useState<string[]>([]);
  const [skillSearch, setSkillSearch] = useState("");
  const [toolSearch, setToolSearch] = useState("");
  const [showSkillDropdown, setShowSkillDropdown] = useState(false);
  const [showToolDropdown, setShowToolDropdown] = useState(false);
  const [showModelDropdown, setShowModelDropdown] = useState(false);

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [model, setModel] = useState(config?.default_model || "");
  const [prompt, setPrompt] = useState("");
  const [selectedSkills, setSelectedSkills] = useState<string[]>([]);
  const [selectedTools, setSelectedTools] = useState<string[]>([]);
  const [permissionMode, setPermissionMode] = useState("standard");
  const [memoryEnabled, setMemoryEnabled] = useState(1);
  const [maxIterations, setMaxIterations] = useState(50);
  const [maxContextTokens, setMaxContextTokens] = useState(32000);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isEditing = !!editingAgent;

  const loadSkills = useCallback(async () => {
    try {
      const data = await invoke<SkillInfo[]>("get_skills", { sessionId: null, workspace: null });
      setSkillsList(data.map((s) => ({ id: s.name, name: s.name })));
    } catch (e) {
      console.error(e);
    }
  }, []);

  const loadTools = useCallback(async () => {
    try {
      const data = await invoke<string[]>("list_available_tools");
      setToolsList(data);
    } catch (e) {
      console.error(e);
    }
  }, []);

  useEffect(() => {
    if (isOpen) {
      loadSkills();
      loadTools();
      if (editingAgent) {
        setName(editingAgent.name);
        setDescription(editingAgent.description);
        setModel(editingAgent.model || config?.default_model || "");
        setPrompt(editingAgent.system_prompt);
        setSelectedSkills(editingAgent.skills || []);
        setSelectedTools(editingAgent.tools || []);
        setPermissionMode(editingAgent.permission_mode || "standard");
        setMemoryEnabled(editingAgent.memory_enabled ?? 1);
        setMaxIterations(editingAgent.max_iterations || 50);
        setMaxContextTokens(editingAgent.max_context_tokens || 32000);
      } else {
        setName("");
        setDescription("");
        setModel(config?.default_model || "");
        setPrompt("");
        setSelectedSkills([]);
        setSelectedTools([]);
        setPermissionMode("standard");
        setMemoryEnabled(1);
        setMaxIterations(50);
        setMaxContextTokens(32000);
      }
      setError(null);
    }
  }, [isOpen, editingAgent, config?.default_model, loadSkills, loadTools]);

  if (!isOpen) return null;

  const handleSave = async () => {
    if (!name.trim()) {
      setError("Agent name is required");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      if (isEditing && editingAgent) {
        await invoke("update_agent", {
          id: editingAgent.id,
          name: name.trim(),
          description: description.trim(),
          systemPrompt: prompt.trim(),
          model,
          skills: selectedSkills,
          tools: selectedTools,
          permissionMode,
          maxIterations,
          maxContextTokens,
          memoryEnabled,
        });
      } else {
        await invoke("create_agent", {
          name: name.trim(),
          description: description.trim(),
          systemPrompt: prompt.trim(),
          model,
          skills: selectedSkills,
          tools: selectedTools,
          permissionMode,
          maxIterations,
          maxContextTokens,
          memoryEnabled,
        });
      }
      onSaved?.();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="settings-modal-backdrop" onClick={onClose} style={{ zIndex: 9999 }}>
      <div
        className="settings-modal"
        onClick={(e) => e.stopPropagation()}
        style={{ width: "620px", maxHeight: "85vh", overflowY: "auto" }}
      >
        <div className="settings-modal-header">
          <h2 className="settings-modal-title">{isEditing ? "Edit Agent" : "New Agent"}</h2>
          <button className="settings-modal-close" onClick={onClose}>
            <XIcon size={16} />
          </button>
        </div>

        <div className="settings-modal-body" style={{ display: "block", padding: "20px 24px", overflowY: "auto" }}>
          {error && (
            <div style={{ color: "var(--danger)", fontSize: "13px", marginBottom: "12px" }}>
              {error}
            </div>
          )}

          <div style={{ display: "flex", flexDirection: "column", gap: "14px" }}>
            <div>
              <label style={{ display: "block", marginBottom: "4px" }}>Agent Name <span style={{ color: "var(--danger)" }}>*</span></label>
              <input
                className="settings-input"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. Code Reviewer"
                style={{ width: "100%" }}
                autoFocus
              />
            </div>

            <div>
              <label style={{ display: "block", marginBottom: "4px" }}>Description</label>
              <input
                className="settings-input"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="What this agent does..."
                style={{ width: "100%" }}
              />
            </div>

            <div>
              <label style={{ display: "block", marginBottom: "4px" }}>System Prompt</label>
              <div className="input-container" style={{ position: "relative", marginTop: "4px" }}>
                {selectedSkills.length > 0 && (
                  <div style={{ display: "flex", gap: "6px", flexWrap: "wrap", padding: "12px 12px 0 12px" }}>
                    {selectedSkills.map(s => (
                      <div key={s} style={{ display: "flex", alignItems: "center", gap: "4px", background: "var(--accent-subtle)", color: "var(--accent)", padding: "4px 8px", borderRadius: "6px", fontSize: "12px", userSelect: "none" }}>
                        <span style={{ fontWeight: 500 }}>{s}</span>
                        <XIcon size={14} style={{ cursor: "pointer", opacity: 0.7 }} onClick={() => setSelectedSkills(selectedSkills.filter(sk => sk !== s))} />
                      </div>
                    ))}
                  </div>
                )}

                <textarea
                  className="chat-input"
                  value={prompt}
                  onChange={(e) => setPrompt(e.target.value)}
                  placeholder="Instructions for the agent..."
                  style={{ color: "var(--text-main)", minHeight: "100px", resize: "vertical" }}
                />

                <div className="input-actions">
                  <div className="input-actions-left">
                    <div className="model-selector-wrapper">
                      <button
                        className="model-selector"
                        onClick={() => setShowModelDropdown(!showModelDropdown)}
                      >
                        <span className="model-selector-text">
                          <span className="model-selector-name">{model ? (model.includes('/') ? model.split('/')[1] : model) : 'Default Model'}</span>
                        </span>
                        <ChevronDownIcon size={12} className={`model-selector-chevron ${showModelDropdown ? 'open' : ''}`} />
                      </button>

                      {showModelDropdown && (
                        <>
                          <div style={{ position: "fixed", inset: 0, zIndex: 100 }} onClick={() => setShowModelDropdown(false)}></div>
                          <div className="model-dropdown" style={{ zIndex: 101, bottom: "100%", top: "auto", marginBottom: "8px", maxHeight: "240px", overflowY: "auto", padding: "4px" }}>
                            <div
                              onClick={() => { setModel(""); setShowModelDropdown(false); }}
                              className={`model-dropdown-item ${!model ? 'selected' : ''}`}
                            >
                              <span className="model-dropdown-item-key">Default Model</span>
                            </div>
                            {config?.providers && Object.entries(config.providers).map(([providerKey, provider]: [string, any]) => (
                              <div key={providerKey} className="model-dropdown-group">
                                <div className="model-dropdown-group-header">
                                  <span>{provider.name || providerKey}</span>
                                </div>
                                {Object.entries(provider.models).map(([modelKey]) => {
                                  const key = `${providerKey}/${modelKey}`;
                                  const isSelected = model === key;
                                  return (
                                    <button
                                      key={key}
                                      className={`model-dropdown-item ${isSelected ? 'selected' : ''}`}
                                      onClick={() => { setModel(key); setShowModelDropdown(false); }}
                                    >
                                      <span className="model-dropdown-item-key">{modelKey}</span>
                                    </button>
                                  );
                                })}
                              </div>
                            ))}
                          </div>
                        </>
                      )}
                    </div>

                    <div className="skill-selector-wrapper">
                      <button
                        className="icon-btn"
                        onClick={() => setShowSkillDropdown(!showSkillDropdown)}
                        title="Attach skills"
                      >
                        <WandIcon size={16} />
                      </button>

                      {showSkillDropdown && (
                        <>
                          <div style={{ position: "fixed", inset: 0, zIndex: 100 }} onClick={() => setShowSkillDropdown(false)}></div>
                          <div className="model-dropdown" style={{ bottom: "100%", top: "auto", left: 0, marginBottom: "8px", width: "320px", zIndex: 101, overflow: "hidden", padding: 0 }}>
                            <div className="model-dropdown-search">
                              <input
                                type="text"
                                className="model-dropdown-search-input"
                                placeholder="Search skills..."
                                value={skillSearch}
                                onChange={(e) => setSkillSearch(e.target.value)}
                              />
                            </div>
                            <div className="model-dropdown-list" style={{ maxHeight: "200px" }}>
                              {skillsList.filter(s => s.name.toLowerCase().includes(skillSearch.toLowerCase())).map((s) => {
                                const isSelected = selectedSkills.includes(s.id);
                                return (
                                  <button
                                    key={s.id}
                                    className={`model-dropdown-item ${isSelected ? 'selected' : ''}`}
                                    title={s.name}
                                    onClick={() => {
                                      if (isSelected) {
                                        setSelectedSkills(selectedSkills.filter(id => id !== s.id));
                                      } else {
                                        setSelectedSkills([...selectedSkills, s.id]);
                                      }
                                    }}
                                    style={{ display: "flex", gap: "8px", alignItems: "center" }}
                                  >
                                    <div style={{
                                      width: "14px", height: "14px", borderRadius: "3px",
                                      border: `1px solid ${isSelected ? "var(--accent)" : "var(--border-color)"}`,
                                      background: isSelected ? "var(--accent)" : "transparent",
                                      display: "flex", alignItems: "center", justifyContent: "center",
                                      flexShrink: 0
                                    }}>
                                      {isSelected && <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="white" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>}
                                    </div>
                                    <span className="model-dropdown-item-key" style={{ color: isSelected ? "var(--accent)" : "var(--text-main)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", textAlign: "left", flex: 1 }}>{s.name}</span>
                                  </button>
                                );
                              })}
                              {skillsList.filter(s => s.name.toLowerCase().includes(skillSearch.toLowerCase())).length === 0 && (
                                <div className="model-dropdown-empty">No skills found</div>
                              )}
                            </div>
                          </div>
                        </>
                      )}
                    </div>
                  </div>
                </div>
              </div>
            </div>

            {/* ── Extended configuration (PLAN-0009) ── */}
            <div style={{ borderTop: "1px solid var(--border-color)", paddingTop: "14px", marginTop: "4px" }}>
              <div style={{ fontSize: "12px", fontWeight: 600, color: "var(--text-muted)", marginBottom: "10px", textTransform: "uppercase", letterSpacing: "0.5px" }}>Configuration</div>

              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "12px" }}>
                <div>
                  <label style={{ display: "block", marginBottom: "4px", fontSize: "13px" }}>Permission Mode</label>
                  <select
                    className="settings-input"
                    value={permissionMode}
                    onChange={(e) => setPermissionMode(e.target.value)}
                    style={{ width: "100%" }}
                  >
                    {PERMISSION_MODES.map(m => <option key={m} value={m}>{m}</option>)}
                  </select>
                </div>

                <div>
                  <label style={{ display: "block", marginBottom: "4px", fontSize: "13px" }}>Memory</label>
                  <select
                    className="settings-input"
                    value={memoryEnabled}
                    onChange={(e) => setMemoryEnabled(Number(e.target.value))}
                    style={{ width: "100%" }}
                  >
                    {MEMORY_MODES.map(m => <option key={m.value} value={m.value}>{m.label}</option>)}
                  </select>
                </div>

                <div>
                  <label style={{ display: "block", marginBottom: "4px", fontSize: "13px" }}>Max Iterations</label>
                  <input
                    className="settings-input"
                    type="number"
                    value={maxIterations}
                    onChange={(e) => setMaxIterations(Number(e.target.value))}
                    min={1}
                    style={{ width: "100%" }}
                  />
                </div>

                <div>
                  <label style={{ display: "block", marginBottom: "4px", fontSize: "13px" }}>Max Context Tokens</label>
                  <input
                    className="settings-input"
                    type="number"
                    value={maxContextTokens}
                    onChange={(e) => setMaxContextTokens(Number(e.target.value))}
                    min={1000}
                    style={{ width: "100%" }}
                  />
                </div>
              </div>

              {/* Tools picker */}
              <div style={{ marginTop: "12px" }}>
                <label style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "4px", fontSize: "13px" }}>
                  <span>Tools {selectedTools.length === 0 && <span style={{ color: "var(--text-muted)" }}>(empty = inherit all)</span>}</span>
                  <button
                    className="icon-btn"
                    onClick={() => setShowToolDropdown(!showToolDropdown)}
                    style={{ padding: "2px 8px", fontSize: "12px" }}
                  >
                    {showToolDropdown ? "Done" : "Select…"}
                  </button>
                </label>
                {selectedTools.length > 0 && (
                  <div style={{ display: "flex", gap: "6px", flexWrap: "wrap", marginBottom: "6px" }}>
                    {selectedTools.map(t => (
                      <div key={t} style={{ display: "flex", alignItems: "center", gap: "4px", background: "var(--accent-subtle)", color: "var(--accent)", padding: "3px 7px", borderRadius: "5px", fontSize: "12px" }}>
                        <span>{t}</span>
                        <XIcon size={12} style={{ cursor: "pointer", opacity: 0.7 }} onClick={() => setSelectedTools(selectedTools.filter(x => x !== t))} />
                      </div>
                    ))}
                  </div>
                )}
                {showToolDropdown && (
                  <div style={{ border: "1px solid var(--border-color)", borderRadius: "6px", maxHeight: "160px", overflowY: "auto", padding: "4px" }}>
                    <input
                      type="text"
                      className="settings-input"
                      placeholder="Search tools..."
                      value={toolSearch}
                      onChange={(e) => setToolSearch(e.target.value)}
                      style={{ width: "100%", marginBottom: "4px" }}
                    />
                    {toolsList.filter(t => t.toLowerCase().includes(toolSearch.toLowerCase())).map(t => {
                      const isSelected = selectedTools.includes(t);
                      return (
                        <button
                          key={t}
                          className={`model-dropdown-item ${isSelected ? 'selected' : ''}`}
                          onClick={() => {
                            if (isSelected) setSelectedTools(selectedTools.filter(x => x !== t));
                            else setSelectedTools([...selectedTools, t]);
                          }}
                          style={{ display: "flex", gap: "8px", alignItems: "center", width: "100%" }}
                        >
                          <div style={{
                            width: "12px", height: "12px", borderRadius: "2px",
                            border: `1px solid ${isSelected ? "var(--accent)" : "var(--border-color)"}`,
                            background: isSelected ? "var(--accent)" : "transparent",
                            flexShrink: 0
                          }}>
                            {isSelected && <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="white" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" style={{ display: "block" }}><polyline points="20 6 9 17 4 12"></polyline></svg>}
                          </div>
                          <span style={{ fontSize: "12px", textAlign: "left" }}>{t}</span>
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>
            </div>
          </div>

          <div style={{ display: "flex", justifyContent: "flex-end", gap: "8px", marginTop: "20px" }}>
            <button className="btn-secondary" onClick={onClose}>
              Cancel
            </button>
            <button className="btn-primary" onClick={handleSave} disabled={saving || !name.trim()}>
              {saving ? "Saving..." : isEditing ? "Save Changes" : "Create Agent"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
