import { memo, useState, useCallback, useRef, useEffect } from "react";
import { useSelector } from "react-redux";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { RootState } from "../../store";
import {
  createProject,
  fetchProjectSessions,
  deleteProject,
  renameProject,
  setActiveProject,
  setActiveSession,
  createSession,
  deleteSession,
  resumeSession,
} from "../../features/project/projectSlice";
import {
  clearChat,
  restoreOrClearSession,
} from "../../features/chat/chatSlice";
import { useAppDispatch } from "../../hooks/useAppDispatch";
import { useSaveSession } from "../../hooks/useSaveSession";
import { useConfirmDialog } from "../ui/DialogManager";
import { formatTimeAgo } from "../../utils/time";
import PlusIcon from "lucide-react/dist/esm/icons/plus.mjs";
import MessageSquareIcon from "lucide-react/dist/esm/icons/message-square.mjs";
import FolderIcon from "lucide-react/dist/esm/icons/folder.mjs";
import SettingsIcon from "lucide-react/dist/esm/icons/settings.mjs";
import SmartphoneIcon from "lucide-react/dist/esm/icons/smartphone.mjs";
import BotIcon from "lucide-react/dist/esm/icons/bot.mjs";
import WorkflowIcon from "lucide-react/dist/esm/icons/workflow.mjs";
import SparklesIcon from "lucide-react/dist/esm/icons/sparkles.mjs";
import TrashIcon from "lucide-react/dist/esm/icons/trash.mjs";
import PencilIcon from "lucide-react/dist/esm/icons/pencil.mjs";
import ExternalLinkIcon from "lucide-react/dist/esm/icons/external-link.mjs";
import ChevronRightIcon from "lucide-react/dist/esm/icons/chevron-right.mjs";
import ChevronDownIcon from "lucide-react/dist/esm/icons/chevron-down.mjs";
import MoreHorizontalIcon from "lucide-react/dist/esm/icons/more-horizontal.mjs";
import LoaderIcon from "lucide-react/dist/esm/icons/loader.mjs";
import ClockIcon from "lucide-react/dist/esm/icons/clock.mjs";
import { CronjobModal } from "../ui/CronjobModal";
import { NewAgentModal } from "../ui/NewAgentModal";

// ── Context menu hook ────────────────────────────────────────────────

function useContextMenu() {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState({ x: 0, y: 0 });
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    if (open) {
      document.addEventListener("mousedown", handleClick);
      return () => document.removeEventListener("mousedown", handleClick);
    }
  }, [open]);

  return { open, setOpen, pos, setPos, menuRef };
}

// ── Project context menu ─────────────────────────────────────────────

function ProjectContextMenu({
  projectId,
  projectName,
  projectPath,
  onDelete,
  onRename,
}: {
  projectId: string;
  projectName: string;
  projectPath: string;
  onDelete: (id: string) => void;
  onRename: (id: string, name: string) => void;
}) {
  const { open, setOpen, pos, setPos, menuRef } = useContextMenu();

  const handleOpenExplorer = async () => {
    try {
      await invoke("open_in_explorer", { path: projectPath });
    } catch {}
    setOpen(false);
  };

  return (
    <>
      <button
        className="sidebar-context-trigger"
        onClick={(e) => {
          e.stopPropagation();
          setPos({ x: e.clientX, y: e.clientY });
          setOpen(true);
        }}
      >
        <MoreHorizontalIcon size={14} />
      </button>
      {open && (
        <div
          ref={menuRef}
          className="context-menu"
          style={{ left: pos.x, top: pos.y }}
        >
          <div
            className="context-menu-item"
            onClick={(e) => {
              e.stopPropagation();
              onRename(projectId, projectName);
              setOpen(false);
            }}
          >
            <PencilIcon size={13} /> Rename
          </div>
          <div
            className="context-menu-item"
            onClick={(e) => {
              e.stopPropagation();
              handleOpenExplorer();
            }}
          >
            <ExternalLinkIcon size={13} /> Open in Explorer
          </div>
          <div className="context-menu-separator" />
          <div
            className="context-menu-item context-menu-danger"
            onClick={(e) => {
              e.stopPropagation();
              setOpen(false);
              onDelete(projectId);
            }}
          >
            <TrashIcon size={13} /> Delete
          </div>
        </div>
      )}
    </>
  );
}

// ── Session context menu ─────────────────────────────────────────────

