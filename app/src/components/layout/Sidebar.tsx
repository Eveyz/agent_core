import { memo, useState, useCallback, useRef, useEffect } from 'react';
import { useDispatch, useSelector } from 'react-redux';
import { open } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { RootState } from '../../store';
import { createProject, fetchProjectSessions, deleteProject, renameProject, setActiveProject } from '../../features/project/projectSlice';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import MoreHorizontalIcon from 'lucide-react/dist/esm/icons/more-horizontal.mjs';
import BoxIcon from 'lucide-react/dist/esm/icons/box.mjs';
import LayoutGridIcon from 'lucide-react/dist/esm/icons/layout-grid.mjs';
import MessageSquareIcon from 'lucide-react/dist/esm/icons/message-square.mjs';
import ClockIcon from 'lucide-react/dist/esm/icons/clock.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import SettingsIcon from 'lucide-react/dist/esm/icons/settings.mjs';
import SmartphoneIcon from 'lucide-react/dist/esm/icons/smartphone.mjs';
import BotIcon from 'lucide-react/dist/esm/icons/bot.mjs';
import TrashIcon from 'lucide-react/dist/esm/icons/trash.mjs';
import PencilIcon from 'lucide-react/dist/esm/icons/pencil.mjs';
import ExternalLinkIcon from 'lucide-react/dist/esm/icons/external-link.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';

const flexGap8 = { display: 'flex', gap: '8px' } as const;

