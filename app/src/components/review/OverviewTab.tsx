import { useState, useMemo, useEffect, useCallback, type MouseEvent } from 'react';
import { useSelector } from 'react-redux';
import { RootState } from '../../store';
import { selectActiveSessionEntries } from '../../features/chat/selectors';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import { viewSubagent } from '../../features/chat/chatSlice';
import type { SubagentEntry, TodoItem } from '../../features/chat/types';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import BotIcon from 'lucide-react/dist/esm/icons/bot.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import BookOpenIcon from 'lucide-react/dist/esm/icons/book-open.mjs';
import FileTextIcon from 'lucide-react/dist/esm/icons/file-text.mjs';
import CheckSquareIcon from 'lucide-react/dist/esm/icons/check-square.mjs';
import ImageIcon from 'lucide-react/dist/esm/icons/image.mjs';
import FileIcon from 'lucide-react/dist/esm/icons/file.mjs';
import { basename } from '../chat/turnHelpers';
import { MarkdownContent } from '../chat/MarkdownContent';
import { getFileIcon } from '../layout/FileTree';
import { formatTime } from '../../utils/format';
import {
  groupSubagentsByPrompt,
  countGroupedSubagents,
  getLastAssistantText,
  getToolNames,
  truncateText,
} from './overviewSubagents';
import { groupPlansByPrompt, countPlanItems, planProgress } from './overviewTodos';
import {
  extractWebSourcesFromEntries,
  formatRelativePublishedAt,
  type WebSource,
} from '../chat/webSources';
import {
  extractMapFeaturesFromEntries,
  providerLabel,
  type MapFeature,
} from '../chat/mapSources';
import GlobeIcon from 'lucide-react/dist/esm/icons/globe.mjs';
import MapPinIcon from 'lucide-react/dist/esm/icons/map-pin.mjs';
import RouteIcon from 'lucide-react/dist/esm/icons/route.mjs';

type SectionKey = 'subagent' | 'files_changed' | 'todos' | 'artifacts' | 'web' | 'maps';

async function openExternalUrl(url: string) {
  try {
    const { openUrl } = await import('@tauri-apps/plugin-opener');
    await openUrl(url);
  } catch {
    window.open(url, '_blank', 'noopener,noreferrer');
  }
}

function SubagentStatusIcon({ status }: { status: SubagentEntry['status'] }) {
  if (status === 'working') {
    return <div className="black-hole-spinner" style={{ width: 12, height: 12 }} />;
  }
  if (status === 'done') {
    return <CheckIcon size={12} color="var(--success)" />;
  }
  if (status === 'error') {
    return <XIcon size={12} color="var(--danger)" />;
  }
  return <BotIcon size={14} style={{ color: 'var(--accent)' }} />;
}

function statusLabel(status: SubagentEntry['status']): string {
  if (status === 'working') return 'Working';
  if (status === 'done') return 'Done';
  if (status === 'error') return 'Failed';
  return status;
}

function planStatusLabel(status: string): string {
  if (status === 'active') return 'Active';
  if (status === 'parked') return 'Parked';
  if (status === 'finished') return 'Finished';
  if (status === 'cancelled') return 'Cancelled';
  return status;
}

function todoItemStatusClass(status: TodoItem['status'] | string): string {
  if (status === 'completed') return 'overview-todo-item-done';
  if (status === 'in_progress') return 'overview-todo-item-active';
  if (status === 'blocked') return 'overview-todo-item-blocked';
  return '';
}

