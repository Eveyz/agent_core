import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSelector } from "react-redux";
import { RootState } from "../../store";
import XIcon from "lucide-react/dist/esm/icons/x.mjs";

interface CronJob {
  id: string;
  name: string;
  cadence_type: string;
  cadence_value: string;
  prompt: string;
  project: string | null;
  skills: string[];
  permission_level: string;
  max_concurrency: number | null;
  enabled: boolean;
}

export function CronjobModal({
  isOpen,
  onClose,
}: {
  isOpen: boolean;
  onClose: () => void;
}) {
  const projects = useSelector((state: RootState) => state.project.projects);
  const [jobs, setJobs] = useState<CronJob[]>([]);
  const [isCreating, setIsCreating] = useState(false);
  const [skillsList, setSkillsList] = useState<{ id: string; name: string }[]>([]);
  const [skillSearch, setSkillSearch] = useState("");

  // Form states
  const [name, setName] = useState("");
  const [cadenceType, setCadenceType] = useState("Hourly");
  const [cadenceValue, setCadenceValue] = useState("0"); // For daily it's time, for weekly it's day-hour, custom is cron string
  const [prompt, setPrompt] = useState("");
  const [projectId, setProjectId] = useState("");
  const [selectedSkills, setSelectedSkills] = useState<string[]>([]);
  const [permissionLevel, setPermissionLevel] = useState("read-only");
  const [maxConcurrency, setMaxConcurrency] = useState<number | "">("");

  useEffect(() => {
    if (isOpen) {
      loadJobs();
      loadSkills();
    }
  }, [isOpen]);

  const loadJobs = async () => {
    try {
      const data = await invoke<CronJob[]>("list_cronjobs");
      setJobs(data);
    } catch (e) {
      console.error(e);
    }
  };

  const loadSkills = async () => {
    try {
      const data = await invoke<any[]>("get_skills");
      setSkillsList(data.map((s) => ({ id: s.name, name: s.name })));
    } catch (e) {
      console.error(e);
    }
  };

  const handleCreate = async () => {
    try {
      await invoke("create_cronjob", {
        name,
        cadenceType,
        cadenceValue,
        prompt,
        project: projectId ? projectId : null,
        skills: selectedSkills,
        permissionLevel,
        maxConcurrency: maxConcurrency === "" ? null : Number(maxConcurrency),
      });
      setIsCreating(false);
      setName("");
      setPrompt("");
      loadJobs();
    } catch (e) {
      alert(`Error creating job: ${e}`);
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await invoke("delete_cronjob", { id });
      loadJobs();
    } catch (e) {
      console.error(e);
    }
  };

  const handleToggle = async (id: string, enabled: boolean) => {
    try {
      await invoke("toggle_cronjob", { id, enabled });
      loadJobs();
    } catch (e) {
      console.error(e);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="settings-modal-backdrop" onClick={onClose} style={{ zIndex: 9999 }}>
      <div
        className="settings-modal"
        onClick={(e) => e.stopPropagation()}
        style={{ width: "800px", height: "auto", maxHeight: "80vh", overflowY: "auto" }}
      >
        <div className="settings-modal-header">
          <h2 className="settings-modal-title">Scheduled Tasks</h2>
          <button className="settings-modal-close" onClick={onClose}>
            <XIcon size={16} />
          </button>
        </div>

        <div className="settings-modal-body" style={{ display: "block", overflowY: "auto", padding: "20px 24px" }}>
          {!isCreating ? (
            <div>
              <button
                className="btn-primary"
                onClick={() => setIsCreating(true)}
                style={{ marginBottom: "16px" }}
              >
                Create New Task
              </button>
              
              {jobs.length === 0 ? (
                <div style={{ color: "var(--text-muted)", fontSize: "14px", textAlign: "center", padding: "32px 0" }}>No tasks scheduled.</div>
              ) : (
                <div style={{ display: "flex", flexDirection: "column", gap: "10px" }}>
                  {jobs.map((job) => (
                    <div key={job.id} style={{ padding: "12px", border: "1px solid var(--border-color)", background: "var(--overlay-0_02)", borderRadius: "8px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                      <div>
                        <div style={{ fontWeight: "600", color: "var(--text-main)" }}>{job.name}</div>
                        <div style={{ fontSize: "12px", color: "var(--text-muted)", marginTop: "4px" }}>
                          {job.cadence_type} ({job.cadence_value}) - {job.prompt}
                        </div>
                      </div>
                      <div style={{ display: "flex", gap: "8px" }}>
                        <button className="btn-secondary" onClick={() => handleToggle(job.id, !job.enabled)}>
                          {job.enabled ? "Pause" : "Resume"}
                        </button>
                        <button className="btn-secondary" style={{ color: "#f87171", borderColor: "transparent" }} onClick={() => handleDelete(job.id)}>
                          Delete
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
              <div>
                <label style={{ display: "block", marginBottom: "4px" }}>Name</label>
                <input
                  type="text"
                  className="settings-input"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="Task name"
                  style={{ width: "100%" }}
                />
              </div>

              <div>
                <label style={{ display: "block", marginBottom: "4px" }}>Cadence</label>
                <div style={{ display: "flex", gap: "8px", alignItems: "center", marginBottom: "8px", minHeight: "36px" }}>
                  <select className="settings-input" value={cadenceType} onChange={(e) => setCadenceType(e.target.value)} style={{ width: "160px" }}>
                    <option value="Hourly">Hourly</option>
                    <option value="Daily">Daily</option>
                    <option value="Weekly">Weekly</option>
                    <option value="Custom">Custom Expression</option>
                  </select>

                  {cadenceType === "Hourly" && (
                    <div style={{ fontSize: "12px", color: "var(--text-muted)", flex: 1 }}>Runs at minute 0 of every hour.</div>
                  )}
                  {cadenceType === "Daily" && (
                    <input type="time" className="settings-input" value={cadenceValue} onChange={(e) => setCadenceValue(e.target.value)} style={{ width: "160px" }} />
                  )}
                  {cadenceType === "Weekly" && (
                    <>
                      <select className="settings-input" value={cadenceValue.split(' ')[0] || 'Monday'} onChange={(e) => setCadenceValue(`${e.target.value} ${cadenceValue.split(' ')[1] || '10:00'}`)} style={{ width: "160px" }}>
                        <option value="Monday">Monday</option>
                        <option value="Tuesday">Tuesday</option>
                        <option value="Wednesday">Wednesday</option>
                        <option value="Thursday">Thursday</option>
                        <option value="Friday">Friday</option>
                        <option value="Saturday">Saturday</option>
                        <option value="Sunday">Sunday</option>
                      </select>
                      <input type="time" className="settings-input" value={cadenceValue.split(' ')[1] || '10:00'} onChange={(e) => setCadenceValue(`${cadenceValue.split(' ')[0] || 'Monday'} ${e.target.value}`)} style={{ width: "160px" }} />
                    </>
                  )}
                  {cadenceType === "Custom" && (
                    <input type="text" className="settings-input" value={cadenceValue} onChange={(e) => setCadenceValue(e.target.value)} placeholder="0 0 * * *" style={{ flex: 1 }} />
                  )}
                </div>
              </div>

              <div>
                <label style={{ display: "block", marginBottom: "4px" }}>Target Project</label>
                <select className="settings-input" value={projectId} onChange={(e) => setProjectId(e.target.value)} style={{ width: "100%" }}>
                  <option value="">(None)</option>
                  {projects.map((p) => (
                    <option key={p.id} value={p.id}>{p.name}</option>
                  ))}
                </select>
              </div>

              <div>
                <label style={{ display: "block", marginBottom: "4px" }}>Skills</label>
                <div style={{ border: "1px solid var(--border-color)", borderRadius: "8px", overflow: "hidden" }}>
                  <div style={{ padding: "8px", borderBottom: "1px solid var(--border-color)", background: "var(--overlay-0_02)" }}>
                    <input 
                      type="text" 
                      className="settings-input" 
                      placeholder="Search skills..." 
                      value={skillSearch}
                      onChange={(e) => setSkillSearch(e.target.value)}
                      style={{ width: "100%", padding: "6px 10px", fontSize: "13px" }}
                    />
                  </div>
                  <div style={{ maxHeight: "180px", overflowY: "auto", background: "var(--overlay-0_015)", padding: "4px" }}>
                    {skillsList.filter(s => s.name.toLowerCase().includes(skillSearch.toLowerCase())).map((s) => {
                      const isSelected = selectedSkills.includes(s.id);
                      return (
                        <div 
                          key={s.id} 
                          onClick={() => {
                            if (isSelected) {
                              setSelectedSkills(selectedSkills.filter(id => id !== s.id));
                            } else {
                              setSelectedSkills([...selectedSkills, s.id]);
                            }
                          }}
                          style={{ 
                            display: "flex", alignItems: "center", gap: "10px", padding: "8px 12px", 
                            cursor: "pointer", borderRadius: "6px",
                            background: isSelected ? "var(--overlay-0_04)" : "transparent",
                            transition: "background 0.15s"
                          }}
                        >
                          <div style={{ 
                            width: "16px", height: "16px", borderRadius: "4px", 
                            border: `1px solid ${isSelected ? "var(--accent)" : "var(--border-color)"}`, 
                            background: isSelected ? "var(--accent)" : "transparent",
                            display: "flex", alignItems: "center", justifyContent: "center"
                          }}>
                            {isSelected && <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="white" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>}
                          </div>
                          <span style={{ fontSize: "13px", color: isSelected ? "var(--accent)" : "var(--text-main)" }}>{s.name}</span>
                        </div>
                      );
                    })}
                    {skillsList.filter(s => s.name.toLowerCase().includes(skillSearch.toLowerCase())).length === 0 && (
                      <div style={{ padding: "16px", textAlign: "center", color: "var(--text-muted)", fontSize: "13px" }}>No skills found.</div>
                    )}
                  </div>
                </div>
              </div>

              <div>
                <label style={{ display: "block", marginBottom: "4px" }}>Permission Level</label>
                <select className="settings-input" value={permissionLevel} onChange={(e) => setPermissionLevel(e.target.value)} style={{ width: "100%" }}>
                  <option value="read-only">Read Only (Auto-allow reads, ask on writes)</option>
                  <option value="read-write">Read & Write (Auto-allow both)</option>
                </select>
              </div>

              <div>
                <label style={{ display: "block", marginBottom: "4px" }}>Max Concurrency</label>
                <input
                  type="number"
                  className="settings-input"
                  value={maxConcurrency}
                  onChange={(e) => setMaxConcurrency(e.target.value ? Number(e.target.value) : "")}
                  placeholder="Leave empty for parallel"
                  style={{ width: "100%" }}
                />
              </div>

              <div>
                <label style={{ display: "block", marginBottom: "4px" }}>Prompt</label>
                <textarea
                  className="settings-textarea"
                  value={prompt}
                  onChange={(e) => setPrompt(e.target.value)}
                  placeholder="Task instructions..."
                  style={{ width: "100%", height: "140px", resize: "vertical" }}
                />
              </div>

              <div style={{ display: "flex", gap: "8px", marginTop: "16px" }}>
                <button className="btn-primary" onClick={handleCreate} disabled={!name || !prompt}>
                  Save Task
                </button>
                <button className="btn-secondary" onClick={() => setIsCreating(false)}>
                  Cancel
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
