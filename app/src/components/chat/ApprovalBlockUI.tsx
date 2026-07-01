import { useState, memo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import { toolApprovalResponded } from '../../features/chat/chatSlice';
import type { ApprovalBlock } from './turnHelpers';

const APPROVAL_LABELS: Record<string, string> = {
  deny: 'Denied (once)',
  deny_persistent: 'Denied (always)',
  allow_once: 'Allowed (once)',
  allow_session: 'Allowed (session)',
  allow_persistent: 'Allowed (always)',
};

const ApprovalBlockUI = memo(function ApprovalBlockUI({
  block,
  isOverlay = false,
}: {
  block: ApprovalBlock;
  isOverlay?: boolean;
}) {
  const dispatch = useAppDispatch();
  const [chosenAction, setChosenAction] = useState<string | null>(null);

  const promptId = block.prompt_id ?? '';
  const handleApprove = async (choice: string) => {
    setChosenAction(choice);
    dispatch(toolApprovalResponded({ promptId, approved: !choice.startsWith('deny') }));
    try {
      await invoke('approve_tool', { promptId, choice });
    } catch (e) {
      console.error('Failed to approve tool', e);
    }
  };

  const isResolved = block.status === 'approved' || block.status === 'denied';
  const statusLabel = chosenAction
    ? APPROVAL_LABELS[chosenAction] ?? (block.status === 'approved' ? 'Approved' : 'Denied')
    : block.status === 'approved' ? 'Approved' : block.status === 'denied' ? 'Denied' : '';

  if (isResolved) {
    return (
      <div className="approval-block approval-resolved">
        <div className="approval-header">
          <span className="approval-title">{block.tool_name}</span>
          {block.danger_level ? (
            <span className={`danger-badge danger-${block.danger_level}`}>{block.danger_level}</span>
          ) : null}
          <span className={`approval-status-badge ${block.status === 'approved' ? 'status-approved' : 'status-denied'}`}>
            {statusLabel}
          </span>
        </div>
      </div>
    );
  }

  const containerClass = isOverlay ? 'approval-overlay-card' : 'approval-block';

  return (
    <div className={containerClass}>
      <div className="approval-header">
        <span className="approval-title">
          Approval Required: <span style={{ color: 'var(--accent)', fontWeight: 600 }}>{block.tool_name}</span>
        </span>
        {block.danger_level ? (
          <span className={`danger-badge danger-${block.danger_level}`}>{block.danger_level}</span>
        ) : null}
      </div>
      <div className="approval-explanation">{block.explanation}</div>
      <div className="approval-args">
        <pre>{typeof block.tool_input === 'string' ? block.tool_input : JSON.stringify(block.tool_input, null, 2)}</pre>
      </div>
      <div className="approval-actions">
        <button className="btn-deny" onClick={() => handleApprove('deny')}>Deny Once</button>
        <button className="btn-deny" onClick={() => handleApprove('deny_persistent')}>Deny Always</button>
        <button className="btn-allow" onClick={() => handleApprove('allow_once')}>Allow Once</button>
        <button className="btn-allow" onClick={() => handleApprove('allow_session')}>Allow Session</button>
        <button className="btn-allow" onClick={() => handleApprove('allow_persistent')}>Always Allow</button>
      </div>
    </div>
  );
});

export default ApprovalBlockUI;
