import { useState, useMemo, useEffect } from 'react';
import { useSelector } from 'react-redux';
import PanelRightCloseIcon from 'lucide-react/dist/esm/icons/panel-right-close.mjs';
import FilePlusIcon from 'lucide-react/dist/esm/icons/file-plus.mjs';
import ListIcon from 'lucide-react/dist/esm/icons/list.mjs';
import FileTextIcon from 'lucide-react/dist/esm/icons/file-text.mjs';
import BookOpenIcon from 'lucide-react/dist/esm/icons/book-open.mjs';
import { ReviewTab } from '../review/ReviewTab';
import { OverviewTab } from '../review/OverviewTab';
import { DocumentTab } from '../review/DocumentTab';
import { RootState } from '../../store';
import { selectActiveSessionEntries } from '../../features/chat/selectors';

interface RightSidebarProps {
  sidebarRef: React.RefObject<HTMLDivElement | null>;
  isExpanded: boolean;
  onToggle: () => void;
}

export function RightSidebar({ sidebarRef, isExpanded, onToggle }: RightSidebarProps) {
  const [activeTab, setActiveTab] = useState<'overview' | 'review' | 'plan' | 'walkthrough'>('overview');

  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const activeProject = projects.find((p) => p.id === activeProjectId);
  const entries = useSelector(selectActiveSessionEntries);

  // Determine if the session artifacts include PLAN.md or walkthrough.md
  const artifacts = useMemo(() => {
    const files = new Set<string>();
    for (const entry of entries) {
      if (entry.type !== 'turn' || !entry.blocks) continue;
      for (const block of entry.blocks) {
        if (block.type === 'tool' && !block.is_error && block.result && block.name === 'write_to_file') {
          const args = block.args as any;
          const path = args?.TargetFile || args?.file_path;
          if (path) {
            files.add(path.toLowerCase());
          }
        }
      }
    }
    return files;
  }, [entries]);

  const hasPlan = useMemo(() => {
    return Array.from(artifacts).some(path => path.includes('plan.md') || path.includes('implementation_plan.md'));
  }, [artifacts]);

  const hasWalkthrough = useMemo(() => {
    return Array.from(artifacts).some(path => path.includes('walkthrough.md'));
  }, [artifacts]);

  // Reset tab if it disappears
  useEffect(() => {
    if (activeTab === 'plan' && !hasPlan) {
      setActiveTab('overview');
    } else if (activeTab === 'walkthrough' && !hasWalkthrough) {
      setActiveTab('overview');
    }
  }, [activeTab, hasPlan, hasWalkthrough]);

  useEffect(() => {
    const handleOpen = (e: Event) => {
      const customEvent = e as CustomEvent<{ tab: 'overview' | 'review' | 'plan' | 'walkthrough' }>;
      if (customEvent.detail?.tab) {
        setActiveTab(customEvent.detail.tab);
      }
    };
    window.addEventListener('open-right-sidebar', handleOpen);
    return () => window.removeEventListener('open-right-sidebar', handleOpen);
  }, []);

  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);

  return (
    <aside 
      className={`right-sidebar ${!isExpanded ? 'right-sidebar-collapsed' : ''}`} 
      ref={sidebarRef} 
      style={!isExpanded ? undefined : { width: 500 }}
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
          {hasPlan && (
            <button 
              className={`tab-btn ${activeTab === 'plan' ? 'active' : ''}`} 
              onClick={() => setActiveTab('plan')}
            >
              <FileTextIcon size={14} className="tab-icon" />
              Implementation Plan
            </button>
          )}
          {hasWalkthrough && (
            <button 
              className={`tab-btn ${activeTab === 'walkthrough' ? 'active' : ''}`} 
              onClick={() => setActiveTab('walkthrough')}
            >
              <BookOpenIcon size={14} className="tab-icon" />
              Walkthrough
            </button>
          )}
        </div>
        <button className="right-sidebar-toggle" onClick={onToggle} title="收起右侧栏">
          <PanelRightCloseIcon size={16} />
        </button>
      </div>
      <div className="right-sidebar-content">
        {activeTab === 'overview' && <OverviewTab />}
        {activeTab === 'review' && <ReviewTab />}
        {activeTab === 'plan' && (
          <DocumentTab
            projectPath={activeProject?.path}
            relativePaths={[
              `~/.agverse/chats/${activeSessionId}/implementation_plan.md`,
              `~/.agverse/chats/${activeSessionId}/plan.md`,
              `~/.agverse/chats/${activeSessionId}/PLAN.md`,
              'docs/active_plans/PLAN.md',
              'PLAN.md'
            ]}
            title="Implementation Plan"
            placeholderMessage="Create a PLAN.md file inside your project's docs/active_plans/ directory to display your implementation plan here."
          />
        )}
        {activeTab === 'walkthrough' && (
          <DocumentTab
            projectPath={activeProject?.path}
            relativePaths={[
              `~/.agverse/chats/${activeSessionId}/walkthrough.md`,
              'docs/active_plans/walkthrough.md',
              'docs/walkthrough.md',
              'walkthrough.md',
              'docs/archive/walkthrough.md'
            ]}
            title="Walkthrough"
            placeholderMessage="Create a walkthrough.md file inside your project's docs/active_plans/ or docs/ directory to display your completion walkthrough here."
          />
        )}
      </div>
    </aside>
  );
}
