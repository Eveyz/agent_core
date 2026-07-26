import { memo, useState, useCallback, useRef, useEffect, useMemo } from "react";
import { useSelector } from "react-redux";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { RootState } from "../../store";
import {
  createProject,
  createNewProject,
  fetchProjectSessions,
  deleteProject,
  renameProject,
  setActiveProject,
  setActiveSession,
  createSession,
  deleteSession,
  setProjectPinned,
  setSessionPinned,
  Project,
  SessionMeta,
} from "../../features/project/projectSlice";
import { selectIsResumingActive } from "../../features/chat/chatSlice";
import { useAppDispatch } from "../../hooks/useAppDispatch";
import { useConfirmDialog } from "../ui/DialogManager";
import { NewProjectDialog } from "../ui/NewProjectDialog";
import { SessionList } from "./SessionList";
import PlusIcon from "lucide-react/dist/esm/icons/plus.mjs";
import MessageSquareIcon from "lucide-react/dist/esm/icons/message-square.mjs";
import FolderIcon from "lucide-react/dist/esm/icons/folder.mjs";
import FolderPlusIcon from "lucide-react/dist/esm/icons/folder-plus.mjs";
import FolderOpenIconLucide from "lucide-react/dist/esm/icons/folder-open.mjs";
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
import PinIcon from "lucide-react/dist/esm/icons/pin.mjs";
import PinOffIcon from "lucide-react/dist/esm/icons/pin-off.mjs";
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