function SessionContextMenu({
  sessionId,
  projectId,
  onDelete,
}: {
  sessionId: string;
  projectId: string;
  onDelete: (sessionId: string, projectId: string) => void;
}) {
  const { open, setOpen, pos, setPos, menuRef } = useContextMenu();

  return (
    <>
      <button
        className="sidebar-context-trigger"
        onClick={(e) => {
          e.stopPropagation();
          setPos({ x: e.clientX, y: e.clientY });
          setOpen(true);
        }}
      >
        <MoreHorizontalIcon size={12} />
      </button>
      {open && (
        <div
          ref={menuRef}
          className="context-menu"
          style={{ left: pos.x, top: pos.y }}
        >
          <div
            className="context-menu-item context-menu-danger"
            onClick={(e) => {
              e.stopPropagation();
              setOpen(false);
              onDelete(sessionId, projectId);
            }}
          >
            <TrashIcon size={13} /> Delete
          </div>
        </div>
      )}
    </>
  );
}

// ── Sidebar ──────────────────────────────────────────────────────────

export type AppView = "chat" | "agents" | "workflows";

export const Sidebar = memo(function Sidebar({
  activeTab,
  onTabChange,
  onOpenSettings,
  collapsed,
  activeView = "chat",
  onNavigate,
}: {
  activeTab: "code" | "write";
  onTabChange: (tab: "code" | "write") => void;
  onOpenSettings: () => void;
  collapsed: boolean;
  activeView?: AppView;
  onNavigate?: (view: AppView) => void;
}) {
  const dispatch = useAppDispatch();
  const projects = useSelector((state: RootState) => state.project.projects);
  const sessions = useSelector((state: RootState) => state.project.sessions);
  const activeProjectId = useSelector(
    (state: RootState) => state.project.activeProjectId,
  );
  const activeSessionId = useSelector(
    (state: RootState) => state.project.activeSessionId,
  );
  const isProcessing = useSelector(
    (state: RootState) => state.chat.isProcessing,
  );
  const defaultModel = useSelector(
    (state: RootState) => state.settings.config?.default_model || "",
  );
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(
    new Set(),
  );
  const [expandedSessions, setExpandedSessions] = useState<Set<string>>(
    new Set(),
  );
  const [creatingSession, setCreatingSession] = useState(false);
  const [isCronModalOpen, setIsCronModalOpen] = useState(false);
  const [isNewAgentOpen, setIsNewAgentOpen] = useState(false);
  const { confirm, prompt, dialogElement } = useConfirmDialog();

  const toggleSessionsExpand = useCallback(
    (projectId: string, e: React.MouseEvent) => {
      e.stopPropagation();
      setExpandedSessions((prev) => {
        const next = new Set(prev);
        if (next.has(projectId)) {
          next.delete(projectId);
        } else {
          next.add(projectId);
        }
        return next;
      });
    },
    [],
  );

  // Auto-expand active project when it changes
  const lastAutoExpanded = useRef<string | null>(null);
  useEffect(() => {
    if (activeProjectId && lastAutoExpanded.current !== activeProjectId) {
      lastAutoExpanded.current = activeProjectId;
      setExpandedProjects((prev) => {
        if (prev.has(activeProjectId)) return prev;
        const next = new Set(prev);
        next.add(activeProjectId);
        return next;
      });
      dispatch(fetchProjectSessions(activeProjectId));
    }
  }, [activeProjectId, dispatch]);

  const toggleProject = useCallback(
    (projectId: string) => {
      setExpandedProjects((prev) => {
        const next = new Set(prev);
        if (next.has(projectId)) {
          next.delete(projectId);
        } else {
          next.add(projectId);
        }
        return next;
      });
      // We can optimistically fetch sessions when toggled.
      dispatch(fetchProjectSessions(projectId));
    },
    [dispatch],
  );

  const handleOpenFolder = useCallback(async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected && typeof selected === "string") {
      dispatch(createProject(selected));
    }
  }, [dispatch]);

  const handleDeleteProject = useCallback(
    async (projectId: string) => {
      const confirmed = await confirm({
        title: "Delete Project",
        message: "Delete this project and all its sessions?",
        confirmLabel: "Delete",
        cancelLabel: "Cancel",
        danger: true,
      });
      if (confirmed) {
        dispatch(deleteProject(projectId));
      }
    },
    [dispatch, confirm],
  );

  const handleRenameProject = useCallback(
    async (projectId: string, newName: string) => {
      const name = await prompt({
        title: "Rename Project",
        message: "Enter new name:",
        defaultValue: newName,
        confirmLabel: "Rename",
        cancelLabel: "Cancel",
      });
      if (name?.trim())
        dispatch(renameProject({ projectId, newName: name.trim() }));
    },
    [dispatch, prompt],
  );

  // Save current session to backend + memory cache (P2-3: uses shared useSaveSession hook)
  const saveSession = useSaveSession();
  const saveAndCacheCurrent = useCallback(() => {
    const project = projects.find((p) => p.id === activeProjectId);
    saveSession({
      activeSessionId,
      activeProjectPath: project?.path ?? null,
      defaultModel,
      skipIfResumed: true,
      cacheAfter: true,
    });
  }, [saveSession, activeSessionId, activeProjectId, projects, defaultModel]);

  const handleNewSession = useCallback(
    async (projectId: string) => {
      if (creatingSession) return;
      setCreatingSession(true);
      try {
        saveAndCacheCurrent();
        dispatch(setActiveProject(projectId));
        await dispatch(createSession(projectId));
        dispatch(clearChat());
      } finally {
        setCreatingSession(false);
      }
    },
    [dispatch, creatingSession, saveAndCacheCurrent],
  );

  const handleSelectSession = useCallback(
    (sessionId: string, projectId: string) => {
      if (activeSessionId && activeSessionId !== sessionId) {
        saveAndCacheCurrent();
      }
      dispatch(setActiveProject(projectId));
      dispatch(setActiveSession(sessionId));
      dispatch(restoreOrClearSession(sessionId));
      dispatch(resumeSession(sessionId));
    },
    [dispatch, activeSessionId, saveAndCacheCurrent],
  );

  const handleDeleteSession = useCallback(
    async (sessionId: string, projectId: string) => {
      const confirmed = await confirm({
        title: "Delete Session",
        message: "Delete this session?",
        confirmLabel: "Delete",
        cancelLabel: "Cancel",
        danger: true,
      });
      if (confirmed) {
        dispatch(deleteSession({ sessionId, projectId }));
      }
    },
    [dispatch, confirm],
  );

  return (
    <aside className={`sidebar ${collapsed ? "sidebar-collapsed" : ""}`}>
      {/* Toggle group */}
      <div className="toggle-group">
        <button
          className={`toggle-btn ${activeTab === "code" ? "active" : ""}`}
          onClick={() => { onTabChange("code"); onNavigate?.("chat"); }}
        >
          <BotIcon size={14} /> Code
        </button>
        <button
          className={`toggle-btn ${activeTab === "write" ? "active" : ""}`}
          onClick={() => onTabChange("write")}
        >
          <MessageSquareIcon size={14} /> Write
        </button>
      </div>

      {/* Quick actions */}
      <div className="sidebar-nav" style={{ marginBottom: "4px" }}>
        <div
          className={`nav-item ${activeView === "agents" ? "active" : ""}`}
          onClick={() => onNavigate?.("agents")}
        >
          <BotIcon size={14} /> Agents
        </div>
        <div
          className={`nav-item ${activeView === "workflows" ? "active" : ""}`}
          onClick={() => onNavigate?.("workflows")}
        >
          <WorkflowIcon size={14} /> Workflows
        </div>
        <div
          className="nav-item"
          onClick={() => setIsNewAgentOpen(true)}
        >
          <SparklesIcon size={14} /> + New Agent
        </div>
        <div
          className="nav-item"
          onClick={() => setIsCronModalOpen(true)}
        >
          <ClockIcon size={14} /> Schedule Task
        </div>
        <div
          className="nav-item"
          style={{ opacity: 0.4, cursor: "default" }}
          title="Coming soon"
        >
          <MessageSquareIcon size={14} /> New requirement
        </div>
      </div>

      {/* Projects list */}
      <div className="projects-section">
        <div className="projects-header">
          <span>Projects</span>
          <button
            className="icon-btn"
            onClick={handleOpenFolder}
            title="Import project"
          >
            <PlusIcon size={13} />
          </button>
        </div>

        <div className="projects-list">
          {projects.length === 0 && (
            <div
              style={{ padding: "16px 20px", color: "#666", fontSize: "12px" }}
            >
              No projects yet. Click the folder icon above to add one.
            </div>
          )}
          {projects.map((project) => {
            const isExpanded = expandedProjects.has(project.id);
            const projectSessions = sessions[project.id] ?? [];

            return (
              <div key={project.id} className="project-group">
                {/* Project row: folder icon + name + chevron + context menu */}
                <div
                  className="sidebar-project-row"
                  onClick={() => {
                    toggleProject(project.id);
                  }}
                >
                  <span className="sidebar-row-content">
                    {isExpanded ? (
                      <ChevronDownIcon size={14} className="sidebar-chevron" />
                    ) : (
                      <ChevronRightIcon size={14} className="sidebar-chevron" />
                    )}
                    <FolderOpenIcon isOpen={isExpanded} size={15} />
                    <span className="sidebar-row-text">{project.name}</span>
                  </span>
                  <span className="sidebar-row-actions">
                    <button
                      className="sidebar-context-trigger"
                      onClick={(e) => {
                        e.stopPropagation();
                        handleNewSession(project.id);
                      }}
                      title="New session"
                    >
                      <PlusIcon size={13} />
                    </button>
                    <ProjectContextMenu
                      projectId={project.id}
                      projectName={project.name}
                      projectPath={project.path}
                      onDelete={handleDeleteProject}
                      onRename={handleRenameProject}
                    />
                  </span>
                </div>

                {/* Sessions under this project */}
                <div
                  className={`session-list-container ${isExpanded ? "expanded" : ""}`}
                >
                  <div className="session-list">
                    {(() => {
                      const maxInitial = 6;
                      const sessionsFullyExpanded = expandedSessions.has(project.id);
                      const visibleSessions = sessionsFullyExpanded
                        ? projectSessions
                        : projectSessions.slice(0, maxInitial);
                      const hiddenCount = projectSessions.length - maxInitial;

                      return (
                        <>
                          {visibleSessions.map((session) => {
                            const isSessionActive =
                              activeSessionId === session.id &&
                              activeProjectId === project.id;
                            return (
                              <div
                                key={session.id}
                                className={`session-row ${isSessionActive ? "session-row-active" : ""}`}
                                onClick={() =>
                                  handleSelectSession(session.id, project.id)
                                }
                              >
                                <span className="session-row-text">
                                  {session.title || "Untitled"}
                                </span>
                                <>
                                  <span className="session-row-time">
                                    {isSessionActive && isProcessing ? (
                                      <LoaderIcon
                                        size={12}
                                        className="session-processing-spinner"
                                        style={{ marginRight: 0 }}
                                      />
                                    ) : (
                                      formatTimeAgo(session.updated_at)
                                    )}
                                  </span>
                                  <span className="session-row-actions">
                                    <SessionContextMenu
                                      sessionId={session.id}
                                      projectId={project.id}
                                      onDelete={handleDeleteSession}
                                    />
                                    <button
                                      className="sidebar-context-trigger session-delete-btn"
                                      onClick={(e) => {
                                        e.stopPropagation();
                                        handleDeleteSession(session.id, project.id);
                                      }}
                                      title="Delete session"
                                    >
                                      <TrashIcon size={12} />
                                    </button>
                                  </span>
                                </>
                              </div>
                            );
                          })}
                          {!sessionsFullyExpanded && hiddenCount > 0 && (
                            <div
                              className="session-expand-toggle"
                              onClick={(e) => toggleSessionsExpand(project.id, e)}
                            >
                              See all ({hiddenCount})
                            </div>
                          )}
                          {sessionsFullyExpanded && hiddenCount > 0 && (
                            <div
                              className="session-expand-toggle"
                              onClick={(e) => toggleSessionsExpand(project.id, e)}
                            >
                              View less
                            </div>
                          )}
                        </>
                      );
                    })()}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* Bottom */}
      <div className="sidebar-bottom">
        <div className="nav-item">
          <SmartphoneIcon size={14} /> Connect phone
        </div>
        <div className="nav-item" onClick={onOpenSettings}>
          <SettingsIcon size={14} /> Settings
        </div>
      </div>

      <CronjobModal isOpen={isCronModalOpen} onClose={() => setIsCronModalOpen(false)} />
      <NewAgentModal isOpen={isNewAgentOpen} onClose={() => setIsNewAgentOpen(false)} />
      {dialogElement}
    </aside>
  );
});

// ── Folder icon that changes when open/closed ─────────────────────────

function FolderOpenIcon({ isOpen, size }: { isOpen: boolean; size: number }) {
  if (!isOpen) {
    return <FolderIcon size={size} color="#9ca3af" />;
  }
  // Open folder: use a different color or style
  return <FolderIcon size={size} color="#fbbf24" />;
}
