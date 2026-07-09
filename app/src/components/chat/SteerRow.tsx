import { memo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSelector, useDispatch } from 'react-redux';
import type { RootState } from '../../store';
import { type ChatEntry, steerMessageCancelled } from '../../features/chat/chatSlice';
import ClockIcon from 'lucide-react/dist/esm/icons/clock.mjs';
import CheckCircleIcon from 'lucide-react/dist/esm/icons/check-circle.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';

export const SteerRow = memo(function SteerRow({ entry }: {
  entry: ChatEntry;
}) {
  const dispatch = useDispatch();
  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);
  const runId = useSelector((state: RootState) => state.chat.runId[activeSessionId ?? '']);
  const isPending = entry.steerStatus === 'pending';

  const handleCancel = async () => {
    if (!entry.steerId) return;
    if (!runId) {
      dispatch(steerMessageCancelled({ sessionId: activeSessionId ?? '', steerId: entry.steerId }));
      return;
    }
    try {
      await invoke('cancel_steer', { runId, steerId: entry.steerId });
    } catch (e) {
      console.error('Failed to cancel steer:', e);
    }
  };

  return (
    <div className="steer-row">
      <div className="steer-row-content">
        <div className="steer-msg">{entry.text}</div>
        <div className="steer-meta">
          {isPending ? (
            <>
              <span className="steer-badge steer-badge-pending">
                <ClockIcon size={11} /> Queued — will inject after current step
              </span>
              <button
                className="steer-cancel-btn"
                onClick={handleCancel}
                title="Cancel this steering message"
              >
                <XIcon size={12} />
              </button>
            </>
          ) : (
            <span className="steer-badge steer-badge-injected">
              <CheckCircleIcon size={11} /> Injected
            </span>
          )}
        </div>
      </div>
    </div>
  );
});
