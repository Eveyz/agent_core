import { useState, useMemo } from 'react';
import { useSelector } from 'react-redux';
import MoreVerticalIcon from 'lucide-react/dist/esm/icons/more-vertical.mjs';
import SearchIcon from 'lucide-react/dist/esm/icons/search.mjs';
import ListTreeIcon from 'lucide-react/dist/esm/icons/list-tree.mjs';
import { RootState } from '../../store';
import { basename, parseUnifiedDiff } from '../chat/turnHelpers';
import { useResizableSidebar } from '../../hooks/useResizableSidebar';
import { FileTree } from '../layout/FileTree';

export function ReviewTab() {
  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const activeProject = projects.find(p => p.id === activeProjectId);

  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [filePaneVisible, setFilePaneVisible] = useState(false);
  
  const { sidebarRef: filePaneRef, onMouseDown: startFilePaneDrag } = useResizableSidebar(250, 150, 600, 'right');

  // Extract modified files from the current session's chat entries
  const entries = useSelector((state: RootState) => state.chat.entries);
  
  const modifiedFiles = useMemo(() => {
    const files = new Map<string, { result: string; timestamp: number }>();
    
    for (const entry of entries) {
      if (entry.type !== 'turn' || !entry.blocks) continue;
      
      for (const block of entry.blocks) {
        if (block.type === 'tool' && !block.is_error && block.result) {
          const name = block.name;
          if (name === 'edit_file' || name === 'replace_file_content' || name === 'multi_replace_file_content' || name === 'write_to_file') {
            const args = block.args as any;
            const path = args?.file_path || args?.TargetFile;
            if (path) {
              files.set(path, { result: block.result, timestamp: entry.endTime || 0 });
            }
          }
        }
      }
    }
    
    return Array.from(files.entries()).map(([path, data]) => ({
      path,
      name: basename(path),
      result: data.result,
    }));
  }, [entries]);

  const selectedFileData = useMemo(() => {
    if (!selectedFile) return null;
    return modifiedFiles.find(f => f.path === selectedFile) || null;
  }, [selectedFile, modifiedFiles]);

  const diffRows = useMemo(() => {
    if (!selectedFileData?.result) return [];
    const diffStart = selectedFileData.result.indexOf('--- ');
    if (diffStart === -1) return [];
    return parseUnifiedDiff(selectedFileData.result.slice(diffStart));
  }, [selectedFileData]);

  return (
    <div className="review-tab-container">
      <div className="review-header">
        <span className="review-title">Review</span>
        <div className="review-actions">
          <button className="icon-btn"><MoreVerticalIcon size={14} /></button>
          <button className="icon-btn"><SearchIcon size={14} /></button>
          <button 
            className="icon-btn" 
            onClick={() => setFilePaneVisible(!filePaneVisible)}
            title={filePaneVisible ? "收起文件树" : "展开文件树"}
          >
            <ListTreeIcon size={14} />
          </button>
        </div>
      </div>
      
      <div className="review-body">
        <div className="review-diff-pane">
          {diffRows.length === 0 ? (
            <div className="empty-message">No changes to review</div>
          ) : (
            <div className="edit-diff-body" style={{ height: '100%', overflowY: 'auto' }}>
              <div className="edit-diff-path">{selectedFileData?.path}</div>
              <div className="edit-diff-table">
                {diffRows.map((row, idx) => (
                  <div key={idx} className={`diff-row diff-${row.type}`}>
                    <span className="diff-lineno diff-old">{row.oldLineNo ?? ''}</span>
                    <span className="diff-linecontent diff-old-content">
                      {row.type === 'add' ? '' : row.oldText}
                    </span>
                    <span className="diff-lineno diff-new">{row.newLineNo ?? ''}</span>
                    <span className="diff-linecontent diff-new-content">
                      {row.type === 'del' ? '' : row.newText}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
        
        {filePaneVisible && (
          <>
            <div className="resizer-handle" onMouseDown={startFilePaneDrag} />
            <div className="review-files-pane" ref={filePaneRef} style={{ width: 250 }}>
              <div className="files-changed-header">Project File Tree</div>
              <div style={{ flex: 1, overflowY: 'auto' }}>
                <FileTree rootPath={activeProject?.path || ''} onSelectFile={setSelectedFile} />
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