function timeAgo(dateStr: string): string {
  if (!dateStr) return '';
  const then = new Date(dateStr).getTime();
  const now = Date.now();
  const diff = now - then;
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return 'now';
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

interface ProjectMenuProps {
  projectId: string;
  projectName: string;
  projectPath: string;
  onDelete: (projectId: string) => void;
  onRename: (projectId: string, newName: string) => void;
}

function ProjectMenu({ projectId, projectName, projectPath, onDelete, onRename }: ProjectMenuProps) {
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [editName, setEditName] = useState(projectName);
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

  const handleRename = () => {
    const name = editName.trim();
    if (name && name !== projectName) {
      onRename(projectId, name);
    }
    setEditing(false);
    setOpen(false);
  };

  const handleOpenExplorer = async () => {
    try {
      await invoke('open_in_explorer', { path: projectPath });
    } catch (e) {
      console.error('Failed to open explorer:', e);
    }
    setOpen(false);
  };

  return (
    <div ref={menuRef} style={{ position: 'relative' }}>
      <button
        className="icon-btn"
        style={{ padding: 0, opacity: 0.5 }}
        onClick={(e) => { e.stopPropagation(); setOpen((s) => !s); }}
        title="More options"
      >
        <MoreHorizontalIcon size={12} />
      </button>
      {open && (
        <div className="dropdown-menu" style={{ right: 0, top: '24px', minWidth: '140px' }}>
          <div className="dropdown-item" onClick={() => { handleOpenExplorer(); }}>
            <ExternalLinkIcon size={12} /> Open in Explorer
          </div>
          <div
            className="dropdown-item"
            onClick={() => {
              setEditing(true);
              setEditName(projectName);
            }}
          >
            <PencilIcon size={12} /> Rename
          </div>
          <div className="dropdown-item dropdown-item-danger" onClick={() => { setOpen(false); onDelete(projectId); }}>
            <TrashIcon size={12} /> Delete
          </div>
        </div>
      )}
      {editing && (
        <div className="dropdown-menu" style={{ right: 0, top: '24px', minWidth: '180px', padding: '8px' }}>
          <input
            className="settings-input"
            style={{ fontSize: '12px', marginBottom: '6px' }}
            value={editName}
            onChange={(e) => setEditName(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter') handleRename(); if (e.key === 'Escape') setEditing(false); }}
            autoFocus
          />
          <div style={{ display: 'flex', gap: '6px', justifyContent: 'flex-end' }}>
            <button className="btn-secondary" style={{ fontSize: '11px', padding: '2px 8px' }} onClick={() => setEditing(false)}>Cancel</button>
            <button className="btn-primary" style={{ fontSize: '11px', padding: '2px 8px' }} onClick={handleRename}>Save</button>
          </div>
        </div>
      )}
    </div>
  );
}

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
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(new Set());

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
    dispatch(renameProject({ projectId, newName }) as any);
  }, [dispatch]);

  const handleSelectProject = useCallback((projectId: string) => {
    dispatch(setActiveProject(projectId));
  }, [dispatch]);

  return (
    <aside className="sidebar">
      <div className="sidebar-header-actions">
        <button className="icon-btn"><LayoutGridIcon size={16} /></button>
      </div>

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

      <div className="sidebar-nav">
        <div className="nav-item"><PlusIcon size={14} /> New Agent</div>
        <div className="nav-item"><MessageSquareIcon size={14} /> New requirement</div>
        <div className="nav-item"><BoxIcon size={14} /> Plugins</div>
        <div className="nav-item"><ClockIcon size={14} /> Scheduled tasks</div>
      </div>

      <div className="projects-header">
        <span>Projects</span>
        <div style={flexGap8}>
          <BoxIcon size={12} />
          <button className="icon-btn" style={{ padding: 0 }} onClick={handleOpenFolder} title="Open project folder">
            <FolderIcon size={12} />
          </button>
        </div>
      </div>

      <div className="sidebar-nav" style={{ marginTop: '8px', overflowY: 'auto', flex: 1 }}>
        {projects.length === 0 && (
          <div style={{ padding: '12px', color: '#808080', fontSize: '12px' }}>
            No projects yet. Click the folder icon to add one.
          </div>
        )}
        {projects.map((project) => {
          const isExpanded = expandedProjects.has(project.id);
          const isActive = activeProjectId === project.id;
          const projectSessions = sessions[project.id] ?? [];

          return (
            <div key={project.id}>
              <div
                className={`project-item ${isActive ? 'project-item-active' : ''}`}
                onClick={() => { handleSelectProject(project.id); toggleProject(project.id); }}
                style={{ cursor: 'pointer' }}
              >
                <span style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                  {isExpanded ? <ChevronDownIcon size={12} /> : <ChevronRightIcon size={12} />}
                  <FolderIcon size={14} color="#808080" />
                  {project.name}
                </span>
                <span style={flexGap8}>
                  <ProjectMenu
                    projectId={project.id}
                    projectName={project.name}
                    projectPath={project.path}
                    onDelete={handleDeleteProject}
                    onRename={handleRenameProject}
                  />
                  <button
                    className="icon-btn"
                    style={{ padding: 0, opacity: 0.5 }}
                    onClick={(e) => { e.stopPropagation(); /* new session */ }}
                    title="New session"
                  >
                    <MessageSquareIcon size={10} />
                  </button>
                </span>
              </div>

              {isExpanded && (
                <div style={{ marginLeft: '8px' }}>
                  {projectSessions.length === 0 && (
                    <div style={{ padding: '6px 12px', color: '#555', fontSize: '11px' }}>
                      No sessions yet
                    </div>
                  )}
                  {projectSessions.map((session) => (
                    <div
                      key={session.id}
                      className="project-item"
                      style={{ paddingLeft: '32px', fontSize: '12px' }}
                    >
                      <span style={{ display: 'flex', alignItems: 'center', gap: '8px', overflow: 'hidden' }}>
                        <MessageSquareIcon size={10} color="#555" />
                        <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                          {session.title}
                        </span>
                      </span>
                      <span className="meta">{timeAgo(session.updated_at)}</span>
                    </div>
                  ))}
                </div>
              )}
            </div>
          );
        })}
      </div>

      <div className="sidebar-bottom">
        <div className="nav-item"><SmartphoneIcon size={14} /> Connect phone</div>
        <div className="nav-item" onClick={onOpenSettings}><SettingsIcon size={14} /> Settings</div>
      </div>
    </aside>
  );
});
