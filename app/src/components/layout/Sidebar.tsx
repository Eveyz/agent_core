import { memo, useState, useCallback, useRef, useEffect } from 'react';
import { useDispatch, useSelector } from 'react-redux';
import { open } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { RootState } from '../../store';
import {
  createProject, fetchProjectSessions, deleteProject, renameProject,
  setActiveProject, setActiveSession,
  createSession, deleteSession, resumeSession,
  saveSessionMessages,
} from '../../features/project/projectSlice';
import { clearChat, cacheCurrentSession, restoreOrClearSession, entriesToMessages } from '../../features/chat/chatSlice';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import LayoutGridIcon from 'lucide-react/dist/esm/icons/layout-grid.mjs';
import MessageSquareIcon from 'lucide-react/dist/esm/icons/message-square.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import SettingsIcon from 'lucide-react/dist/esm/icons/settings.mjs';
import SmartphoneIcon from 'lucide-react/dist/esm/icons/smartphone.mjs';
import BotIcon from 'lucide-react/dist/esm/icons/bot.mjs';
import TrashIcon from 'lucide-react/dist/esm/icons/trash.mjs';
import PencilIcon from 'lucide-react/dist/esm/icons/pencil.mjs';
import ExternalLinkIcon from 'lucide-react/dist/esm/icons/external-link.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import MoreHorizontalIcon from 'lucide-react/dist/esm/icons/more-horizontal.mjs';

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
      document.addEventListener('mousedown', handleClick);
      return () => document.removeEventListener('mousedown', handleClick);
    }
  }, [open]);

  return { open, setOpen, pos, setPos, menuRef };
}

// ── Project context menu ─────────────────────────────────────────────

function ProjectContextMenu({ projectId, projectName, projectPath, onDelete, onRename }: {
  projectId: string; projectName: string; projectPath: string;
  onDelete: (id: string) => void;
  onRename: (id: string, name: string) => void;
}) {
  const { open, setOpen, pos, setPos, menuRef } = useContextMenu();

  const handleOpenExplorer = async () => {
    try { await invoke('open_in_explorer', { path: projectPath }); } catch {}
    setOpen(false);
  };

  return (
    <>
      <button
        className="sidebar-context-trigger"
        onClick={(e) => { e.stopPropagation(); setPos({ x: e.clientX, y: e.clientY }); setOpen(true); }}
      >
        <MoreHorizontalIcon size={14} />
      </button>
      {open && (
        <div ref={menuRef} className="context-menu" style={{ left: pos.x, top: pos.y }}>
          <div className="context-menu-item" onClick={() => { onRename(projectId, projectName); setOpen(false); }}>
            <PencilIcon size={13} /> Rename
          </div>
          <div className="context-menu-item" onClick={handleOpenExplorer}>
            <ExternalLinkIcon size={13} /> Open in Explorer
          </div>
          <div className="context-menu-separator" />
          <div className="context-menu-item context-menu-danger" onClick={() => { setOpen(false); onDelete(projectId); }}>
            <TrashIcon size={13} /> Delete
          </div>
        </div>
      )}
    </>
  );
}

// ── Session context menu ─────────────────────────────────────────────

function SessionContextMenu({ sessionId, projectId, onDelete }: {
  sessionId: string; projectId: string;
  onDelete: (sessionId: string, projectId: string) => void;
}) {
  const { open, setOpen, pos, setPos, menuRef } = useContextMenu();

  return (
    <>
      <button
        className="sidebar-context-trigger"
        onClick={(e) => { e.stopPropagation(); setPos({ x: e.clientX, y: e.clientY }); setOpen(true); }}
      >
        <MoreHorizontalIcon size={12} />
      </button>
      {open && (
        <div ref={menuRef} className="context-menu" style={{ left: pos.x, top: pos.y }}>
          <div className="context-menu-item context-menu-danger" onClick={() => { setOpen(false); onDelete(sessionId, projectId); }}>
            <TrashIcon size={13} /> Delete
          </div>
        </div>
      )}
    </>
  );
}

// ── Sidebar ──────────────────────────────────────────────────────────

