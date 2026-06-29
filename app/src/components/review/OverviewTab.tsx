import { useState, useMemo } from 'react';
import { useSelector } from 'react-redux';
import { RootState } from '../../store';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import { basename } from '../chat/turnHelpers';
import { getFileIcon } from '../layout/FileTree';

type SectionKey = 'subagent' | 'files_changed' | 'tasks' | 'artifacts';

export function OverviewTab() {
  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const activeProject = projects.find((p) => p.id === activeProjectId);
  const entries = useSelector((state: RootState) => state.chat.entries);
  const todo = useSelector((state: RootState) => state.chat.todo);

  const [expanded, setExpanded] = useState<Set<SectionKey>>(
    new Set(['files_changed', 'tasks'])
  );

  const toggleSection = (key: SectionKey) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

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
            name === 'write_to_file'
          ) {
            const args = block.args as any;
            const path = args?.file_path || args?.TargetFile;
            if (path && !files.has(path)) {
              files.set(path, basename(path));
            }
          }
        }
      }
    }
    return Array.from(files.entries()).map(([path, name]) => ({ path, name }));
  }, [entries]);

  const sectionHeader = (key: SectionKey, label: string, count?: number) => (
    <div className="overview-section-header" onClick={() => toggleSection(key)}>
      <span className="overview-section-chevron">
        {expanded.has(key) ? <ChevronDownIcon size={14} /> : <ChevronRightIcon size={14} />}
      </span>
      <span className="overview-section-label">
        {label}{count !== undefined ? ` (${count})` : ''}
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

  return (
    <div className="overview-tab-container">
      <div className="overview-header">
        <span className="overview-title">Project Overview</span>
      </div>
      <div className="overview-body">
        {!activeProject ? (
          <div className="empty-message">No active project</div>
        ) : (
          <div className="overview-sections">
            {/* ── Subagent ──────────────────────────────────────── */}
            <div className="overview-section">
              {sectionHeader('subagent', 'Subagents')}
              {expanded.has('subagent') && (
                <div className="overview-section-body">
                  <div className="overview-placeholder">Coming soon</div>
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
                    modifiedFiles.map((file) => (
                      <div key={file.path} className="overview-file-row" title={file.path}>
                        <span className="overview-file-icon">
                          {getFileIcon(file.name)}
                        </span>
                        <span className="overview-file-name">{file.name}</span>
                        <span className="overview-file-path">{file.path}</span>
                      </div>
                    ))
                  )}
                </div>
              )}
            </div>

            {/* ── Tasks ──────────────────────────────────────────── */}
            <div className="overview-section">
              {sectionHeader('tasks', 'Tasks', todo.length)}
              {expanded.has('tasks') && (
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
              {sectionHeader('artifacts', 'Artifacts')}
              {expanded.has('artifacts') && (
                <div className="overview-section-body">
                  <div className="overview-placeholder">Coming soon</div>
                </div>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
