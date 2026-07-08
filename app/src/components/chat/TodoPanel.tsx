import { memo, useState } from 'react';
import { useSelector } from 'react-redux';
import { RootState } from '../../store';
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

function TodoPanel() {
  const { t } = useTranslation();
  const todo = useSelector((state: RootState) => state.chat.todo[state.chat.activeSessionId ?? '']);
  const [collapsed, setCollapsed] = useState(true);

  if (!todo || todo.length === 0) return null;

  const completed = todo.filter((t) => t.status === 'completed').length;
  const pct = Math.round((completed / todo.length) * 100);

  if (completed === todo.length) return null;

  return (
    <div className="todo-panel">
      <div 
        className="todo-header" 
        onClick={() => setCollapsed(!collapsed)}
        style={{ cursor: 'pointer', userSelect: 'none', marginBottom: collapsed ? 0 : 8 }}
      >
        <span className="todo-title-group" style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
          {collapsed ? <ChevronRightIcon size={14} style={{ color: 'var(--text-dim)' }} /> : <ChevronDownIcon size={14} style={{ color: 'var(--text-dim)' }} />}
          <span className="todo-title">{t('chat.todoPanel.plan')}</span>
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
    </div>
  );
}

export default memo(TodoPanel);

