import { useState, useCallback } from 'react';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import PencilIcon from 'lucide-react/dist/esm/icons/pencil.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import { renameSession } from '../../features/project/projectSlice';
import { clearSubagentView, popSubagentView } from '../../features/chat/chatSlice';

interface SessionTitleProps {
  sessionTitle: string;
  viewingSubagentPath: Array<{ id: string; name: string }>;
  activeSessionId: string | null;
  activeProjectId: string | null;
}

export function SessionTitle({
  sessionTitle,
  viewingSubagentPath,
  activeSessionId,
  activeProjectId,
}: SessionTitleProps) {
  const dispatch = useAppDispatch();
  const [isEditingTitle, setIsEditingTitle] = useState(false);
  const [titleEditValue, setTitleEditValue] = useState('');

  const startEditingTitle = useCallback(() => {
    setTitleEditValue(sessionTitle || 'New Session');
    setIsEditingTitle(true);
  }, [sessionTitle]);

  const commitTitleEdit = useCallback(() => {
    const trimmed = titleEditValue.trim();
    if (trimmed && activeSessionId && activeProjectId) {
      dispatch(renameSession({ sessionId: activeSessionId, projectId: activeProjectId, newTitle: trimmed }));
    }
    setIsEditingTitle(false);
  }, [dispatch, titleEditValue, activeSessionId, activeProjectId]);

  const cancelTitleEdit = useCallback(() => {
    setIsEditingTitle(false);
  }, []);

  return (
    <div className="header-title">
      {isEditingTitle && viewingSubagentPath.length === 0 ? (
        <>
          <input
            className="header-title-input"
            value={titleEditValue}
            onChange={(e) => setTitleEditValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') commitTitleEdit();
              if (e.key === 'Escape') cancelTitleEdit();
            }}
            autoFocus
          />
          <button className="icon-btn header-edit-btn" onClick={commitTitleEdit} title="Save" style={{ opacity: 1 }}>
            <CheckIcon size={12} />
          </button>
          <button className="icon-btn header-edit-btn" onClick={cancelTitleEdit} title="Cancel" style={{ opacity: 1 }}>
            <XIcon size={12} />
          </button>
        </>
      ) : (
        <>
          <span
            className="header-session-name"
            style={viewingSubagentPath.length > 0 ? { cursor: 'pointer' } : undefined}
            onClick={viewingSubagentPath.length > 0 ? () => dispatch(clearSubagentView()) : undefined}
          >
            {sessionTitle || 'New Session'}
          </span>
          {viewingSubagentPath.map((seg, i) => (
            <span key={seg.id} className="header-breadcrumb">
              <span className="header-breadcrumb-sep">›</span>
              <span
                className="header-breadcrumb-name"
                style={{ cursor: i < viewingSubagentPath.length - 1 ? 'pointer' : 'default' }}
                onClick={
                  i < viewingSubagentPath.length - 1
                    ? () => {
                        const pops = viewingSubagentPath.length - 1 - i;
                        for (let k = 0; k < pops; k++) dispatch(popSubagentView());
                      }
                    : undefined
                }
              >
                {seg.name}
              </span>
            </span>
          ))}
          {viewingSubagentPath.length === 0 && (
            <button className="icon-btn header-edit-btn" onClick={startEditingTitle} title="Edit session title">
              <PencilIcon size={12} />
            </button>
          )}
        </>
      )}
    </div>
  );
}