export const Sidebar = memo(function Sidebar({
  activeTab,
  onTabChange,
  onOpenSettings,
}: {
  activeTab: 'code' | 'write';
  onTabChange: (tab: 'code' | 'write') => void;
  onOpenSettings: () => void;
}) {
  const dispatch = useDispatch();
  const projects = useSelector((state: RootState) => state.project.projects);
  const sessions = useSelector((state: RootState) => state.project.sessions);
  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);
  const chatEntries = useSelector((state: RootState) => state.chat.entries);
  const defaultModel = useSelector((state: RootState) => state.settings.config?.default_model || '');
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(new Set());
  const [creatingSession, setCreatingSession] = useState(false);

  // Auto-expand active project
  useEffect(() => {
    if (activeProjectId) {
      setExpandedProjects((prev) => {
        if (prev.has(activeProjectId)) return prev;
        const next = new Set(prev);
        next.add(activeProjectId);
        dispatch(fetchProjectSessions(activeProjectId) as any);
        return next;
      });
    }
  }, [activeProjectId, dispatch]);

  const toggleProject = useCallback((projectId: string) => {
    setExpandedProjects((prev) => {
      const next = new Set(prev);
      if (next.has(projectId)) {
        next.delete(projectId);
      } else {
        next.add(projectId);
        dispatch(fetchProjectSessions(projectId) as any);
      }
      return next;
    });
  }, [dispatch]);

  const handleOpenFolder = useCallback(async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected && typeof selected === 'string') {
      dispatch(createProject(selected) as any);
    }
  }, [dispatch]);

  const handleDeleteProject = useCallback((projectId: string) => {
    if (confirm('Delete this project and all its sessions?')) {
      dispatch(deleteProject(projectId) as any);
    }
  }, [dispatch]);

  const handleRenameProject = useCallback((projectId: string, newName: string) => {
    const name = prompt('New name:', newName);
    if (name?.trim()) dispatch(renameProject({ projectId, newName: name.trim() }) as any);
  }, [dispatch]);

  const handleSelectProject = useCallback((projectId: string) => {
    dispatch(setActiveProject(projectId));
  }, [dispatch]);

  // Save current session to backend + memory cache
  const saveAndCacheCurrent = useCallback(() => {
    if (!activeSessionId || !activeProjectId) return;
    const msgs = entriesToMessages(chatEntries);
    if (msgs.length > 0) {
      const project = projects.find((p) => p.id === activeProjectId);
      if (project) {
        dispatch(saveSessionMessages({
          sessionId: activeSessionId,
          messages: msgs,
          cwd: project.path,
          modelUsed: defaultModel,
        }) as any);
      }
    }
    dispatch(cacheCurrentSession(activeSessionId));
  }, [dispatch, activeSessionId, activeProjectId, chatEntries, projects, defaultModel]);

  const handleNewSession = useCallback(async (projectId: string) => {
    if (creatingSession) return;
    setCreatingSession(true);
    try {
      saveAndCacheCurrent();
      await dispatch(createSession(projectId) as any);
      dispatch(clearChat());
    } finally {
      setCreatingSession(false);
    }
  }, [dispatch, creatingSession, saveAndCacheCurrent]);

  const handleSelectSession = useCallback((sessionId: string, projectId: string) => {
    if (activeSessionId && activeSessionId !== sessionId) {
      saveAndCacheCurrent();
    }
    dispatch(setActiveProject(projectId));
    dispatch(setActiveSession(sessionId));
    dispatch(restoreOrClearSession(sessionId));
    dispatch(resumeSession(sessionId) as any);
  }, [dispatch, activeSessionId, saveAndCacheCurrent]);

  const handleDeleteSession = useCallback((sessionId: string, projectId: string) => {
    if (confirm('Delete this session?')) {
      dispatch(deleteSession({ sessionId, projectId }) as any);
    }
  }, [dispatch]);

  return (
    <aside className="sidebar">
      {/* Header */}
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '10px 12px 6px' }}>
        <button className="icon-btn"><LayoutGridIcon size={16} /></button>
        <div style={{ display: 'flex', gap: '4px' }}>
          <button className="icon-btn" onClick={handleOpenFolder} title="Open folder">
            <FolderIcon size={15} />
          </button>
        </div>
      </div>

      {/* Toggle group */}
      <div className="toggle-group">
        <button
          className={`toggle-btn ${activeTab === 'code' ? 'active' : ''}`}
          onClick={() => onTabChange('code')}
        >
          <BotIcon size={14} /> Code
        </button>
        <button
          className={`toggle-btn ${activeTab === 'write' ? 'active' : ''}`}
          onClick={() => onTabChange('write')}
        >
          <MessageSquareIcon size={14} /> Write
        </button>
      </div>

      {/* Quick actions */}
      <div className="sidebar-nav" style={{ marginBottom: '4px' }}>
        <div className="nav-item"><PlusIcon size={14} /> New Agent</div>
        <div className="nav-item"><MessageSquareIcon size={14} /> New requirement</div>
      </div>

      {/* Projects list */}
      <div className="projects-section">
        <div className="projects-header">
          <span>Projects</span>
          <button className="icon-btn" onClick={handleOpenFolder} title="Import project">
            <PlusIcon size={13} />
          </button>
        </div>

        <div className="projects-list">
          {projects.length === 0 && (
            <div style={{ padding: '16px 20px', color: '#666', fontSize: '12px' }}>
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
                  onClick={() => { handleSelectProject(project.id); toggleProject(project.id); }}
                >
                  <span className="sidebar-row-content">
                    {isExpanded
                      ? <ChevronDownIcon size={14} className="sidebar-chevron" />
                      : <ChevronRightIcon size={14} className="sidebar-chevron" />
                    }
                    <FolderOpenIcon isOpen={isExpanded} size={15} />
                    <span className="sidebar-row-text">{project.name}</span>
                  </span>
                  <span className="sidebar-row-actions">
                    <button
                      className="sidebar-context-trigger"
                      onClick={(e) => { e.stopPropagation(); handleNewSession(project.id); }}
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
                {isExpanded && (
                  <div className="session-list">
                    {projectSessions.map((session) => {
                      const isSessionActive = activeSessionId === session.id && activeProjectId === project.id;
                      return (
                        <div
                          key={session.id}
                          className={`session-row ${isSessionActive ? 'session-row-active' : ''}`}
                          onClick={() => handleSelectSession(session.id, project.id)}
                        >
                          <span className="session-row-text">{session.title || 'Untitled'}</span>
                          <span className="session-row-actions">
                            <SessionContextMenu
                              sessionId={session.id}
                              projectId={project.id}
                              onDelete={handleDeleteSession}
                            />
                            <button
                              className="sidebar-context-trigger session-delete-btn"
                              onClick={(e) => { e.stopPropagation(); handleDeleteSession(session.id, project.id); }}
                              title="Delete session"
                            >
                              <TrashIcon size={12} />
                            </button>
                          </span>
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>

      {/* Bottom */}
      <div className="sidebar-bottom">
        <div className="nav-item"><SmartphoneIcon size={14} /> Connect phone</div>
        <div className="nav-item" onClick={onOpenSettings}><SettingsIcon size={14} /> Settings</div>
      </div>
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
