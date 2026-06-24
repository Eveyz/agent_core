import { useState, useMemo, memo } from 'react';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import { formatTime } from '../../utils/format';
import { MarkdownContent } from './MarkdownContent';
import { getToolIcon } from './toolIcons';

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
  const [showMore, setShowMore] = useState(false);

  if (active !== prevActive) {
    setPrevActive(active);
    setCollapsed(!active);
  }

  const formattedArgs = useMemo(() => {
    if (!args) return '';
    if (typeof args === 'string') return args;
    try {
      return JSON.stringify(args, null, 2);
    } catch {
      return String(args);
    }
  }, [args]);

  const displayResult = useMemo(() => {
    if (!result) return '';
    if (!showMore && result.length > 500) {
      return result.substring(0, 500) + '...\n\n*(Truncated. Click Show More to see full output)*';
    }
    return result;
  }, [result, showMore]);

  return (
    <div className="step-block">
      <div
        className={`step-row ${active ? 'step-row-active' : ''} ${is_error ? 'step-row-error' : ''} step-row-pointer`}
        onClick={() => setCollapsed(!collapsed)}
      >
        {(() => { const ToolIcon = getToolIcon(name); return <ToolIcon size={13} className="step-icon tool-icon-margin" color={is_error ? '#f87171' : (active ? 'var(--text-muted)' : 'var(--text-muted)')} />; })()}
        <span className="step-label tool-name tool-name-flex">
          <span>{name}</span>
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
          {formattedArgs && (
            <div className="tool-section">
              <div className="tool-section-label">INPUT</div>
              <pre className="tool-args-pre">{formattedArgs}</pre>
            </div>
          )}
          {!active && (
            <div className="tool-section">
              <div className="tool-section-label">OUTPUT</div>
              {result ? (
                <>
                  <MarkdownContent content={displayResult} className="tool-result-content assistant-msg" plainText />
                  {!showMore && result.length > 500 && (
                    <div
                      className="tool-show-more-btn"
                      onClick={(e) => { e.stopPropagation(); setShowMore(true); }}
                      style={{
                        color: 'var(--accent)',
                        cursor: 'pointer',
                        fontSize: '12px',
                        textAlign: 'center',
                        padding: '6px 0',
                        marginTop: '8px',
                        borderRadius: '6px',
                        background: 'var(--overlay-0_04)',
                        transition: 'background 0.2s',
                      }}
                      onMouseEnter={(e) => e.currentTarget.style.background = 'var(--overlay-0_08)'}
                      onMouseLeave={(e) => e.currentTarget.style.background = 'var(--overlay-0_04)'}
                    >
                      Show More
                    </div>
                  )}
                </>
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
