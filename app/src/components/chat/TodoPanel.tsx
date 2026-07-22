import { memo, useState, useCallback } from 'react';
import { useSelector } from 'react-redux';
import { invoke } from '@tauri-apps/api/core';
import { RootState } from '../../store';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import { plansHydrated } from '../../features/chat/chatSlice';
import type { ParkedPlan, PlanDetail, TodoItem } from '../../features/chat/types';
import type { SendPayload } from './imageAttachments';
import CheckCircleIcon from 'lucide-react/dist/esm/icons/check-circle.mjs';
import CircleIcon from 'lucide-react/dist/esm/icons/circle.mjs';
import LoaderIcon from 'lucide-react/dist/esm/icons/loader.mjs';
import AlertCircleIcon from 'lucide-react/dist/esm/icons/alert-circle.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import { useTranslation } from 'react-i18next';

function statusIcon(status: string) {
  switch (status) {
    case 'completed':
      return <CheckCircleIcon size={14} className="todo-icon todo-icon-completed" />;
    case 'in_progress':
      return <LoaderIcon size={14} className="todo-icon todo-icon-in-progress" />;
    case 'blocked':
      return <AlertCircleIcon size={14} className="todo-icon todo-icon-blocked" />;
    default:
      return <CircleIcon size={14} className="todo-icon todo-icon-pending" />;
  }
}

type PlansDto = {
  items: TodoItem[];
  parked: ParkedPlan[];
  plans?: PlanDetail[];
  active_plan_id?: string | null;
  active_plan_title?: string | null;
};

function applyPlansDto(
  dispatch: ReturnType<typeof useAppDispatch>,
  sessionId: string,
  dto: PlansDto,
) {
  dispatch(
    plansHydrated({
      sessionId,
      items: dto.items ?? [],
      parked: dto.parked ?? [],
      plans: dto.plans ?? [],
      activePlanId: dto.active_plan_id ?? null,
      activePlanTitle: dto.active_plan_title ?? null,
    }),
  );
}

function TodoPanel({
  onSend,
  isProcessing = false,
}: {
  onSend: (payload: SendPayload | string) => void;
  isProcessing?: boolean;
}) {
  const { t } = useTranslation();
  const dispatch = useAppDispatch();
  const sessionId = useSelector((state: RootState) => state.project.activeSessionId ?? '');
  const todo = useSelector((state: RootState) => state.chat.todo[sessionId] ?? []);
  const parked = useSelector((state: RootState) => state.chat.parkedPlans[sessionId] ?? []);
  const activeTitle = useSelector(
    (state: RootState) => state.chat.activePlanTitle[sessionId] ?? null,
  );
  const [collapsed, setCollapsed] = useState(true);

  const resumePlan = useCallback(
    (planId: string) => {
      if (!sessionId || isProcessing) return;
      onSend(`/plan resume ${planId}`);
    },
    [sessionId, isProcessing, onSend],
  );

  const cancelPlan = useCallback(
    async (planId: string) => {
      if (!sessionId) return;
      try {
        const dto = await invoke<PlansDto>('cancel_session_plan', {
          sessionId,
          planId,
        });
        applyPlansDto(dispatch, sessionId, dto);
      } catch (e) {
        console.error('cancel_session_plan failed', e);
      }
    },
    [dispatch, sessionId],
  );

  const incompleteActive = todo.filter((item) => item.status !== 'completed');
  const showActive = incompleteActive.length > 0;
  const showParked = parked.length > 0;
  if (!showActive && !showParked) return null;

  const completed = todo.filter((item) => item.status === 'completed').length;
  const pct = todo.length ? Math.round((completed / todo.length) * 100) : 0;

  return (
    <div className="todo-panel">
      {showActive && (
        <>
          <div
            className="todo-header"
            onClick={() => setCollapsed(!collapsed)}
            style={{ cursor: 'pointer', userSelect: 'none', marginBottom: collapsed ? 0 : 8 }}
          >
            <span className="todo-title-group" style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
              {collapsed ? (
                <ChevronRightIcon size={14} style={{ color: 'var(--text-dim)' }} />
              ) : (
                <ChevronDownIcon size={14} style={{ color: 'var(--text-dim)' }} />
              )}
              <span className="todo-title">
                {activeTitle ? activeTitle : t('chat.todoPanel.plan')}
              </span>
            </span>
            <span className="todo-progress-text">
              {t('chat.todoPanel.completed', { completed, total: todo.length })}
            </span>
          </div>

          {!collapsed && (
            <>
              <div className="todo-progress-bar">
                <div className="todo-progress-fill" style={{ width: `${pct}%` }} />
              </div>
              <ul className="todo-list">
                {todo.map((item) => (
                  <li key={item.id} className={`todo-item todo-item-${item.status}`}>
                    {statusIcon(item.status)}
                    <span className="todo-desc">{item.description}</span>
                  </li>
                ))}
              </ul>
            </>
          )}
        </>
      )}

      {showParked && (
        <div className={`todo-parked${showActive ? ' todo-parked-below-active' : ''}`}>
          <div className="todo-header todo-parked-header">
            <span className="todo-title">Parked</span>
            <span className="todo-progress-text">{parked.length}</span>
          </div>
          <ul className="todo-list">
            {parked.map((p) => (
              <li key={p.id} className="todo-item todo-parked-row">
                <span className="todo-desc todo-parked-title">
                  {p.title}
                  <span className="todo-parked-progress">
                    {p.completed}/{p.total}
                  </span>
                </span>
                <div className="todo-parked-actions">
                  <button
                    type="button"
                    className="todo-parked-btn todo-parked-btn-resume"
                    disabled={isProcessing}
                    onClick={() => resumePlan(p.id)}
                  >
                    Resume
                  </button>
                  <button
                    type="button"
                    className="todo-parked-btn todo-parked-btn-cancel"
                    onClick={() => cancelPlan(p.id)}
                  >
                    Cancel
                  </button>
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

export default memo(TodoPanel);