type PinnedItem =
  | { kind: "project"; project: Project; pinnedAt: string }
  | { kind: "session"; session: SessionMeta; projectId: string; pinnedAt: string };

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
  const isResuming = useSelector(selectIsResumingActive);
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(
    new Set(),
  );
  const [creatingSession, setCreatingSession] = useState(false);
  const [isCronModalOpen, setIsCronModalOpen] = useState(false);
  const [projectsCollapsed, setProjectsCollapsed] = useState(false);
  const [pinnedCollapsed, setPinnedCollapsed] = useState(false);
  const [chatCollapsed, setChatCollapsed] = useState(false);
  const [addMenuOpen, setAddMenuOpen] = useState(false);
  const [newProjectOpen, setNewProjectOpen] = useState(false);
  const [creatingProject, setCreatingProject] = useState(false);
  const addMenuRef = useRef<HTMLDivElement>(null);
  const { confirm, prompt, dialogElement } = useConfirmDialog();

  useEffect(() => {
    if (!addMenuOpen) return;
    function handleClick(e: MouseEvent) {
      if (addMenuRef.current && !addMenuRef.current.contains(e.target as Node)) {
        setAddMenuOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, [addMenuOpen]);

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
    dispatch(fetchProjectSessions("__adhoc_chat__"));
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

  const handleCreateNewProject = useCallback(
    async (name: string, path: string) => {
      setCreatingProject(true);
      try {
        await dispatch(createNewProject({ name, path })).unwrap();
        setNewProjectOpen(false);
      } catch {
        // Keep dialog open; error surfaces via project slice if needed
      } finally {
        setCreatingProject(false);
      }
    },
    [dispatch],
  );

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

  const handleToggleProjectPin = useCallback(
    (projectId: string, currentlyPinned: boolean) => {
      dispatch(setProjectPinned({ projectId, pinned: !currentlyPinned }));
    },
    [dispatch],
  );

  const handleToggleSessionPin = useCallback(
    (sessionId: string, projectId: string, currentlyPinned: boolean) => {
      dispatch(
        setSessionPinned({
          sessionId,
          projectId,
          pinned: !currentlyPinned,
        }),
      );
    },
    [dispatch],
  );

  const handleNewSession = useCallback(
    async (projectId: string) => {
      if (creatingSession) return;
      setCreatingSession(true);
      try {
        await dispatch(createSession(projectId));
      } finally {
        setCreatingSession(false);
      }
    },
    [dispatch, creatingSession],
  );

  const handleSelectSession = useCallback(
    (sessionId: string, projectId: string) => {
      dispatch(setActiveProject(projectId));
      dispatch(setActiveSession(sessionId));
      // Always navigate to chat — re-clicking the already-active session does not
      // change activeSessionId, so App's [activeSessionId] effect would not fire.
      onNavigate?.("chat");
    },
    [dispatch, onNavigate],
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

  const unpinnedProjects = useMemo(
    () => projects.filter((p) => p.id !== "__adhoc_chat__" && !p.pinned),
    [projects],
  );

  const pinnedItems = useMemo(() => {
    const items: PinnedItem[] = [];
    for (const project of projects) {
      if (project.id === "__adhoc_chat__") continue;
      if (project.pinned) {
        items.push({
          kind: "project",
          project,
          pinnedAt: project.pinned_at || project.updated_at,
        });
      }
    }
    for (const [projectId, list] of Object.entries(sessions)) {
      for (const session of list) {
        if (session.pinned) {
          items.push({
            kind: "session",
            session,
            projectId,
            pinnedAt: session.pinned_at || session.updated_at,
          });
        }
      }
    }
    items.sort((a, b) => {
      const diff =
        new Date(b.pinnedAt).getTime() - new Date(a.pinnedAt).getTime();
      if (diff !== 0) return diff;
      const aId = a.kind === "project" ? a.project.id : a.session.id;
      const bId = b.kind === "project" ? b.project.id : b.session.id;
      return bId.localeCompare(aId);
    });
    return items;
  }, [projects, sessions]);

  const renderProjectGroup = (project: Project) => {
    const isExpanded = expandedProjects.has(project.id);
    const projectSessions = (sessions[project.id] ?? []).filter((s) => !s.pinned);
    const isPinned = Boolean(project.pinned);

    return (
      <div key={project.id} className="project-group">
        <div
          className="sidebar-project-row"
          onClick={() => {
            toggleProject(project.id);
          }}
        >
          <span className="sidebar-row-content">
            <FolderOpenIcon isOpen={isExpanded} size={15} />
            <span className="sidebar-row-text">{project.name}</span>
          </span>
          <span className="sidebar-row-actions">
            <button
              className="sidebar-context-trigger sidebar-pin-btn"
              onClick={(e) => {
                e.stopPropagation();
                handleToggleProjectPin(project.id, isPinned);
              }}
              title={isPinned ? t("sidebar.actions.unpin") : t("sidebar.actions.pin")}
            >
              {isPinned ? <PinOffIcon size={13} /> : <PinIcon size={13} />}
            </button>
            <button
              className="sidebar-context-trigger"
              onClick={(e) => {
                e.stopPropagation();
                handleNewSession(project.id);
              }}
              title={t("sidebar.actions.newSession")}
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
              onTogglePinSession={(sessionId) => {
                const session = (sessions[project.id] ?? []).find(
                  (s) => s.id === sessionId,
                );
                handleToggleSessionPin(
                  sessionId,
                  project.id,
                  Boolean(session?.pinned),
                );
              }}
            />
          </div>
        </div>
      </div>
    );
  };

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
        {/* Pinned section */}
        {pinnedItems.length > 0 && (
          <div className={`projects-section ${pinnedCollapsed ? "section-collapsed" : ""}`}>
            <div
              className="projects-header"
              role="button"
              tabIndex={0}
              aria-expanded={!pinnedCollapsed}
              onClick={() => setPinnedCollapsed(!pinnedCollapsed)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  setPinnedCollapsed(!pinnedCollapsed);
                }
              }}
            >
              <span style={{ display: "flex", alignItems: "center", gap: "6px" }}>
                {pinnedCollapsed ? (
                  <ChevronRightIcon size={12} style={{ opacity: 0.7 }} />
                ) : (
                  <ChevronDownIcon size={12} style={{ opacity: 0.7 }} />
                )}
                {t("sidebar.pinned")}
              </span>
            </div>

            {!pinnedCollapsed && (
              <div className="projects-list">
                {pinnedItems.map((item) => {
                  if (item.kind === "project") {
                    return renderProjectGroup(item.project);
                  }
                  const { session, projectId } = item;
                  return (
                    <div key={`pinned-session-${session.id}`} className="pinned-session-wrap">
                      <SessionList
                        sessions={[session]}
                        activeSessionId={
                          activeView === "chat" && activeSessionId === session.id
                            ? activeSessionId
                            : null
                        }
                        onSelectSession={(sessionId) =>
                          handleSelectSession(sessionId, projectId)
                        }
                        title={session.title}
                        emptyMessage=""
                        processingBySession={processingBySession}
                        onDeleteSession={(sessionId) =>
                          handleDeleteSession(sessionId, projectId)
                        }
                        onTogglePinSession={(sessionId) =>
                          handleToggleSessionPin(sessionId, projectId, true)
                        }
                        paddingLeft="16px"
                      />
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        )}

        {/* Projects list */}
        <div
          className={`projects-section ${projectsCollapsed ? "section-collapsed" : ""}`}
          style={pinnedItems.length > 0 ? { marginTop: "12px" } : undefined}
        >
          <div
            className="projects-header"
            role="button"
            tabIndex={0}
            aria-expanded={!projectsCollapsed}
            onClick={() => setProjectsCollapsed(!projectsCollapsed)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                setProjectsCollapsed(!projectsCollapsed);
              }
            }}
          >
            <span style={{ display: "flex", alignItems: "center", gap: "6px" }}>
              {projectsCollapsed ? (
                <ChevronRightIcon size={12} style={{ opacity: 0.7 }} />
              ) : (
                <ChevronDownIcon size={12} style={{ opacity: 0.7 }} />
              )}
              {t("sidebar.projects")}
            </span>
            <div className="projects-add-wrap" ref={addMenuRef}>
              <button
                className="icon-btn"
                onClick={(e) => {
                  e.stopPropagation();
                  setAddMenuOpen((v) => !v);
                }}
                title={t("sidebar.actions.addProject")}
                aria-haspopup="menu"
                aria-expanded={addMenuOpen}
              >
                <PlusIcon size={13} />
              </button>
              {addMenuOpen && (
                <div
                  className="projects-add-menu"
                  role="menu"
                  onClick={(e) => e.stopPropagation()}
                >
                  <button
                    className="projects-add-menu-item"
                    role="menuitem"
                    onClick={() => {
                      setAddMenuOpen(false);
                      setNewProjectOpen(true);
                    }}
                  >
                    <FolderPlusIcon size={14} />
                    {t("sidebar.actions.newProject")}
                  </button>
                  <button
                    className="projects-add-menu-item"
                    role="menuitem"
                    onClick={() => {
                      setAddMenuOpen(false);
                      handleOpenFolder();
                    }}
                  >
                    <FolderOpenIconLucide size={14} />
                    {t("sidebar.actions.existingProjectFolder")}
                  </button>
                </div>
              )}
            </div>
          </div>

          {!projectsCollapsed && (
            <div className="projects-list">
              {unpinnedProjects.length === 0 && (
                <div
                  style={{ padding: "16px 20px", color: "var(--text-tertiary)", fontSize: "12px" }}
                >
                  {t("sidebar.projectsSection.noProjects")}
                </div>
              )}
              {unpinnedProjects.map((project) => renderProjectGroup(project))}
            </div>
          )}
        </div>

        {/* Chat section */}
        <div className={`projects-section ${chatCollapsed ? "section-collapsed" : ""}`} style={{ marginTop: "12px" }}>
          <div
            className="projects-header"
            role="button"
            tabIndex={0}
            aria-expanded={!chatCollapsed}
            onClick={() => setChatCollapsed(!chatCollapsed)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                setChatCollapsed(!chatCollapsed);
              }
            }}
          >
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
                handleNewSession("__adhoc_chat__");
              }}
              title={t("sidebar.actions.newChat")}
            >
              <PlusIcon size={13} />
            </button>
          </div>

          {!chatCollapsed && (
            <div className="projects-list">
              <div className="session-list">
                <SessionList
                  sessions={(sessions["__adhoc_chat__"] ?? []).filter((s) => !s.pinned)}
                  activeSessionId={
                    activeView === "chat" && activeProjectId === "__adhoc_chat__"
                      ? activeSessionId
                      : null
                  }
                  onSelectSession={(sessionId) =>
                    handleSelectSession(sessionId, "__adhoc_chat__")
                  }
                  title={t("sidebar.chat")}
                  emptyMessage={t("sidebar.projectsSection.noChats")}
                  processingBySession={processingBySession}
                  onDeleteSession={(sessionId) =>
                    handleDeleteSession(sessionId, "__adhoc_chat__")
                  }
                  onTogglePinSession={(sessionId) => {
                    const session = (sessions["__adhoc_chat__"] ?? []).find(
                      (s) => s.id === sessionId,
                    );
                    handleToggleSessionPin(
                      sessionId,
                      "__adhoc_chat__",
                      Boolean(session?.pinned),
                    );
                  }}
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
        <div
          className="nav-item"
          role="button"
          tabIndex={0}
          onClick={onOpenSettings}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              onOpenSettings();
            }
          }}
        >
          <SettingsIcon size={14} /> {t("sidebar.nav.settings")}
        </div>
      </div>

      <CronjobModal isOpen={isCronModalOpen} onClose={() => setIsCronModalOpen(false)} />
      <NewProjectDialog
        open={newProjectOpen}
        onClose={() => setNewProjectOpen(false)}
        onCreate={handleCreateNewProject}
        creating={creatingProject}
      />
      {dialogElement}
    </aside>
  );
});

// ── Folder icon that changes when open/closed ─────────────────────────

function FolderOpenIcon({ isOpen, size }: { isOpen: boolean; size: number }) {
  if (!isOpen) {
    return <FolderIcon size={size} color="var(--text-tertiary)" />;
  }
  return <FolderOpenIconLucide size={size} color="var(--text-tertiary)" />;
}
