import PanelRightIcon from 'lucide-react/dist/esm/icons/panel-right.mjs';
import BoxIcon from 'lucide-react/dist/esm/icons/box.mjs';
import MessageSquareIcon from 'lucide-react/dist/esm/icons/message-square.mjs';
import TerminalSquareIcon from 'lucide-react/dist/esm/icons/terminal-square.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import Maximize2Icon from 'lucide-react/dist/esm/icons/maximize-2.mjs';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
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
}

export function AppHeader({
  sessionTitle,
  viewingSubagentPath,
  activeSessionId,
  activeProjectId,
  sidebarCollapsed,
  onExpandSidebar,
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
        <button className="icon-btn" disabled title="Coming soon">
          <BoxIcon size={14} />
        </button>
        <button className="icon-btn" disabled title="Coming soon">
          <MessageSquareIcon size={14} />
        </button>
        <button className="icon-btn" disabled title="Coming soon">
          <TerminalSquareIcon size={14} />
        </button>
        <button className="icon-btn" disabled title="Coming soon">
          <FolderIcon size={14} />
        </button>
        <button className="icon-btn" disabled title="Coming soon">
          <Maximize2Icon size={14} />
        </button>
      </div>
    </header>
  );
}
