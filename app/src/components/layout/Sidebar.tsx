import { memo, useState, useCallback, useRef, useEffect } from "react";
import { useSelector } from "react-redux";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
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
} from "../../features/project/projectSlice";
import {
  clearChat,
} from "../../features/chat/chatSlice";
import { useAppDispatch } from "../../hooks/useAppDispatch";
import { useSaveSession } from "../../hooks/useSaveSession";
import { useConfirmDialog } from "../ui/DialogManager";
import { SessionList } from "./SessionList";
import PlusIcon from "lucide-react/dist/esm/icons/plus.mjs";
import MessageSquareIcon from "lucide-react/dist/esm/icons/message-square.mjs";
import FolderIcon from "lucide-react/dist/esm/icons/folder.mjs";
import SettingsIcon from "lucide-react/dist/esm/icons/settings.mjs";
import SmartphoneIcon from "lucide-react/dist/esm/icons/smartphone.mjs";
import BotIcon from "lucide-react/dist/esm/icons/bot.mjs";
import WorkflowIcon from "lucide-react/dist/esm/icons/workflow.mjs";

import TrashIcon from "lucide-react/dist/esm/icons/trash.mjs";
import PencilIcon from "lucide-react/dist/esm/icons/pencil.mjs";
import ExternalLinkIcon from "lucide-react/dist/esm/icons/external-link.mjs";
import ChevronRightIcon from "lucide-react/dist/esm/icons/chevron-right.mjs";
import ChevronDownIcon from "lucide-react/dist/esm/icons/chevron-down.mjs";
import MoreHorizontalIcon from "lucide-react/dist/esm/icons/more-horizontal.mjs";
import ClockIcon from "lucide-react/dist/esm/icons/clock.mjs";
import { CronjobModal } from "../ui/CronjobModal";

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
  const { t } = useTranslation();
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
            <PencilIcon size={13} /> {t("sidebar.actions.rename")}
          </div>
          <div
            className="context-menu-item"
            onClick={(e) => {
              e.stopPropagation();
              handleOpenExplorer();
            }}
          >
            <ExternalLinkIcon size={13} /> {t("sidebar.actions.openInExplorer")}
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
            <TrashIcon size={13} /> {t("sidebar.actions.delete")}
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
  const { t } = useTranslation();
  const dispatch = useAppDispatch();
  const projects = useSelector((state: RootState) => state.project.projects);
  const sessions = useSelector((state: RootState) => state.project.sessions);
  const activeProjectId = useSelector(
    (state: RootState) => state.project.activeProjectId,
  );
  const activeSessionId = useSelector(
    (state: RootState) => state.project.activeSessionId,
  );
  const processingBySession = useSelector(
    (state: RootState) => state.chat.processing,
  );
  const isResuming = useSelector(
    (state: RootState) => state.chat.isResuming,
  );
  const defaultModel = useSelector(
    (state: RootState) => state.settings.config?.default_model || "",
  );
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(
    new Set(),
  );
  const [creatingSession, setCreatingSession] = useState(false);
  const [isCronModalOpen, setIsCronModalOpen] = useState(false);
  const [projectsCollapsed, setProjectsCollapsed] = useState(false);
  const [chatCollapsed, setChatCollapsed] = useState(false);
  const { confirm, prompt, dialogElement } = useConfirmDialog();

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

  // Fetch default project sessions on mount for the Chat section
  useEffect(() => {
    dispatch(fetchProjectSessions('__adhoc_chat__'));
  }, [dispatch]);

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
        title: t("sidebar.prompts.deleteProjectTitle"),
        message: t("sidebar.prompts.deleteProjectMessage"),
        confirmLabel: t("sidebar.actions.delete"),
        cancelLabel: t("sidebar.actions.cancel"),
        danger: true,
      });
      if (confirmed) {
        dispatch(deleteProject(projectId));
      }
    },
    [dispatch, confirm, t],
  );

  const handleRenameProject = useCallback(
    async (projectId: string, newName: string) => {
      const name = await prompt({
        title: t("sidebar.prompts.renameProjectTitle"),
        message: t("sidebar.prompts.renameProjectMessage"),
        defaultValue: newName,
        confirmLabel: t("sidebar.actions.rename"),
        cancelLabel: t("sidebar.actions.cancel"),
      });
      if (name?.trim())
        dispatch(renameProject({ projectId, newName: name.trim() }));
    },
    [dispatch, prompt, t],
  );

  // Save current session to backend + memory cache (P2-3: uses shared useSaveSession hook)
  const saveSession = useSaveSession();
  const saveAndCacheCurrent = useCallback(() => {
    const project = projects.find((p) => p.id === activeProjectId);
    saveSession({
      activeSessionId,
      activeProjectPath: project?.path ?? null,
      defaultModel,
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
    },
    [dispatch, activeSessionId, saveAndCacheCurrent],
  );

  const handleDeleteSession = useCallback(
    async (sessionId: string, projectId: string) => {
      const confirmed = await confirm({
        title: t("sidebar.prompts.deleteSessionTitle"),
        message: t("sidebar.prompts.deleteSessionMessage"),
        confirmLabel: t("sidebar.actions.delete"),
        cancelLabel: t("sidebar.actions.cancel"),
        danger: true,
      });
      if (confirmed) {
        dispatch(deleteSession({ sessionId, projectId }));
      }
    },
    [dispatch, confirm, t],
  );

  const regularProjects = projects.filter((p) => p.id !== '__adhoc_chat__');

  return (
    <aside className={`sidebar ${collapsed ? "sidebar-collapsed" : ""} ${isResuming ? "sidebar-resuming" : ""}`}>
      {/* Toggle group */}
      <div className="toggle-group">
        <button
          className={`toggle-btn ${activeTab === "code" ? "active" : ""}`}
          onClick={() => { onTabChange("code"); onNavigate?.("chat"); }}
        >
          <BotIcon size={14} /> {t("sidebar.tabs.code")}
        </button>
        <button
          className={`toggle-btn ${activeTab === "write" ? "active" : ""}`}
          onClick={() => onTabChange("write")}
        >
          <MessageSquareIcon size={14} /> {t("sidebar.tabs.write")}
        </button>
      </div>

      {/* Quick actions */}
      <div className="sidebar-nav" style={{ marginBottom: "4px" }}>
        <div
          className={`nav-item ${activeView === "agents" ? "active" : ""}`}
          onClick={() => onNavigate?.("agents")}
        >
          <BotIcon size={14} /> {t("sidebar.agents")}
        </div>
        <div
          className={`nav-item ${activeView === "workflows" ? "active" : ""}`}
          onClick={() => onNavigate?.("workflows")}
        >
          <WorkflowIcon size={14} /> {t("sidebar.workflows")}
        </div>
        <div
          className="nav-item"
          onClick={() => setIsCronModalOpen(true)}
        >
          <ClockIcon size={14} /> {t("sidebar.nav.scheduleTask")}
        </div>
        <div
          className="nav-item"
          style={{ opacity: 0.4, cursor: "default" }}
          title={t("sidebar.nav.comingSoon")}
        >
          <MessageSquareIcon size={14} /> {t("sidebar.nav.newRequirement")}
        </div>
      </div>

      <div className="sidebar-scrollable">
        {/* Projects list */}
        <div className={`projects-section ${projectsCollapsed ? "section-collapsed" : ""}`}>
        <div className="projects-header" role="button" tabIndex={0} aria-expanded={!projectsCollapsed} onClick={() => setProjectsCollapsed(!projectsCollapsed)} onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); setProjectsCollapsed(!projectsCollapsed); } }}>
          <span style={{ display: "flex", alignItems: "center", gap: "6px" }}>
            {projectsCollapsed ? (
              <ChevronRightIcon size={12} style={{ opacity: 0.7 }} />
            ) : (
              <ChevronDownIcon size={12} style={{ opacity: 0.7 }} />
            )}
            {t("sidebar.projects")}
          </span>
          <button
            className="icon-btn"
            onClick={(e) => {
              e.stopPropagation();
              handleOpenFolder();
            }}
            title="Import project"
          >
            <PlusIcon size={13} />
          </button>
        </div>

        {!projectsCollapsed && (
          <div className="projects-list">
          {regularProjects.length === 0 && (
            <div
              style={{ padding: "16px 20px", color: "var(--text-tertiary)", fontSize: "12px" }}
            >
              {t("sidebar.projectsSection.noProjects")}
            </div>
          )}
          {regularProjects.map((project) => {
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
                    <SessionList
                      sessions={projectSessions}
                      activeSessionId={
                        activeView === "chat" && activeProjectId === project.id
                          ? activeSessionId
                          : null
                      }
                      onSelectSession={(sessionId) =>
                        handleSelectSession(sessionId, project.id)
                      }
                      title={project.name}
                      emptyMessage={t("sidebar.projectsSection.emptySessions")}
                      processingBySession={processingBySession}
                      onDeleteSession={(sessionId) =>
                        handleDeleteSession(sessionId, project.id)
                      }
                    />
                  </div>
                </div>
              </div>
            );
          })}
          </div>
        )}
      </div>

      {/* Chat section */}
      <div className={`projects-section ${chatCollapsed ? "section-collapsed" : ""}`} style={{ marginTop: "12px" }}>
        <div className="projects-header" role="button" tabIndex={0} aria-expanded={!chatCollapsed} onClick={() => setChatCollapsed(!chatCollapsed)} onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); setChatCollapsed(!chatCollapsed); } }}>
          <span style={{ display: "flex", alignItems: "center", gap: "6px" }}>
            {chatCollapsed ? (
              <ChevronRightIcon size={12} style={{ opacity: 0.7 }} />
            ) : (
              <ChevronDownIcon size={12} style={{ opacity: 0.7 }} />
            )}
            {t("sidebar.chat")}
          </span>
          <button
            className="icon-btn"
            onClick={(e) => {
              e.stopPropagation();
              handleNewSession('__adhoc_chat__');
            }}
            title="New chat"
          >
            <PlusIcon size={13} />
          </button>
        </div>

        {!chatCollapsed && (
          <div className="projects-list">
            <div className="session-list">
              <SessionList
                sessions={sessions['__adhoc_chat__'] ?? []}
                activeSessionId={
                  activeView === "chat" && activeProjectId === '__adhoc_chat__'
                    ? activeSessionId
                    : null
                }
                onSelectSession={(sessionId) =>
                  handleSelectSession(sessionId, '__adhoc_chat__')
                }
                title={t("sidebar.chat")}
                emptyMessage={t("sidebar.projectsSection.noChats")}
                processingBySession={processingBySession}
                onDeleteSession={(sessionId) =>
                  handleDeleteSession(sessionId, '__adhoc_chat__')
                }
                paddingLeft="16px"
              />
            </div>
          </div>
        )}
      </div>
    </div>

    {/* Bottom */}
      <div className="sidebar-bottom">
        <div className="nav-item">
          <SmartphoneIcon size={14} /> {t("sidebar.nav.connectPhone")}
        </div>
        <div className="nav-item" role="button" tabIndex={0} onClick={onOpenSettings} onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onOpenSettings(); } }}>
          <SettingsIcon size={14} /> {t("sidebar.nav.settings")}
        </div>
      </div>

      <CronjobModal isOpen={isCronModalOpen} onClose={() => setIsCronModalOpen(false)} />
      {dialogElement}
    </aside>
  );
});

// ── Folder icon that changes when open/closed ─────────────────────────

function FolderOpenIcon({ isOpen, size }: { isOpen: boolean; size: number }) {
  if (!isOpen) {
    return <FolderIcon size={size} color="var(--text-tertiary)" />;
  }
  // Open folder: use a different color or style
  return <FolderIcon size={size} color="var(--warning)" />;
}
