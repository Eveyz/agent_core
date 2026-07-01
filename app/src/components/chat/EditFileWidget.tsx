import { useState, useMemo, memo } from 'react';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import { getToolIcon } from './toolIcons';
import { basename, parseEditSummary, parseUnifiedDiff } from './turnHelpers';

const EditFileWidget = memo(function EditFileWidget({
  args,
  result,
  active,
  is_error,
}: {
  args?: unknown;
  result?: string;
  active?: boolean;
  is_error?: boolean;
}) {
  const [collapsed, setCollapsed] = useState(!active);
  const [prevActive, setPrevActive] = useState(active);

  if (active !== prevActive) {
    setPrevActive(active);
    setCollapsed(!active);
  }

  const filePath = (args as Record<string, unknown> | undefined)?.file_path as string | undefined;
  const fileName = filePath ? basename(filePath) : 'file';

  const summary = useMemo(() => (result ? parseEditSummary(result) : null), [result]);
  const diffRows = useMemo(() => {
    if (!result) return [];
    // The diff starts after the summary line.
    const diffStart = result.indexOf('--- ');
    if (diffStart === -1) return [];
    return parseUnifiedDiff(result.slice(diffStart));
  }, [result]);

  const range = useMemo(() => {
    if (!summary) return null;
    return summary.start === summary.end ? `L${summary.start}` : `L${summary.start}–L${summary.end}`;
  }, [summary]);

  const labelPrefix = active ? 'Editing' : is_error ? 'Edit failed:' : summary ? 'Edited' : 'Edited';

  return (
    <div className="step-block edit-file-block">
      <div
        className={`step-row ${active ? 'step-row-active' : ''} ${is_error ? 'step-row-error' : ''} ${active ? 'step-row-default' : 'step-row-pointer'}`}
        onClick={() => !active && setCollapsed(!collapsed)}
      >
        {(() => { const ToolIcon = getToolIcon('edit'); return <ToolIcon size={13} className="step-icon tool-icon-margin" color={is_error ? 'var(--danger)' : (active ? 'var(--text-muted)' : 'var(--text-muted)')} />; })()}
        <span className="step-label edit-file-label">
          {labelPrefix} <span className="edit-file-name">{fileName}</span>
          {range && <span className="edit-file-range"> · {range}</span>}
          {summary && (
            <span className="edit-file-stats">
              {' '}(<span className="stat-add">+{summary.additions}</span> <span className="stat-del">−{summary.deletions}</span>)
            </span>
          )}
        </span>
        {!active && !is_error && diffRows.length > 0 && (
          collapsed
            ? <ChevronRightIcon size={12} className="step-chevron" />
            : <ChevronDownIcon size={12} className="step-chevron" />
        )}
      </div>
      {!collapsed && diffRows.length > 0 && (
        <div className="edit-diff-body">
          <div className="edit-diff-path">{filePath}</div>
          <div className="edit-diff-table">
            {diffRows.map((row, idx) => (
              <div key={idx} className={`diff-row diff-${row.type}`}>
                <span className="diff-lineno diff-old">{row.oldLineNo ?? ''}</span>
                <span className="diff-linecontent diff-old-content">
                  {row.type === 'add' ? '' : row.oldText}
                </span>
                <span className="diff-lineno diff-new">{row.newLineNo ?? ''}</span>
                <span className="diff-linecontent diff-new-content">
                  {row.type === 'del' ? '' : row.newText}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
});

export default EditFileWidget;
