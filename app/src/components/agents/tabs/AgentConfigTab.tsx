import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSelector } from "react-redux";
import { RootState } from "../../../store";
import { useAppDispatch } from "../../../hooks/useAppDispatch";
import { PERMISSION_MODES, MEMORY_MODES, type AgentDef } from "../../../features/agents/types";
import { updateAgent } from "../../../features/agents/agentSlice";
import XIcon from "lucide-react/dist/esm/icons/x.mjs";
import ChevronDownIcon from "lucide-react/dist/esm/icons/chevron-down.mjs";
import ZapIcon from "lucide-react/dist/esm/icons/zap.mjs";
import SearchIcon from "lucide-react/dist/esm/icons/search.mjs";
import ServerIcon from "lucide-react/dist/esm/icons/server.mjs";
import CheckIcon from "lucide-react/dist/esm/icons/check.mjs";

interface SkillInfo {
  name: string;
  description: string;
  version?: string;
}

interface AgentConfigTabProps {
  agent: AgentDef;
}

export function AgentConfigTab({ agent }: AgentConfigTabProps) {
  const dispatch = useAppDispatch();
  const config = useSelector((state: RootState) => state.settings.config);
  
  const [skillsList, setSkillsList] = useState<{ id: string; name: string }[]>([]);
  const [toolsList, setToolsList] = useState<string[]>([]);
  const [skillSearch, setSkillSearch] = useState("");
  const [modelSearch, setModelSearch] = useState("");
  const [toolSearch, setToolSearch] = useState("");
  const [showSkillDropdown, setShowSkillDropdown] = useState(false);
  const [showToolDropdown, setShowToolDropdown] = useState(false);
  const [showModelDropdown, setShowModelDropdown] = useState(false);

  const [name, setName] = useState(agent.name);
  const [description, setDescription] = useState(agent.description);
  const [model, setModel] = useState(agent.model || config?.default_model || "");
  const [prompt, setPrompt] = useState(agent.system_prompt);
  const [selectedSkills, setSelectedSkills] = useState<string[]>(agent.skills || []);
  const [selectedTools, setSelectedTools] = useState<string[]>(agent.tools || []);
  const [permissionMode, setPermissionMode] = useState(agent.permission_mode || "standard");
  const [memoryEnabled, setMemoryEnabled] = useState(agent.memory_enabled ?? 1);
  const [maxIterations, setMaxIterations] = useState(agent.max_iterations || 50);
  const [maxContextTokens, setMaxContextTokens] = useState(agent.max_context_tokens || 32000);
  
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [successMsg, setSuccessMsg] = useState("");
  const successTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Clean up success message timer on unmount
  useEffect(() => () => {
    if (successTimerRef.current) clearTimeout(successTimerRef.current);
  }, []);

  // Re-sync local state if agent prop changes
  useEffect(() => {
    setName(agent.name);
    setDescription(agent.description);
    setModel(agent.model || config?.default_model || "");
    setPrompt(agent.system_prompt);
    setSelectedSkills(agent.skills || []);
    setSelectedTools(agent.tools || []);
    setPermissionMode(agent.permission_mode || "standard");
    setMemoryEnabled(agent.memory_enabled ?? 1);
    setMaxIterations(agent.max_iterations || 50);
    setMaxContextTokens(agent.max_context_tokens || 32000);
  }, [agent, config?.default_model]);

  useEffect(() => {
    loadSkills();
    loadTools();
  }, []);

  const loadSkills = async () => {
    try {
      const data = await invoke<SkillInfo[]>("get_skills", { sessionId: null, workspace: null });
      setSkillsList(data.map((s) => ({ id: s.name, name: s.name })));
    } catch (e) {
      console.error(e);
    }
  };

  const loadTools = async () => {
    try {
      const data = await invoke<string[]>("list_available_tools");
      setToolsList(data);
    } catch (e) {
      console.error(e);
    }
  };

  const handleSave = async () => {
    if (!name.trim()) {
      setError("Agent name is required");
      return;
    }
    setSaving(true);
    setError(null);
    setSuccessMsg("");
    try {
      await dispatch(updateAgent({
        id: agent.id,
        name: name.trim(),
        description: description.trim(),
        system_prompt: prompt.trim(),
        model,
        skills: selectedSkills,
        tools: selectedTools,
        permission_mode: permissionMode,
        max_iterations: maxIterations,
        max_context_tokens: maxContextTokens,
        memory_enabled: memoryEnabled,
      })).unwrap();
      
      setSuccessMsg("Configuration saved successfully.");
      if (successTimerRef.current) clearTimeout(successTimerRef.current);
      successTimerRef.current = setTimeout(() => setSuccessMsg(""), 3000);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const displayModelName = model
    ? model.includes('/')
      ? model.slice(model.indexOf('/') + 1)
      : model
    : 'Default Model';

  // We reuse most of the NewAgentModal UI but adapted for the tab
  return (
    <div className="agent-config-tab">
      {error && <div style={{ color: "var(--danger)", fontSize: "13px", padding: "12px", background: "var(--danger-bg)", borderRadius: "6px", marginBottom: "20px" }}>{error}</div>}
      {successMsg && <div style={{ color: "var(--success)", fontSize: "13px", padding: "12px", background: "var(--success-bg)", borderRadius: "6px", marginBottom: "20px" }}>{successMsg}</div>}

      <div style={{ display: "grid", gridTemplateColumns: "2fr 1fr", gap: "40px" }}>
        {/* Left Column: Core Prompts & Identity */}
        <div style={{ display: "flex", flexDirection: "column", gap: "20px" }}>
          <div>
            <label style={{ display: "block", marginBottom: "8px", fontWeight: 500 }}>Agent Name <span style={{ color: "var(--danger)" }}>*</span></label>
            <input
              className="settings-input"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. Code Reviewer"
              style={{ width: "100%" }}
            />
          </div>

          <div>
            <label style={{ display: "block", marginBottom: "8px", fontWeight: 500 }}>Description</label>
            <input
              className="settings-input"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="What this agent does..."
              style={{ width: "100%" }}
            />
          </div>

          <div>
            <label style={{ display: "block", marginBottom: "8px", fontWeight: 500 }}>System Prompt</label>
            <div className="input-container" style={{ position: "relative" }}>
              {selectedSkills.length > 0 && (
                <div style={{ display: "flex", gap: "6px", flexWrap: "wrap", padding: "10px 14px", borderBottom: "1px solid var(--overlay-0_06)" }}>
                  {selectedSkills.map(s => (
                    <div key={s} style={{ display: "flex", alignItems: "center", gap: "4px", background: "rgba(139, 92, 246, 0.12)", color: "var(--violet-500)", border: "1px solid rgba(139, 92, 246, 0.25)", padding: "3px 8px", borderRadius: "6px", fontSize: "12px", userSelect: "none" }}>
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
                style={{ color: "var(--text-main)", minHeight: "240px", resize: "none", borderTopLeftRadius: selectedSkills.length ? 0 : undefined, borderTopRightRadius: selectedSkills.length ? 0 : undefined }}
              />

              <div className="input-actions" style={{ background: "transparent" }}>
                <div className="input-actions-left">
                  <div className="model-selector-wrapper">
                    <button
                      type="button"
                      className="model-selector"
                      onClick={() => setShowModelDropdown(!showModelDropdown)}
                    >
                      <span className="model-selector-text">
                        <span className="model-selector-name">{displayModelName}</span>
                      </span>
                      <ChevronDownIcon size={12} className={`model-selector-chevron ${showModelDropdown ? 'open' : ''}`} />
                    </button>

                    {showModelDropdown && (
                      <>
                        <div style={{ position: "fixed", inset: 0, zIndex: 100 }} onClick={() => setShowModelDropdown(false)}></div>
                        <div className="model-dropdown-shell" style={{ zIndex: 101 }}>
                          <div className="model-dropdown">
                            <div className="model-dropdown-search">
                              <SearchIcon size={14} />
                              <input
                                type="text"
                                className="model-dropdown-search-input"
                                placeholder="Search models..."
                                value={modelSearch}
                                onChange={(e) => setModelSearch(e.target.value)}
                                autoFocus
                              />
                            </div>
                            <div className="model-dropdown-list">
                              <button
                                type="button"
                                onClick={() => { setModel(""); setShowModelDropdown(false); setModelSearch(""); }}
                                className={`model-dropdown-item ${!model ? 'selected' : ''}`}
                              >
                                <span className="model-dropdown-item-key">Default Model</span>
                                {!model && <CheckIcon size={14} className="model-dropdown-item-check" />}
                              </button>
                              {config?.providers && Object.entries(config.providers).map(([providerKey, provider]: [string, any]) => {
                                const matchedModels = Object.entries(provider.models).filter(([mKey]) =>
                                  mKey.toLowerCase().includes(modelSearch.toLowerCase()) || providerKey.toLowerCase().includes(modelSearch.toLowerCase())
                                );
                                if (matchedModels.length === 0) return null;
                                return (
                                  <div key={providerKey} className="model-dropdown-group">
                                    <div className="model-dropdown-group-header">
                                      <ServerIcon size={12} />
                                      <span>{provider.name || providerKey}</span>
                                    </div>
                                    {matchedModels.map(([modelKey]) => {
                                      const key = `${providerKey}/${modelKey}`;
                                      const isSelected = model === key;
                                      return (
                                        <button
                                          key={key}
                                          type="button"
                                          className={`model-dropdown-item ${isSelected ? 'selected' : ''}`}
                                          onClick={() => { setModel(key); setShowModelDropdown(false); setModelSearch(""); }}
                                        >
                                          <span className="model-dropdown-item-key">{modelKey}</span>
                                          {isSelected && <CheckIcon size={14} className="model-dropdown-item-check" />}
                                        </button>
                                      );
                                    })}
                                  </div>
                                );
                              })}
                            </div>
                          </div>
                        </div>
                      </>
                    )}
                  </div>

                  <div className="skill-selector-wrapper">
                    <button
                      type="button"
                      className="icon-btn"
                      onClick={() => setShowSkillDropdown(!showSkillDropdown)}
                      title="Attach skills"
                    >
                      <ZapIcon size={16} style={{ color: 'var(--violet-500)' }} />
                    </button>

                    {showSkillDropdown && (
                      <>
                        <div style={{ position: "fixed", inset: 0, zIndex: 100 }} onClick={() => setShowSkillDropdown(false)}></div>
                        <div className="model-dropdown-shell" style={{ zIndex: 101 }}>
                          <div className="model-dropdown" style={{ width: "320px", overflow: "hidden", padding: 0 }}>
                            <div className="model-dropdown-search">
                              <SearchIcon size={14} />
                              <input
                                type="text"
                                className="model-dropdown-search-input"
                                placeholder="Search skills..."
                                value={skillSearch}
                                onChange={(e) => setSkillSearch(e.target.value)}
                                autoFocus
                              />
                            </div>
                            <div className="model-dropdown-list" style={{ maxHeight: "200px" }}>
                              {skillsList.filter(s => s.name.toLowerCase().includes(skillSearch.toLowerCase())).map((s) => {
                                const isSelected = selectedSkills.includes(s.id);
                                return (
                                  <button
                                    key={s.id}
                                    type="button"
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
                            </div>
                          </div>
                        </div>
                      </>
                    )}
                  </div>
                </div>
              </div>
            </div>
          </div>
          
          <div style={{ marginTop: "16px" }}>
            <button className="btn-primary" onClick={handleSave} disabled={saving || !name.trim()} style={{ padding: "10px 24px" }}>
              {saving ? "Saving..." : "Save Configuration"}
            </button>
          </div>
        </div>

        {/* Right Column: Execution Rules */}
        <div style={{ display: "flex", flexDirection: "column", gap: "20px" }}>
          <div style={{ background: "var(--overlay-0_02)", border: "1px solid var(--overlay-0_08)", borderRadius: "12px", padding: "20px" }}>
            <h3 style={{ fontSize: "14px", fontWeight: 600, color: "var(--text-main)", margin: "0 0 16px 0" }}>Execution Rules</h3>
            
            <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
              <div>
                <label style={{ display: "block", marginBottom: "6px", fontSize: "13px", color: "var(--text-muted)" }}>Permission Mode</label>
                <select
                  className="settings-input"
                  value={permissionMode}
                  onChange={(e) => setPermissionMode(e.target.value)}
                  style={{ width: "100%" }}
                >
                  {PERMISSION_MODES.map(m => <option key={m} value={m}>{m}</option>)}
                </select>
                <p style={{ margin: "7px 0 0", fontSize: "12px", color: "var(--text-muted)", lineHeight: 1.45 }}>
                  Controls when this agent asks for tool approval. Paranoid and standard ask more
                  often; developer and permissive allow more operations; yolo runs without prompts.
                </p>
              </div>

              <div>
                <label style={{ display: "block", marginBottom: "6px", fontSize: "13px", color: "var(--text-muted)" }}>Memory Strategy</label>
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
                <label style={{ display: "block", marginBottom: "6px", fontSize: "13px", color: "var(--text-muted)" }}>Max Iterations</label>
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
                <label style={{ display: "block", marginBottom: "6px", fontSize: "13px", color: "var(--text-muted)" }}>Max Context Tokens</label>
                <input
                  className="settings-input"
                  type="number"
                  value={maxContextTokens}
                  onChange={(e) => setMaxContextTokens(Number(e.target.value))}
                  min={1000}
                  step={1000}
                  style={{ width: "100%" }}
                />
              </div>
            </div>
          </div>

          <div style={{ background: "var(--overlay-0_02)", border: "1px solid var(--overlay-0_08)", borderRadius: "12px", padding: "20px" }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", margin: "0 0 16px 0" }}>
              <h3 style={{ fontSize: "14px", fontWeight: 600, color: "var(--text-main)", margin: 0 }}>Tools Integration</h3>
              <button
                className="icon-btn"
                onClick={() => setShowToolDropdown(!showToolDropdown)}
                style={{ padding: "4px 10px", fontSize: "12px", background: "var(--overlay-0_05)" }}
              >
                {showToolDropdown ? "Done" : "Manage"}
              </button>
            </div>
            
            {selectedTools.length === 0 ? (
              <div style={{ fontSize: "13px", color: "var(--text-muted)", fontStyle: "italic" }}>
                Inheriting all globally available tools.
              </div>
            ) : (
              <div style={{ display: "flex", gap: "8px", flexWrap: "wrap" }}>
                {selectedTools.map(t => (
                  <div key={t} style={{ display: "flex", alignItems: "center", gap: "6px", background: "var(--accent-subtle)", color: "var(--accent)", padding: "4px 10px", borderRadius: "6px", fontSize: "13px" }}>
                    <span>{t}</span>
                    <XIcon size={12} style={{ cursor: "pointer", opacity: 0.7 }} onClick={() => setSelectedTools(selectedTools.filter(x => x !== t))} />
                  </div>
                ))}
              </div>
            )}
            
            {showToolDropdown && (
              <div style={{ border: "1px solid var(--overlay-0_1)", borderRadius: "8px", maxHeight: "200px", overflowY: "auto", padding: "8px", marginTop: "12px", background: "var(--bg-main)" }}>
                <input
                  type="text"
                  className="settings-input"
                  placeholder="Search tools..."
                  value={toolSearch}
                  onChange={(e) => setToolSearch(e.target.value)}
                  style={{ width: "100%", marginBottom: "8px" }}
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
                      style={{ display: "flex", gap: "10px", alignItems: "center", width: "100%", padding: "8px" }}
                    >
                      <div style={{
                        width: "14px", height: "14px", borderRadius: "3px",
                        border: `1px solid ${isSelected ? "var(--accent)" : "var(--overlay-0_2)"}`,
                        background: isSelected ? "var(--accent)" : "transparent",
                        display: "flex", alignItems: "center", justifyContent: "center",
                        flexShrink: 0
                      }}>
                        {isSelected && <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="white" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>}
                      </div>
                      <span style={{ fontSize: "13px", textAlign: "left" }}>{t}</span>
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
