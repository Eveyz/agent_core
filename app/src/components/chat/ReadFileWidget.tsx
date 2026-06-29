import { memo } from 'react';
import { getToolIcon } from './toolIcons';
import { basename } from './turnHelpers';

const ReadFileWidget = memo(function ReadFileWidget({
  args,
  active,
  is_error,
}: {
  args?: unknown;
  active?: boolean;
  is_error?: boolean;
}) {
  const argObj = (args as Record<string, unknown> | undefined);
  const filePath = argObj?.path as string | undefined || argObj?.file_path as string | undefined;
  const fileName = filePath ? basename(filePath) : 'file';

  const offset = argObj?.offset as number | undefined;
  const limit = argObj?.limit as number | undefined;
  
  let range = '';
  if (offset !== undefined) {
    if (limit !== undefined) {
      range = `L${offset}–L${offset + limit - 1}`;
    } else {
      range = `L${offset}`;
    }
  }

  const labelPrefix = active ? 'Reading' : is_error ? 'Read failed:' : 'Read';

  return (
    <div className="step-block read-file-block">
      <div
        className={`step-row ${active ? 'step-row-active' : ''} ${is_error ? 'step-row-error' : ''} step-row-default`}
      >
        {(() => { const ToolIcon = getToolIcon('read_file'); return <ToolIcon size={13} className="step-icon tool-icon-margin" color={is_error ? '#f87171' : 'var(--text-muted)'} />; })()}
        <span className="step-label edit-file-label">
          {labelPrefix} <span className="edit-file-name">{fileName}</span>
          {range && <span className="edit-file-range"> · {range}</span>}
        </span>
      </div>
    </div>
  );
});

export default ReadFileWidget;
