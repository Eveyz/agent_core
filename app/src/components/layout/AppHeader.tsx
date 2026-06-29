import PanelRightOpenIcon from 'lucide-react/dist/esm/icons/panel-right-open.mjs';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import PanelRightIcon from 'lucide-react/dist/esm/icons/panel-right.mjs';
import { SessionTitle } from './SessionTitle';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import { createSession } from '../../features/project/projectSlice';

interface AppHeaderProps {
  sessionTitle: string;
  viewingSubagentPath: Array<{ id: string; name: string }>;
  activeSessionId: string | null;
  activeProjectId: string | null;
  sidebarCollapsed: boolean;
  onExpandSidebar: () => void;
  rightSidebarExpanded?: boolean;
  onToggleRightSidebar?: () => void;
}

export function AppHeader({
  sessionTitle,
  viewingSubagentPath,
  activeSessionId,
  activeProjectId,
  sidebarCollapsed,
  onExpandSidebar,
  rightSidebarExpanded,
  onToggleRightSidebar,
}: AppHeaderProps) {
  const dispatch = useAppDispatch();

  return (
    <header className="main-header">
      <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
        {sidebarCollapsed && (
          <>
            <div style={{ width: 60, flexShrink: 0 }} />
            <button className="sidebar-expand-btn" onClick={onExpandSidebar} title="展开侧边栏">
              <PanelRightIcon size={16} />
            </button>
            <button
              className="sidebar-expand-btn"
              onClick={() => {
                if (activeProjectId) {
                  dispatch(createSession(activeProjectId));
                }
              }}
              title="新建会话"
            >
              <PlusIcon size={16} />
            </button>
          </>
        )}
        <SessionTitle
          sessionTitle={sessionTitle}
          viewingSubagentPath={viewingSubagentPath}
          activeSessionId={activeSessionId}
          activeProjectId={activeProjectId}
        />
      </div>
      <div className="header-actions">
        {onToggleRightSidebar && !rightSidebarExpanded && (
          <button className="icon-btn" onClick={onToggleRightSidebar} title="展开右侧栏">
            <PanelRightOpenIcon size={16} />
          </button>
        )}
      </div>
    </header>
  );
}