export function OverviewTab() {
  const dispatch = useAppDispatch();
  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const activeProject = projects.find((p) => p.id === activeProjectId);
  const entries = useSelector(selectActiveSessionEntries);
  const subagentsMap = useSelector((state: RootState) => state.chat.subagents);
  const allPrompts = useSelector((state: RootState) => state.chat.allPrompts);
  const plansBySession = useSelector((state: RootState) => state.chat.plans);
  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);

  const [expanded, setExpanded] = useState<Set<SectionKey>>(
    new Set(['files_changed', 'todos'])
  );
  const [showAllFiles, setShowAllFiles] = useState(false);
  const [expandedSubagentId, setExpandedSubagentId] = useState<string | null>(null);
  const [expandedPlanId, setExpandedPlanId] = useState<string | null>(null);

  const sessionSubagents = useMemo(() => {
    if (!activeSessionId) return {};
    return subagentsMap[activeSessionId] || {};
  }, [subagentsMap, activeSessionId]);

  const prompts = useMemo(() => {
    if (!activeSessionId) return [];
    return allPrompts[activeSessionId] || [];
  }, [allPrompts, activeSessionId]);

  const subagentGroups = useMemo(
    () => groupSubagentsByPrompt(prompts, sessionSubagents, entries),
    [prompts, sessionSubagents, entries]
  );

  const subagentCount = useMemo(
    () => countGroupedSubagents(subagentGroups),
    [subagentGroups]
  );

  const sessionPlans = useMemo(() => {
    if (!activeSessionId) return [];
    return plansBySession[activeSessionId] || [];
  }, [plansBySession, activeSessionId]);

  const todoGroups = useMemo(
    () => groupPlansByPrompt(sessionPlans, prompts),
    [sessionPlans, prompts]
  );

  const todoItemCount = useMemo(() => countPlanItems(sessionPlans), [sessionPlans]);

  // Auto-expand Subagents section when the session has any
  useEffect(() => {
    if (subagentCount === 0) return;
    setExpanded((prev) => {
      if (prev.has('subagent')) return prev;
      const next = new Set(prev);
      next.add('subagent');
      return next;
    });
  }, [subagentCount]);

  // Auto-expand Todos when plans exist
  useEffect(() => {
    if (todoItemCount === 0) return;
    setExpanded((prev) => {
      if (prev.has('todos')) return prev;
      const next = new Set(prev);
      next.add('todos');
      return next;
    });
  }, [todoItemCount]);

  const webSources = useMemo(
    () => extractWebSourcesFromEntries(entries),
    [entries],
  );

  const mapFeatures = useMemo(
    () => extractMapFeaturesFromEntries(entries),
    [entries],
  );

  // Focus Web / Maps section when chip opens Overview
  useEffect(() => {
    const handleOpen = (e: Event) => {
      const detail = (e as CustomEvent<{ tab?: string; section?: string }>).detail;
      if (detail?.tab !== 'overview') return;
      if (detail.section !== 'web' && detail.section !== 'maps') return;
      const section = detail.section;
      setExpanded((prev) => {
        if (prev.has(section)) return prev;
        const next = new Set(prev);
        next.add(section);
        return next;
      });
      // Scroll into view after expand paints
      requestAnimationFrame(() => {
        document.getElementById(`overview-section-${section}`)?.scrollIntoView({
          behavior: 'smooth',
          block: 'nearest',
        });
      });
    };
    window.addEventListener('open-right-sidebar', handleOpen);
    return () => window.removeEventListener('open-right-sidebar', handleOpen);
  }, []);

  // Auto-expand Web when the session has sources
  useEffect(() => {
    if (webSources.length === 0) return;
    setExpanded((prev) => {
      if (prev.has('web')) return prev;
      const next = new Set(prev);
      next.add('web');
      return next;
    });
  }, [webSources.length]);

  useEffect(() => {
    if (mapFeatures.length === 0) return;
    setExpanded((prev) => {
      if (prev.has('maps')) return prev;
      const next = new Set(prev);
      next.add('maps');
      return next;
    });
  }, [mapFeatures.length]);

  const toggleSection = (key: SectionKey) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const toggleSubagentRow = useCallback((id: string) => {
    setExpandedSubagentId((prev) => (prev === id ? null : id));
  }, []);

  const togglePlanRow = useCallback((id: string) => {
    setExpandedPlanId((prev) => (prev === id ? null : id));
  }, []);

  const handleViewDetails = useCallback(
    (sa: SubagentEntry, e: MouseEvent) => {
      e.stopPropagation();
      if (!activeSessionId) return;
      const name = sa.role_name || sa.id;
      dispatch(viewSubagent({ sessionId: activeSessionId, id: sa.id, name }));
    },
    [dispatch, activeSessionId]
  );

  // Extract modified files
  const modifiedFiles = useMemo(() => {
    const files = new Map<string, string>();
    const getParsedArgs = (rawArgs: unknown) => {
      if (typeof rawArgs === 'string') {
        try {
          return JSON.parse(rawArgs);
        } catch {
          return {};
        }
      }
      return rawArgs || {};
    };

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
            const args = getParsedArgs(block.args);
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
    const getParsedArgs = (rawArgs: unknown) => {
      if (typeof rawArgs === 'string') {
        try {
          return JSON.parse(rawArgs);
        } catch {
          return {};
        }
      }
      return rawArgs || {};
    };

    for (const entry of entries) {
      if (entry.type !== 'turn' || !entry.blocks) continue;
      for (const block of entry.blocks) {
        if (
          block.type === 'tool' &&
          !block.is_error &&
          block.result &&
          (block.name === 'write_to_file' || block.name === 'write_file')
        ) {
          const args = getParsedArgs(block.args);
          const path = args?.TargetFile || args?.file_path || args?.path;
          if (path) {
            const filename = basename(path);
            const ext = filename.split('.').pop()?.toLowerCase() || '';
            const isSysDoc = systemDocs.includes(filename);
            const isMedia = mediaExts.includes(ext);
            const isUnderSessionsDir =
              path.includes('.agverse/sessions/') || path.includes('.agverse\\sessions\\');

            if ((isSysDoc || isMedia || isUnderSessionsDir) && !files.has(path)) {
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
        icon: <BookOpenIcon size={14} style={{ color: 'var(--text-dim)' }} />,
      };
    }
    if (nameLower === 'implementation_plan.md' || nameLower === 'plan.md') {
      return {
        displayName: 'Implementation Plan',
        icon: <FileTextIcon size={14} style={{ color: 'var(--text-dim)' }} />,
      };
    }
    if (nameLower === 'task.md') {
      return {
        displayName: 'Task',
        icon: <CheckSquareIcon size={14} style={{ color: 'var(--text-dim)' }} />,
      };
    }
    if (/\.(png|jpg|jpeg|webp|gif)$/i.test(nameLower)) {
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
        icon: <ImageIcon size={14} style={{ color: 'var(--text-dim)' }} />,
      };
    }
    const cleanExt = name.replace(/\.[^/.]+$/, '');
    const displayName = cleanExt
      .split(/[-_]+/)
      .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
      .join(' ');
    return {
      displayName,
      icon: <FileIcon size={14} style={{ color: 'var(--text-dim)' }} />,
    };
  };

  const sectionHeader = (key: SectionKey, label: string, count?: number) => (
    <div
      className="overview-section-header"
      role="button"
      tabIndex={0}
      aria-expanded={expanded.has(key)}
      onClick={() => toggleSection(key)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          toggleSection(key);
        }
      }}
    >
      <span className="overview-section-label-group">
        <span className="overview-section-label">{label}</span>
        {count !== undefined && <span className="overview-section-badge">{count}</span>}
      </span>
      <span className="overview-section-chevron">
        {expanded.has(key) ? <ChevronDownIcon size={14} /> : <ChevronRightIcon size={14} />}
      </span>
    </div>
  );

  const statusDot = (status: string) => {
    const map: Record<string, string> = {
      pending: 'var(--text-muted)',
      in_progress: 'var(--warning)',
      completed: 'var(--success)',
      blocked: 'var(--danger)',
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

  const renderSubagentRow = (sa: SubagentEntry) => {
    const isOpen = expandedSubagentId === sa.id;
    const finalText = getLastAssistantText(sa.blocks);
    const preview = truncateText(finalText || (typeof sa.task === 'string' ? sa.task : ''));
    const toolNames = getToolNames(sa.blocks);
    const displayName = sa.role_name || 'Subagent';
    const elapsed =
      sa.endTime && sa.startTime
        ? formatTime(sa.endTime - sa.startTime)
        : sa.status === 'working' && sa.startTime
          ? formatTime(Date.now() - sa.startTime)
          : '';

    return (
      <div key={sa.id} className={`overview-subagent-row ${isOpen ? 'is-expanded' : ''}`}>
        <div
          className="overview-subagent-header"
          role="button"
          tabIndex={0}
          aria-expanded={isOpen}
          onClick={() => toggleSubagentRow(sa.id)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              toggleSubagentRow(sa.id);
            }
          }}
        >
          <span className="overview-subagent-icon">
            <SubagentStatusIcon status={sa.status} />
          </span>
          <div className="overview-subagent-main">
            <div className="overview-subagent-title-row">
              <span className="overview-subagent-name">{displayName}</span>
              <span className="overview-subagent-status">{statusLabel(sa.status)}</span>
            </div>
            {preview ? (
              <div className="overview-subagent-preview" title={preview}>
                {preview}
              </div>
            ) : null}
          </div>
          <span className="overview-subagent-chevron">
            {isOpen ? <ChevronDownIcon size={12} /> : <ChevronRightIcon size={12} />}
          </span>
        </div>

        {isOpen && (
          <div className="overview-subagent-expand">
            {sa.task ? (
              <div className="overview-subagent-meta">
                <span className="overview-subagent-meta-label">Task</span>
                <span className="overview-subagent-meta-value">
                  {typeof sa.task === 'string' ? sa.task : JSON.stringify(sa.task)}
                </span>
              </div>
            ) : null}

            <div className="overview-subagent-meta">
              <span className="overview-subagent-meta-label">Stats</span>
              <span className="overview-subagent-meta-value">
                {[
                  statusLabel(sa.status),
                  sa.iterations_used != null ? `${sa.iterations_used} iter` : null,
                  toolNames.length > 0 ? `${toolNames.length} tools` : null,
                  elapsed || null,
                ]
                  .filter(Boolean)
                  .join(' · ')}
              </span>
            </div>

            {toolNames.length > 0 ? (
              <div className="overview-subagent-meta">
                <span className="overview-subagent-meta-label">Tools</span>
                <span className="overview-subagent-tools">
                  {toolNames.map((name) => (
                    <span key={name} className="overview-subagent-tool-chip">
                      {name}
                    </span>
                  ))}
                </span>
              </div>
            ) : null}

            {finalText ? (
              <div className="overview-subagent-meta">
                <span className="overview-subagent-meta-label">Result</span>
                <MarkdownContent content={finalText} className="overview-subagent-final" />
              </div>
            ) : null}

            <button
              type="button"
              className="overview-subagent-view-btn"
              onClick={(e) => handleViewDetails(sa, e)}
            >
              View details <ChevronRightIcon size={12} />
            </button>
          </div>
        )}
      </div>
    );
  };

  return (
    <div className="overview-tab-container">
      <div className="overview-body">
        {!activeProject ? (
          <div className="empty-message">No active project</div>
        ) : (
          <div className="overview-sections">
            {/* ── Subagents ─────────────────────────────────────── */}
            <div className="overview-section">
              {sectionHeader('subagent', 'Subagents', subagentCount)}
              {expanded.has('subagent') && (
                <div className="overview-section-body">
                  {subagentCount === 0 ? (
                    <div className="overview-placeholder">No subagents</div>
                  ) : (
                    subagentGroups.map((group) => (
                      <div key={group.promptId} className="overview-prompt-group">
                        <div className="overview-prompt-label" title={group.userPreview}>
                          <span className="overview-prompt-index">
                            Prompt {group.turnIndex + 1}
                          </span>
                          <span className="overview-prompt-preview">{group.userPreview}</span>
                        </div>
                        {group.subagents.map(renderSubagentRow)}
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
                          <span className="overview-file-icon">{getFileIcon(file.name)}</span>
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
                            transition: 'color 0.15s',
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

            {/* ── Todos ──────────────────────────────────────────── */}
            <div className="overview-section">
              {sectionHeader('todos', 'Todos', todoItemCount)}
              {expanded.has('todos') && (
                <div className="overview-section-body">
                  {todoItemCount === 0 ? (
                    <div className="overview-placeholder">No todos yet</div>
                  ) : (
                    todoGroups.map((group) => (
                      <div
                        key={group.promptId ?? 'current'}
                        className="overview-prompt-group"
                      >
                        <div className="overview-prompt-label" title={group.userPreview}>
                          <span className="overview-prompt-index">
                            {group.promptId == null
                              ? 'Current'
                              : `Prompt ${group.turnIndex + 1}`}
                          </span>
                          {group.promptId != null ? (
                            <span className="overview-prompt-preview">{group.userPreview}</span>
                          ) : null}
                        </div>
                        {group.plans.map((plan) => {
                          const isOpen = expandedPlanId === plan.id;
                          const { completed, total } = planProgress(plan.items ?? []);
                          return (
                            <div
                              key={plan.id}
                              className={`overview-todo-plan ${isOpen ? 'is-expanded' : ''}`}
                            >
                              <div
                                className="overview-todo-plan-header"
                                role="button"
                                tabIndex={0}
                                aria-expanded={isOpen}
                                onClick={() => togglePlanRow(plan.id)}
                                onKeyDown={(e) => {
                                  if (e.key === 'Enter' || e.key === ' ') {
                                    e.preventDefault();
                                    togglePlanRow(plan.id);
                                  }
                                }}
                              >
                                <span
                                  className={`overview-todo-plan-badge status-${plan.status}`}
                                >
                                  {planStatusLabel(plan.status)}
                                </span>
                                <div className="overview-todo-plan-main">
                                  <span className="overview-todo-plan-title">
                                    {plan.title || 'Untitled plan'}
                                  </span>
                                  <span className="overview-todo-plan-progress">
                                    {completed}/{total}
                                  </span>
                                </div>
                                <span className="overview-subagent-chevron">
                                  {isOpen ? (
                                    <ChevronDownIcon size={12} />
                                  ) : (
                                    <ChevronRightIcon size={12} />
                                  )}
                                </span>
                              </div>
                              {isOpen && (
                                <div className="overview-todo-plan-items">
                                  {(plan.items ?? []).length === 0 ? (
                                    <div className="overview-placeholder">No items</div>
                                  ) : (
                                    (plan.items ?? []).map((item) => (
                                      <div key={item.id} className="overview-task-row">
                                        {statusDot(item.status)}
                                        <span
                                          className={`overview-task-text ${todoItemStatusClass(item.status)}`}
                                        >
                                          {item.description}
                                        </span>
                                      </div>
                                    ))
                                  )}
                                </div>
                              )}
                            </div>
                          );
                        })}
                      </div>
                    ))
                  )}
                </div>
              )}
            </div>

            {/* ── Web ────────────────────────────────────────────── */}
            <div className="overview-section" id="overview-section-web">
              {sectionHeader('web', 'Web', webSources.length)}
              {expanded.has('web') && (
                <div className="overview-section-body">
                  {webSources.length === 0 ? (
                    <div className="overview-placeholder">No web sources yet</div>
                  ) : (
                    webSources.map((source) => (
                      <OverviewWebRow key={`${source.callId}:${source.url}`} source={source} />
                    ))
                  )}
                </div>
              )}
            </div>

            {/* ── Maps ───────────────────────────────────────────── */}
            <div className="overview-section" id="overview-section-maps">
              {sectionHeader('maps', 'Maps', mapFeatures.length)}
              {expanded.has('maps') && (
                <div className="overview-section-body">
                  {mapFeatures.length === 0 ? (
                    <div className="overview-placeholder">No map places yet</div>
                  ) : (
                    mapFeatures.map((feature) => (
                      <OverviewMapRow key={feature.id} feature={feature} />
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
                        <div
                          key={file.path}
                          className="overview-file-row"
                          title={file.path}
                          style={{ cursor: 'pointer' }}
                        >
                          <span className="overview-file-icon">{details.icon}</span>
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

function OverviewWebRow({ source }: { source: WebSource }) {
  const when = formatRelativePublishedAt(source.publishedAt);
  return (
    <button
      type="button"
      className="overview-web-row"
      title={source.url}
      onClick={() => void openExternalUrl(source.url)}
    >
      <span className="overview-web-favicon">
        {source.faviconUrl ? (
          <img src={source.faviconUrl} alt="" loading="lazy" referrerPolicy="no-referrer" />
        ) : (
          <GlobeIcon size={14} />
        )}
      </span>
      <span className="overview-web-main">
        <span className="overview-web-title">{source.title}</span>
        <span className="overview-web-meta">
          {source.siteName}
          {when ? ` · ${when}` : ''}
        </span>
        {source.snippet ? (
          <span className="overview-web-snippet">{source.snippet}</span>
        ) : null}
      </span>
    </button>
  );
}

function OverviewMapRow({ feature }: { feature: MapFeature }) {
  const title = feature.kind === 'place' ? feature.name : feature.title;
  const meta =
    feature.kind === 'place'
      ? [providerLabel(feature.provider), feature.address].filter(Boolean).join(' · ')
      : [providerLabel(feature.provider), feature.summary].filter(Boolean).join(' · ');
  return (
    <button
      type="button"
      className="overview-web-row"
      title={feature.mapUrl}
      onClick={() => void openExternalUrl(feature.mapUrl)}
    >
      <span className="overview-web-favicon">
        {feature.kind === 'place' ? <MapPinIcon size={14} /> : <RouteIcon size={14} />}
      </span>
      <span className="overview-web-main">
        <span className="overview-web-title">{title}</span>
        {meta ? <span className="overview-web-meta">{meta}</span> : null}
      </span>
    </button>
  );
}
