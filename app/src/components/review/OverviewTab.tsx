import { useSelector } from 'react-redux';
import { RootState } from '../../store';

export function OverviewTab() {
  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const activeProject = projects.find((p) => p.id === activeProjectId);

  return (
    <div className="overview-tab-container">
      <div className="overview-header">
        <span className="overview-title">Project Overview</span>
      </div>
      <div className="overview-body">
        {activeProject ? (
          <div className="file-tree-placeholder">
            <div className="tree-root">{activeProject.path}</div>
            <div className="tree-children">
              {/* TODO: Integrate with backend fs api to list directory contents */}
              <div className="tree-item text-muted">Loading files...</div>
            </div>
          </div>
        ) : (
          <div className="empty-message">No active project</div>
        )}
      </div>
    </div>
  );
}
