import { useState, useMemo, useEffect } from 'react';
import { useSelector } from 'react-redux';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import PanelRightCloseIcon from 'lucide-react/dist/esm/icons/panel-right-close.mjs';
import FilePlusIcon from 'lucide-react/dist/esm/icons/file-plus.mjs';
import ListIcon from 'lucide-react/dist/esm/icons/list.mjs';
import FileTextIcon from 'lucide-react/dist/esm/icons/file-text.mjs';
import BookOpenIcon from 'lucide-react/dist/esm/icons/book-open.mjs';
import { ReviewTab } from '../review/ReviewTab';
import { OverviewTab } from '../review/OverviewTab';
import { DocumentTab } from '../review/DocumentTab';
import { PreviewPanel } from '../preview/PreviewPanel';
import { RootState } from '../../store';
import { selectActiveSessionEntries } from '../../features/chat/selectors';
import { selectHasPreviewSession, selectPreviewPanelOpen, showPreviewPanel } from '../../features/preview/previewSlice';
import MonitorIcon from 'lucide-react/dist/esm/icons/monitor.mjs';

interface RightSidebarProps {
  sidebarRef: React.RefObject<HTMLDivElement | null>;
  isExpanded: boolean;
  onToggle: () => void;
}

export function RightSidebar({ sidebarRef, isExpanded, onToggle }: RightSidebarProps) {
  const dispatch = useAppDispatch();
  const [activeTab, setActiveTab] = useState<'overview' | 'review' | 'plan' | 'walkthrough' | 'preview'>('overview');
  const previewPanelOpen = useSelector(selectPreviewPanelOpen);
  const hasPreviewSession = useSelector(selectHasPreviewSession);
  const activePreviewId = useSelector((state: RootState) => state.preview.activePreviewId);

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
        if (
          block.type === 'tool' &&
          !block.is_error &&
          block.result &&
          (block.name === 'write_to_file' || block.name === 'write_file')
        ) {
          const args = block.args as Record<string, unknown> | undefined;
          const path =
            (args?.TargetFile as string | undefined) ||
            (args?.file_path as string | undefined) ||
            (args?.path as string | undefined);
          if (path) {
            files.add(path);
          }
        }
      }
    }
    return files;
  }, [entries]);

  const hasPlan = useMemo(() => {
    return Array.from(artifacts).some((path) => {
      const lower = path.toLowerCase();
      return lower.includes('plan.md') || lower.includes('implementation_plan.md');
    });
  }, [artifacts]);

  const hasWalkthrough = useMemo(() => {
    return Array.from(artifacts).some((path) => path.toLowerCase().includes('walkthrough.md'));
  }, [artifacts]);

  const planPaths = useMemo(() => {
    const fromSession = Array.from(artifacts).filter((path) => {
      const lower = path.toLowerCase();
      return lower.endsWith('plan.md') || lower.endsWith('implementation_plan.md');
    });
    return [...fromSession, 'docs/active_plans/PLAN.md', 'PLAN.md'];
  }, [artifacts]);

  const walkthroughPaths = useMemo(() => {
    const fromSession = Array.from(artifacts).filter((path) =>
      path.toLowerCase().endsWith('walkthrough.md')
    );
    return [
      ...fromSession,
      'docs/active_plans/walkthrough.md',
      'docs/walkthrough.md',
      'walkthrough.md',
      'docs/archive/walkthrough.md',
    ];
  }, [artifacts]);

  useEffect(() => {
    if (previewPanelOpen) {
      setActiveTab('preview');
    }
  }, [previewPanelOpen]);

  const reopenPreview = () => {
    if (!activePreviewId) return;
    void dispatch(showPreviewPanel(activePreviewId));
  };

  // Reset tab if it disappears
  useEffect(() => {
    if (activeTab === 'plan' && !hasPlan) {
      setActiveTab('overview');
    } else if (activeTab === 'walkthrough' && !hasWalkthrough) {
      setActiveTab('overview');
    } else if (activeTab === 'preview' && !hasPreviewSession) {
      setActiveTab('overview');
    }
  }, [activeTab, hasPlan, hasWalkthrough, hasPreviewSession]);

  useEffect(() => {
    const handleOpen = (e: Event) => {
      const customEvent = e as CustomEvent<{ tab: 'overview' | 'review' | 'plan' | 'walkthrough' | 'preview' }>;
      if (customEvent.detail?.tab) {
        setActiveTab(customEvent.detail.tab);
      }
    };
    window.addEventListener('open-right-sidebar', handleOpen);
    return () => window.removeEventListener('open-right-sidebar', handleOpen);
  }, []);

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
          {hasPreviewSession && (
            <button
              className={`tab-btn ${activeTab === 'preview' ? 'active' : ''}`}
              onClick={() => {
                setActiveTab('preview');
                reopenPreview();
              }}
            >
              <MonitorIcon size={14} className="tab-icon" />
              Preview
            </button>
          )}
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
        {activeTab === 'preview' && hasPreviewSession && <PreviewPanel />}
        {activeTab === 'plan' && (
          <DocumentTab
            projectPath={activeProject?.path}
            relativePaths={planPaths}
            title="Implementation Plan"
            placeholderMessage="Create a PLAN.md file inside your project's docs/active_plans/ directory to display your implementation plan here."
          />
        )}
        {activeTab === 'walkthrough' && (
          <DocumentTab
            projectPath={activeProject?.path}
            relativePaths={walkthroughPaths}
            title="Walkthrough"
            placeholderMessage="Create a walkthrough.md file inside your project's docs/active_plans/ or docs/ directory to display your completion walkthrough here."
          />
        )}
      </div>
    </aside>
  );
}
