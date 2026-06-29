import { useState } from 'react';
import PanelRightCloseIcon from 'lucide-react/dist/esm/icons/panel-right-close.mjs';
import FilePlusIcon from 'lucide-react/dist/esm/icons/file-plus.mjs';
import ListIcon from 'lucide-react/dist/esm/icons/list.mjs';
import { ReviewTab } from '../review/ReviewTab';
import { OverviewTab } from '../review/OverviewTab';

interface RightSidebarProps {
  sidebarRef: React.RefObject<HTMLDivElement | null>;
  isExpanded: boolean;
  onToggle: () => void;
}

export function RightSidebar({ sidebarRef, isExpanded, onToggle }: RightSidebarProps) {
  const [activeTab, setActiveTab] = useState<'overview' | 'review'>('overview');

  return (
    <aside 
      className={`right-sidebar ${!isExpanded ? 'right-sidebar-collapsed' : ''}`} 
      ref={sidebarRef} 
      style={!isExpanded ? undefined : { width: 400 }}
    >
      <div className="right-sidebar-header">
        <div className="right-sidebar-tabs">
          <button 
            className={`tab-btn ${activeTab === 'overview' ? 'active' : ''}`} 
            onClick={() => setActiveTab('overview')}
          >
            <ListIcon size={14} className="tab-icon" />
            Overview
          </button>
          <button 
            className={`tab-btn ${activeTab === 'review' ? 'active' : ''}`} 
            onClick={() => setActiveTab('review')}
          >
            <FilePlusIcon size={14} className="tab-icon" />
            Review
          </button>
        </div>
        <button className="right-sidebar-toggle" onClick={onToggle} title="收起右侧栏">
          <PanelRightCloseIcon size={16} />
        </button>
      </div>
      <div className="right-sidebar-content">
        {activeTab === 'overview' && <OverviewTab />}
        {activeTab === 'review' && <ReviewTab />}
      </div>
    </aside>
  );
}
