import { useState, useMemo, memo } from 'react';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import { formatTime } from '../../utils/format';
import { MarkdownContent } from './MarkdownContent';
import { getToolIcon } from './toolIcons';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';

const ToolBlockUI = memo(function ToolBlockUI({
  name,
  args,
  result,
  active,
  is_error,
  startTime,
  endTime,
  approvalStatus,
}: {
  name: string;
  args?: unknown;
  result?: string;
  active?: boolean;
  is_error?: boolean;
  startTime?: number;
  endTime?: number;
  approvalStatus?: 'approved' | 'denied';
}) {
  const [collapsed, setCollapsed] = useState(!active);
  const [prevActive, setPrevActive] = useState(active);
  const [copied, setCopied] = useState(false);

  if (active !== prevActive) {
    setPrevActive(active);
    setCollapsed(!active);
  }

  const handleCopy = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(result || '');
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (error) {
      console.error('Failed to copy:', error);
    }
  };

  const formattedArgs = useMemo(() => {
    if (!args) return '';
    if (typeof args === 'string') return args;
    try {
      return JSON.stringify(args, null, 2);
    } catch {
      return String(args);
    }
  }, [args]);

  const displayLabel = useMemo(() => {
    if (name === 'tavily_search') {
      const query = (args as Record<string, unknown> | undefined)?.query as string | undefined;
      if (query) return `Search "${query}"`;
    } else if (name === 'webfetch') {
      const url = (args as Record<string, unknown> | undefined)?.url as string | undefined;
      if (url) return `Fetch ${url}`;
    } else if (name === 'write_file' || name === 'write_to_file') {
      const path = ((args as Record<string, unknown> | undefined)?.TargetFile || (args as Record<string, unknown> | undefined)?.file_path || (args as Record<string, unknown> | undefined)?.path) as string | undefined;
      if (path) {
        const parts = path.replace(/\\/g, '/').split('/');
        const basename = parts[parts.length - 1];
        return active ? `Creating file: ${basename}` : `Created file: ${basename}`;
      }
      return active ? 'Creating file' : 'Created file';
    } else if (name === 'todo_write') {
      return 'Create Task List';
    } else if (name === 'todo_read') {
      return 'Read Task List';
    } else if (name === 'todo_update') {
      const id = (args as Record<string, unknown> | undefined)?.id as string | undefined;
      const status = (args as Record<string, unknown> | undefined)?.status as string | undefined;
      
      let desc = '';
      if (result) {
        const match = result.match(/Todo '.*?': "(.*?)" updated to/);
        if (match && match[1]) {
          desc = match[1];
        }
      }
      
      if (id && status) {
        if (desc) {
          const shortDesc = desc.length > 30 ? desc.substring(0, 30) + '...' : desc;
          return `Update Task "${shortDesc}" to ${status}`;
        }
        return `Update Task ${id} to ${status}`;
      }
      return 'Update Task';
    } else if (name === 'skill_load' || name === 'skill_reload') {
      const skillName = (args as Record<string, unknown> | undefined)?.name || (args as Record<string, unknown> | undefined)?.skill_name as string | undefined;
      const action = name === 'skill_load' ? 'Load' : 'Reload';
      if (skillName) {
        return active ? `${action}ing skill ${skillName}` : `${action} skill ${skillName}`;
      }
      return `${action} skill`;
    } else if (name === 'skill_deactivate') {
      const skillName = (args as Record<string, unknown> | undefined)?.name || (args as Record<string, unknown> | undefined)?.skill_name as string | undefined;
      if (skillName) {
        return active ? `Deactivating skill ${skillName}` : `Deactivate skill ${skillName}`;
      }
      return 'Deactivate skill';
    } else if (name === 'archival_memory_search') {
      const query = (args as Record<string, unknown> | undefined)?.query as string | undefined;
      return query ? `Search memory "${query}"` : 'Search memory';
    } else if (name === 'conversation_search') {
      const query = (args as Record<string, unknown> | undefined)?.query as string | undefined;
      return query ? `Search conversation "${query}"` : 'Search conversation';
    }
    return name;
  }, [name, args]);

  const isSearch = name === 'tavily_search' || name === 'webfetch';
  const hideInput = isSearch || name.startsWith('todo_') || name.startsWith('skill_') || name === 'archival_memory_search' || name === 'conversation_search';

  return (
    <div className="step-block">
      <style>{`
        .search-tool-result {
          font-size: 13px !important;
        }
        .search-tool-result h1, .search-tool-result h2, .search-tool-result h3, .search-tool-result h4, .search-tool-result h5, .search-tool-result h6 {
          font-size: 14px !important;
          margin: 10px 0 6px 0 !important;
          line-height: 1.4 !important;
          font-weight: 600 !important;
        }
        .search-tool-result * {
          font-size: 13px !important;
        }
        .scrollable-markdown {
          max-height: 400px;
          overflow-y: auto;
        }
        .tool-result-content {
          background: var(--overlay-0_02) !important;
          border: 1px solid var(--border-color) !important;
        }
      `}</style>
      <div
        className={`step-row ${active ? 'step-row-active' : ''} ${is_error ? 'step-row-error' : ''} step-row-pointer`}
        onClick={() => setCollapsed(!collapsed)}
      >
        {(() => { const ToolIcon = getToolIcon(name); return <ToolIcon size={13} className="step-icon tool-icon-margin" color={is_error ? '#f87171' : (active ? 'var(--text-muted)' : 'var(--text-muted)')} />; })()}
        <span className="step-label tool-name tool-name-flex">
          <span>{displayLabel}</span>
          {startTime && endTime && (
            <span className="tool-time-muted">· {formatTime(endTime - startTime)}</span>
          )}
        </span>
        <div className="tool-controls-flex">
          {approvalStatus === 'approved' && (
            <span className="approval-status-badge status-approved approval-badge-style">Approved</span>
          )}
          {approvalStatus === 'denied' && (
            <span className="approval-status-badge status-denied approval-badge-style">Denied</span>
          )}
          {collapsed ? <ChevronRightIcon size={12} className="step-chevron" /> : <ChevronDownIcon size={12} className="step-chevron" />}
        </div>
      </div>
      {!collapsed && (
        <div className="step-body">
          {formattedArgs && !hideInput && (
            <div className="tool-section">
              <div className="tool-section-label">INPUT</div>
              <pre className="tool-args-pre">{formattedArgs}</pre>
            </div>
          )}
          {!active && (
            <div className="tool-section">
              {!hideInput && <div className="tool-section-label">OUTPUT</div>}
              {result ? (
                name === 'skill_load' ? (
                  <div style={{ position: 'relative' }}>
                    <div style={{ padding: '12px', background: 'var(--overlay-0_02)', borderRadius: '8px', border: '1px solid var(--border-color)', fontSize: '13px' }}>
                       <div style={{ marginBottom: '8px', color: 'var(--text-primary)' }}><strong>Name:</strong> {((args as Record<string, unknown> | undefined)?.name as string) || ''}</div>
                       <div style={{ color: 'var(--text-muted)' }}><strong>Description:</strong> {result.match(/Description:\s*(.*?)\n/)?.[1]?.trim() || 'No description available'}</div>
                    </div>
                  </div>
                ) : name === 'archival_memory_search' || name === 'conversation_search' ? (
                  <div style={{ position: 'relative' }}>
                    <div style={{ padding: '12px', background: 'var(--overlay-0_02)', borderRadius: '8px', border: '1px solid var(--border-color)', fontSize: '13px' }}>
                      {(() => {
                        try {
                          const parsed = JSON.parse(result);
                          const results = parsed.results || [];
                          if (results.length === 0) return <div style={{ color: 'var(--text-muted)' }}>No results found.</div>;
                          return (
                            <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
                              {results.map((r: any, idx: number) => (
                                <div key={r.id || idx} style={{ padding: '10px', background: 'var(--bg-secondary)', borderRadius: '6px', border: '1px solid var(--border-color)' }}>
                                  <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: '11px', color: 'var(--text-muted)', marginBottom: '8px', borderBottom: '1px solid var(--border-color)', paddingBottom: '4px' }}>
                                    <span style={{ fontWeight: 600, textTransform: 'uppercase', color: 'var(--text-secondary)' }}>{r.role || 'MEMORY'}</span>
                                    <div style={{ display: 'flex', gap: '12px' }}>
                                      {r.importance !== undefined && <span>Score: {typeof r.importance === 'number' ? r.importance.toFixed(2) : r.importance}</span>}
                                      {r.created_at && <span>{new Date(r.created_at).toLocaleString()}</span>}
                                    </div>
                                  </div>
                                  <div style={{ color: 'var(--text-primary)', whiteSpace: 'pre-wrap', wordBreak: 'break-word', lineHeight: '1.5' }}>{r.content || r.text || r.message}</div>
                                  {r.metadata && <div style={{ fontSize: '11px', color: 'var(--text-muted)', marginTop: '8px', background: 'var(--overlay-0_02)', padding: '6px', borderRadius: '4px', border: '1px solid var(--border-color)', fontFamily: 'monospace', whiteSpace: 'pre-wrap', wordBreak: 'break-all' }}>{r.metadata}</div>}
                                </div>
                              ))}
                            </div>
                          );
                        } catch (e) {
                          return <MarkdownContent content={result} plainText={true} />;
                        }
                      })()}
                    </div>
                  </div>
                ) : (
                  <div style={{ position: 'relative' }}>
                    <button className="code-block-copy-btn" onClick={handleCopy} title="Copy output" style={{ position: 'absolute', top: '8px', right: '12px', display: 'flex', background: 'var(--bg-secondary)', border: '1px solid var(--border-color)', borderRadius: '4px', padding: '4px', cursor: 'pointer', color: 'var(--text-muted)', zIndex: 10 }}>
                      {copied ? <CheckIcon size={14} color="var(--success)" /> : <CopyIcon size={14} />}
                    </button>
                    <MarkdownContent 
                      content={result} 
                      className={`tool-result-content assistant-msg scrollable-markdown ${isSearch ? 'search-tool-result' : ''}`} 
                      plainText={!isSearch} 
                    />
                  </div>
                )
              ) : (
                <div className="tool-no-output">(No output returned)</div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
});

export default ToolBlockUI;
