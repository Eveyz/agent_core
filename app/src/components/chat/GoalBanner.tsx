import { memo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSelector } from 'react-redux';
import TargetIcon from 'lucide-react/dist/esm/icons/target.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import { RootState } from '../../store';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import { goalCleared } from '../../features/chat/chatSlice';

/** Pinned goal strip — sits above ChatInput, below TodoPanel. */
function GoalBanner() {
  const dispatch = useAppDispatch();
  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);
  const goal = useSelector((state: RootState) => state.chat.goal[activeSessionId ?? '']);
  const goalCompleted = useSelector(
    (state: RootState) => state.chat.goalCompleted[activeSessionId ?? '']
  );

  if (!goal || !activeSessionId) return null;

  const handleClear = async () => {
    dispatch(goalCleared({ sessionId: activeSessionId }));
    try {
      await invoke('clear_session_goal', { sessionId: activeSessionId });
    } catch (e) {
      console.error('Failed to clear session goal', e);
    }
  };

  return (
    <div className={`goal-banner${goalCompleted ? ' goal-banner-done' : ''}`}>
      <TargetIcon size={14} className="goal-banner-icon" />
      <div className="goal-banner-text">
        <strong>Goal:</strong> {goal}
        {goalCompleted ? <span className="goal-banner-done-label"> ✓ completed</span> : null}
      </div>
      <button
        type="button"
        className="goal-banner-clear"
        onClick={handleClear}
        title="Clear goal"
        aria-label="Clear goal"
      >
        <XIcon size={14} />
      </button>
    </div>
  );
}

export default memo(GoalBanner);
