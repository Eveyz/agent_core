import { useState, useMemo } from 'react';
import { useSelector } from 'react-redux';
import { RootState } from '../../store';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import BotIcon from 'lucide-react/dist/esm/icons/bot.mjs';
import BookOpenIcon from 'lucide-react/dist/esm/icons/book-open.mjs';
import FileTextIcon from 'lucide-react/dist/esm/icons/file-text.mjs';
import CheckSquareIcon from 'lucide-react/dist/esm/icons/check-square.mjs';
import ImageIcon from 'lucide-react/dist/esm/icons/image.mjs';
import FileIcon from 'lucide-react/dist/esm/icons/file.mjs';
import { basename } from '../chat/turnHelpers';
import { getFileIcon } from '../layout/FileTree';

type SectionKey = 'subagent' | 'files_changed' | 'background_tasks' | 'artifacts';

export function OverviewTab() {
  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const activeProject = projects.find((p) => p.id === activeProjectId);
  const entries = useSelector((state: RootState) => state.chat.entries);
  const todo = useSelector((state: RootState) => state.chat.todo);
  const subagentsBySession = useSelector((state: RootState) => state.chat.subagentsBySession);
  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);

  const [expanded, setExpanded] = useState<Set<SectionKey>>(
    new Set(['files_changed', 'background_tasks'])
  );
  const [showAllFiles, setShowAllFiles] = useState(false);

  const toggleSection = (key: SectionKey) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  // Extract active subagents
  const subagents = useMemo(() => {
    if (!activeSessionId) return [];
    return Object.values(subagentsBySession[activeSessionId] || {});
  }, [subagentsBySession, activeSessionId]);

  // Extract modified files
  const modifiedFiles = useMemo(() => {
    const files = new Map<string, string>();
    for (const entry of entries) {
      if (entry.type !== 'turn' || !entry.blocks) continue;
      for (const block of entry.blocks) {
        if (block.type === 'tool' && !block.is_error && block.result) {
          const name = block.name;
          if (
            name === 'edit_file' ||
            name === 'replace_file_content' ||
            name === 'multi_replace_file_content' ||
            name === 'write_to_file' ||
            name === 'edit' ||
            name === 'write_file'
          ) {
            const args = block.args as any;
            const path = args?.file_path || args?.TargetFile || args?.path;
            if (path && !files.has(path)) {
              files.set(path, basename(path));
            }
          }
        }
      }
    }
    return Array.from(files.entries()).map(([path, name]) => ({ path, name }));
  }, [entries]);

  // Extract artifacts (only system documentation, media, or files in the chats folder)
  const artifacts = useMemo(() => {
    const files = new Map<string, string>();
    const systemDocs = ['PLAN.md', 'plan.md', 'implementation_plan.md', 'walkthrough.md', 'task.md'];
    const mediaExts = ['png', 'jpg', 'jpeg', 'webp', 'gif'];

    for (const entry of entries) {
      if (entry.type !== 'turn' || !entry.blocks) continue;
      for (const block of entry.blocks) {
        if (
          block.type === 'tool' && 
          !block.is_error && 
          block.result && 
          (block.name === 'write_to_file' || block.name === 'write_file')
        ) {
          const args = block.args as any;
          const path = args?.TargetFile || args?.file_path || args?.path;
          if (path) {
            const filename = basename(path);
            const ext = filename.split('.').pop()?.toLowerCase() || '';
            const isSysDoc = systemDocs.includes(filename);
            const isMedia = mediaExts.includes(ext);
            const isUnderChatsDir = path.includes('.agverse/chats/') || path.includes('.agverse\\chats\\');
            
            if ((isSysDoc || isMedia || isUnderChatsDir) && !files.has(path)) {
              files.set(path, filename);
            }
          }
        }
      }
    }
    return Array.from(files.entries()).map(([path, name]) => ({ path, name }));
  }, [entries]);

  const getArtifactDetails = (name: string) => {
    const nameLower = name.toLowerCase();
    if (nameLower === 'walkthrough.md') {
      return {
        displayName: 'Walkthrough',
        icon: <BookOpenIcon size={14} style={{ color: 'var(--text-dim)' }} />
      };
    }
    if (nameLower === 'implementation_plan.md' || nameLower === 'plan.md') {
      return {
        displayName: 'Implementation Plan',
        icon: <FileTextIcon size={14} style={{ color: 'var(--text-dim)' }} />
      };
    }
    if (nameLower === 'task.md') {
      return {
        displayName: 'Task',
        icon: <CheckSquareIcon size={14} style={{ color: 'var(--text-dim)' }} />
      };
    }
    if (/\.(png|jpg|jpeg|webp|gif)$/i.test(nameLower)) {
      // Try to parse timestamp from name
      const tsMatch = name.match(/(\d{10})/);
      let dateStr = '';
      if (tsMatch) {
        const date = new Date(parseInt(tsMatch[1]) * 1000);
        const isToday = date.toDateString() === new Date().toDateString();
        let hours = date.getHours();
        const minutes = String(date.getMinutes()).padStart(2, '0');
        const ampm = hours >= 12 ? 'PM' : 'AM';
        hours = hours % 12;
        hours = hours ? hours : 12;
        const timeStr = `${hours}:${minutes} ${ampm}`;
        dateStr = ` (${isToday ? 'Today' : `${date.getMonth() + 1}/${date.getDate()}/${date.getFullYear()}`} ${timeStr})`;
      }
      return {
        displayName: `Media${dateStr}`,
        icon: <ImageIcon size={14} style={{ color: 'var(--text-dim)' }} />
      };
    }
    // Default fallback
    const cleanExt = name.replace(/\.[^/.]+$/, "");
    const displayName = cleanExt.split(/[-_]+/).map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(' ');
    return {
      displayName,
      icon: <FileIcon size={14} style={{ color: 'var(--text-dim)' }} />
    };
  };

  const sectionHeader = (key: SectionKey, label: string, count?: number) => (
    <div className="overview-section-header" onClick={() => toggleSection(key)}>
      <span className="overview-section-label-group">
        <span className="overview-section-label">{label}</span>
        {count !== undefined && (
          <span className="overview-section-badge">{count}</span>
        )}
      </span>
      <span className="overview-section-chevron">
        {expanded.has(key) ? <ChevronDownIcon size={14} /> : <ChevronRightIcon size={14} />}
      </span>
    </div>
  );

  const statusDot = (status: string) => {
    const map: Record<string, string> = {
      pending: 'var(--text-muted)',
      in_progress: '#fbbf24',
      completed: '#34d399',
      blocked: '#f87171',
    };
    return (
      <span
        className="todo-status-dot"
        style={{
          width: 8,
          height: 8,
          borderRadius: '50%',
          backgroundColor: map[status] || 'var(--text-muted)',
          flexShrink: 0,
        }}
      />
    );
  };

  const visibleFiles = showAllFiles ? modifiedFiles : modifiedFiles.slice(0, 6);

  return (
    <div className="overview-tab-container">
      <div className="overview-body">
        {!activeProject ? (
          <div className="empty-message">No active project</div>
        ) : (
          <div className="overview-sections">
            {/* ── Subagent ──────────────────────────────────────── */}
            <div className="overview-section">
              {sectionHeader('subagent', 'Subagents', subagents.length)}
              {expanded.has('subagent') && (
                <div className="overview-section-body">
                  {subagents.length === 0 ? (
                    <div className="overview-placeholder">No subagents active</div>
                  ) : (
                    subagents.map((sa: any) => (
                      <div key={sa.id} className="overview-file-row" title={sa.id}>
                        <span className="overview-file-icon">
                          <BotIcon size={14} style={{ color: 'var(--accent, #006efe)' }} />
                        </span>
                        <span className="overview-file-name">{sa.name || sa.role || 'Subagent'}</span>
                        <span className="overview-file-path">{sa.role}</span>
                      </div>
                    ))
                  )}
                </div>
              )}
            </div>

            {/* ── Files Changed ─────────────────────────────────── */}
            <div className="overview-section">
              {sectionHeader('files_changed', 'Files Changed', modifiedFiles.length)}
              {expanded.has('files_changed') && (
                <div className="overview-section-body">
                  {modifiedFiles.length === 0 ? (
                    <div className="overview-placeholder">No files changed yet</div>
                  ) : (
                    <>
                      {visibleFiles.map((file) => (
                        <div key={file.path} className="overview-file-row" title={file.path}>
                          <span className="overview-file-icon">
                            {getFileIcon(file.name)}
                          </span>
                          <span className="overview-file-name">{file.name}</span>
                          <span className="overview-file-path">{file.path}</span>
                        </div>
                      ))}
                      {modifiedFiles.length > 6 && (
                        <div 
                          className="see-more-link" 
                          onClick={() => setShowAllFiles(!showAllFiles)}
                          style={{
                            padding: '6px 16px',
                            fontSize: '12px',
                            color: 'var(--text-dim)',
                            cursor: 'pointer',
                            fontWeight: 500,
                            transition: 'color 0.15s'
                          }}
                          onMouseEnter={(e) => (e.currentTarget.style.color = 'var(--text-main)')}
                          onMouseLeave={(e) => (e.currentTarget.style.color = 'var(--text-dim)')}
                        >
                          {showAllFiles ? 'See less' : `See all (${modifiedFiles.length})`}
                        </div>
                      )}
                    </>
                  )}
                </div>
              )}
            </div>

            {/* ── Tasks ──────────────────────────────────────────── */}
            <div className="overview-section">
              {sectionHeader('background_tasks', 'Background Tasks', todo.length)}
              {expanded.has('background_tasks') && (
                <div className="overview-section-body">
                  {todo.length === 0 ? (
                    <div className="overview-placeholder">No tasks yet</div>
                  ) : (
                    todo.map((item) => (
                      <div key={item.id} className="overview-task-row">
                        {statusDot(item.status)}
                        <span
                          className={`overview-task-text ${
                            item.status === 'completed' ? 'overview-task-done' : ''
                          }`}
                        >
                          {item.description}
                        </span>
                      </div>
                    ))
                  )}
                </div>
              )}
            </div>

            {/* ── Artifacts ──────────────────────────────────────── */}
            <div className="overview-section">
              {sectionHeader('artifacts', 'Artifacts', artifacts.length)}
              {expanded.has('artifacts') && (
                <div className="overview-section-body">
                  {artifacts.length === 0 ? (
                    <div className="overview-placeholder">No artifacts yet</div>
                  ) : (
                    artifacts.map((file) => {
                      const details = getArtifactDetails(file.name);
                      return (
                        <div key={file.path} className="overview-file-row" title={file.path} style={{ cursor: 'pointer' }}>
                          <span className="overview-file-icon">
                            {details.icon}
                          </span>
                          <span className="overview-file-name" style={{ color: 'var(--text-main)' }}>
                            {details.displayName}
                          </span>
                        </div>
                      );
                    })
                  )}
                </div>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
