import { memo } from 'react';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import SearchIcon from 'lucide-react/dist/esm/icons/search.mjs';
import BoxIcon from 'lucide-react/dist/esm/icons/box.mjs';
import LayoutGridIcon from 'lucide-react/dist/esm/icons/layout-grid.mjs';
import MessageSquareIcon from 'lucide-react/dist/esm/icons/message-square.mjs';
import ClockIcon from 'lucide-react/dist/esm/icons/clock.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import SettingsIcon from 'lucide-react/dist/esm/icons/settings.mjs';
import SmartphoneIcon from 'lucide-react/dist/esm/icons/smartphone.mjs';
import BotIcon from 'lucide-react/dist/esm/icons/bot.mjs';

const flexGap8 = { display: 'flex', gap: '8px' } as const;

export const Sidebar = memo(function Sidebar({
  activeTab,
  onTabChange,
  onOpenSettings,
}: {
  activeTab: 'code' | 'write';
  onTabChange: (tab: 'code' | 'write') => void;
  onOpenSettings: () => void;
}) {
  return (
    <aside className="sidebar">
      <div className="sidebar-header-actions">
        <button className="icon-btn"><LayoutGridIcon size={16} /></button>
      </div>

      <div className="toggle-group">
        <button
          className={`toggle-btn ${activeTab === 'code' ? 'active' : ''}`}
          onClick={() => onTabChange('code')}
        >
          <BotIcon size={14} /> Code
        </button>
        <button
          className={`toggle-btn ${activeTab === 'write' ? 'active' : ''}`}
          onClick={() => onTabChange('write')}
        >
          <MessageSquareIcon size={14} /> Write
        </button>
      </div>

      <div className="sidebar-nav">
        <div className="nav-item"><PlusIcon size={14} /> New Agent</div>
        <div className="nav-item"><MessageSquareIcon size={14} /> New requirement</div>
        <div className="nav-item"><BoxIcon size={14} /> Plugins</div>
        <div className="nav-item"><ClockIcon size={14} /> Scheduled tasks</div>
      </div>

      <div className="projects-header">
        <span>Projects</span>
        <div style={flexGap8}>
          <SearchIcon size={12} />
          <BoxIcon size={12} />
          <FolderIcon size={12} />
        </div>
      </div>

      <div className="sidebar-nav" style={{ marginTop: '8px' }}>
        <div className="project-item">
          <span style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
            <FolderIcon size={14} color="#808080" /> agent_core
          </span>
          <span className="meta">rust-projects</span>
        </div>
        <div className="project-item">
          <span style={{ display: 'flex', alignItems: 'center', gap: '8px', paddingLeft: '22px', fontSize: '12px' }}>
            check the weather for Shenz...
          </span>
          <span className="meta" style={{ color: '#E2E2E2' }}>now</span>
        </div>
        <div className="project-item">
          <span style={{ display: 'flex', alignItems: 'center', gap: '8px', paddingLeft: '22px', fontSize: '12px' }}>
            what's the result for t...
          </span>
          <span className="meta">13 hours ago</span>
        </div>
        <div className="project-item">
          <span style={{ display: 'flex', alignItems: 'center', gap: '8px', paddingLeft: '22px', fontSize: '12px' }}>
            in the top status bar, c...
          </span>
          <span className="meta">3 days ago</span>
        </div>
      </div>

      <div className="sidebar-bottom">
        <div className="nav-item"><SmartphoneIcon size={14} /> Connect phone</div>
        <div className="nav-item" onClick={onOpenSettings}><SettingsIcon size={14} /> Settings</div>
      </div>
    </aside>
  );
});
