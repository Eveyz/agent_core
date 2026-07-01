import { useState, useMemo, useEffect, useCallback } from 'react';
import { useSelector } from 'react-redux';
import MoreVerticalIcon from 'lucide-react/dist/esm/icons/more-vertical.mjs';
import SearchIcon from 'lucide-react/dist/esm/icons/search.mjs';
import ListIcon from 'lucide-react/dist/esm/icons/list.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import { invoke } from '@tauri-apps/api/core';
import { RootState } from '../../store';
import { basename, parseUnifiedDiff, parseEditSummary } from '../chat/turnHelpers';
import { getFileIcon } from '../layout/FileTree';

interface FileDiffViewerProps {
  diffRows: any[];
  fileContent?: string;
  onFetchContent: () => void;
}

function FileDiffViewer({ diffRows, fileContent, onFetchContent }: FileDiffViewerProps) {
  const [items, setItems] = useState<any[]>([]);

  // Calculate gaps
  const getDiffItems = useCallback((rows: any[], content?: string) => {
    if (rows.length === 0) return [];
    
    const res: any[] = [];
    let firstNewLine = 1;
    
    for (const r of rows) {
      if (r.newLineNo !== null) { firstNewLine = r.newLineNo; }
      if (r.oldLineNo !== null || r.newLineNo !== null) break;
    }
    
    if (firstNewLine > 1) {
      res.push({
        type: 'gap',
        gapSize: firstNewLine - 1,
        gapStartOld: 1,
        gapStartNew: 1,
      });
    }
    
    for (let i = 0; i < rows.length; i++) {
      const row = rows[i];
      const newLine = row.newLineNo;
      
      if (i > 0) {
        let prevNewLine = 0;
        let prevOldLine = 0;
        for (let j = i - 1; j >= 0; j--) {
          if (rows[j].newLineNo !== null && prevNewLine === 0) {
            prevNewLine = rows[j].newLineNo;
          }
          if (rows[j].oldLineNo !== null && prevOldLine === 0) {
            prevOldLine = rows[j].oldLineNo;
          }
          if (prevNewLine !== 0 && prevOldLine !== 0) break;
        }
        
        if (newLine !== null && prevNewLine !== 0 && newLine > prevNewLine + 1) {
          const gapSize = newLine - prevNewLine - 1;
          res.push({
            type: 'gap',
            gapSize,
            gapStartOld: prevOldLine + 1,
            gapStartNew: prevNewLine + 1,
          });
        }
      }
      
      res.push(row);
    }
    
    if (content) {
      const totalLines = content.split('\n').length;
      let finalNewLine = 0;
      let finalOldLine = 0;
      for (let j = rows.length - 1; j >= 0; j--) {
        if (rows[j].newLineNo !== null && finalNewLine === 0) {
          finalNewLine = rows[j].newLineNo;
        }
        if (rows[j].oldLineNo !== null && finalOldLine === 0) {
          finalOldLine = rows[j].oldLineNo;
        }
        if (finalNewLine !== 0 && finalOldLine !== 0) break;
      }
      
      if (finalNewLine !== 0 && totalLines > finalNewLine) {
        res.push({
          type: 'gap',
          gapSize: totalLines - finalNewLine,
          gapStartOld: finalOldLine + 1,
          gapStartNew: finalNewLine + 1,
        });
      }
    }
    
    return res;
  }, []);

  useEffect(() => {
    let activeRows = diffRows;
    if (diffRows.length === 0 && fileContent) {
      const lines = fileContent.split('\n');
      activeRows = lines.map((line, idx) => ({
        oldLineNo: null,
        newLineNo: idx + 1,
        oldText: '',
        newText: line,
        type: 'add'
      }));
    }
    setItems(getDiffItems(activeRows, fileContent));
  }, [diffRows, fileContent, getDiffItems]);

  useEffect(() => {
    if (!fileContent) {
      onFetchContent();
    }
  }, [fileContent, onFetchContent]);

  const expandGap = (gapIndex: number, direction: 'top' | 'bottom' | 'all') => {
    if (!fileContent) return;
    const fileLines = fileContent.split('\n');
    
    setItems(prev => {
      const next = [...prev];
      const gap = next[gapIndex];
      if (gap.type !== 'gap') return prev;
      
      const { gapSize, gapStartOld, gapStartNew } = gap;
      
      let linesToExpand = gapSize;
      let startOffset = 0;
      
      if (direction === 'top') {
        linesToExpand = Math.min(10, gapSize);
        startOffset = gapSize - linesToExpand;
      } else if (direction === 'bottom') {
        linesToExpand = Math.min(10, gapSize);
        startOffset = 0;
      }
      
      const expandedRows = [];
      for (let i = 0; i < linesToExpand; i++) {
        const currentOffset = startOffset + i;
        const newNo = gapStartNew + currentOffset;
        const oldNo = gapStartOld + currentOffset;
        
        const lineText = fileLines[newNo - 1] || '';
        expandedRows.push({
          oldLineNo: oldNo,
          newLineNo: newNo,
          oldText: lineText,
          newText: lineText,
          type: 'context'
        });
      }
      
      const newItems = [];
      
      if (direction === 'top' && startOffset > 0) {
        newItems.push({
          type: 'gap',
          gapSize: startOffset,
          gapStartOld,
          gapStartNew
        });
      }
      
      newItems.push(...expandedRows);
      
      if (direction === 'bottom' && gapSize > linesToExpand) {
        newItems.push({
          type: 'gap',
          gapSize: gapSize - linesToExpand,
          gapStartOld: gapStartOld + linesToExpand,
          gapStartNew: gapStartNew + linesToExpand
        });
      }
      
      next.splice(gapIndex, 1, ...newItems);
      return next;
    });
  };

  return (
    <div className="unified-diff-table">
      {items.map((row, idx) => {
        if (row.type === 'gap') {
          return (
            <div key={`gap-${idx}`} className="unified-diff-row diff-gap" style={{ display: 'grid', gridTemplateColumns: '7em 1fr', height: '36px', borderTop: '1px solid var(--border-color)', borderBottom: '1px solid var(--border-color)', backgroundColor: 'var(--bg-2, #f3f4f6)' }}>
              <div style={{ display: 'flex', borderRight: '1px solid var(--border-color)', justifyContent: 'space-around', alignItems: 'center', padding: '0 4px' }}>
                <button 
                  onClick={() => expandGap(idx, 'bottom')}
                  style={{
                    fontSize: '9px',
                    padding: '2px 4px',
                    borderRadius: '3px',
                    border: '1px solid var(--border-color)',
                    background: 'var(--bg-1, #ffffff)',
                    color: 'var(--text-muted)',
                    cursor: 'pointer',
                    lineHeight: 1,
                    fontWeight: 600
                  }}
                  title="Expand 10 lines down"
                >
                  +10
                </button>
                <button 
                  onClick={() => expandGap(idx, 'top')}
                  style={{
                    fontSize: '9px',
                    padding: '2px 4px',
                    borderRadius: '3px',
                    border: '1px solid var(--border-color)',
                    background: 'var(--bg-1, #ffffff)',
                    color: 'var(--text-muted)',
                    cursor: 'pointer',
                    lineHeight: 1,
                    fontWeight: 600
                  }}
                  title="Expand 10 lines up"
                >
                  +10
                </button>
              </div>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', position: 'relative', height: '100%' }}>
                <div style={{ position: 'absolute', left: 0, right: 0, top: '50%', borderTop: '1px solid var(--border-color)', zIndex: 1 }} />
                <button
                  onClick={() => expandGap(idx, 'all')}
                  style={{
                    position: 'relative',
                    zIndex: 2,
                    fontSize: '11px',
                    padding: '3px 10px',
                    borderRadius: '12px',
                    border: '1px solid var(--border-color)',
                    background: 'var(--bg-1, #ffffff)',
                    color: 'var(--text-muted)',
                    cursor: 'pointer',
                    fontWeight: 500,
                  }}
                >
                  +{row.gapSize} more lines
                </button>
              </div>
            </div>
          );
        }

        return (
          <div key={idx} className={`unified-diff-row diff-${row.type}`}>
            <span className="unified-diff-lineno">{row.oldLineNo ?? ''}</span>
            <span className="unified-diff-lineno">{row.newLineNo ?? ''}</span>
            <span className="unified-diff-content">
              {row.type === 'add' ? row.newText : row.oldText}
            </span>
          </div>
        );
      })}
    </div>
  );
}

export function ReviewTab() {
  const [expandedFiles, setExpandedFiles] = useState<Set<string>>(new Set());
  const [fileContents, setFileContents] = useState<Record<string, string>>({});

  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const activeProject = projects.find((p) => p.id === activeProjectId);
  const entries = useSelector((state: RootState) => state.chat.entries);

  const fetchFileContent = async (path: string) => {
    try {
      const content = await invoke<string>('read_file', { path });
      setFileContents(prev => ({ ...prev, [path]: content }));
    } catch (err) {
      console.error("Failed to read file for diff context:", err);
    }
  };

  const toggleFileExpanded = (path: string) => {
    setExpandedFiles(prev => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
        if (!fileContents[path]) {
          fetchFileContent(path);
        }
      }
      return next;
    });
  };

  useEffect(() => {
    const handleOpen = (e: Event) => {
      const customEvent = e as CustomEvent;
      if (customEvent.detail?.filePath) {
        const filePath = customEvent.detail.filePath;
        setExpandedFiles(prev => {
          const next = new Set(prev);
          next.add(filePath);
          return next;
        });
        if (!fileContents[filePath]) {
          fetchFileContent(filePath);
        }
        setTimeout(() => {
          const id = `review-file-${filePath}`;
          const element = document.getElementById(id);
          if (element) {
            element.scrollIntoView({ behavior: 'smooth', block: 'center' });
          }
        }, 150);
      }
    };
    window.addEventListener('open-right-sidebar', handleOpen);
    return () => window.removeEventListener('open-right-sidebar', handleOpen);
  }, [fileContents]);

  // Extract modified files from the current session's chat entries
  const modifiedFiles = useMemo(() => {
    const files = new Map<string, { result: string; timestamp: number; args?: any }>();
    const getParsedArgs = (rawArgs: any) => {
      if (typeof rawArgs === 'string') {
        try {
          return JSON.parse(rawArgs);
        } catch {
          return {};
        }
      }
      return rawArgs || {};
    };
    
    for (const entry of entries) {
      if (entry.type !== 'turn' || !entry.blocks) continue;
      
      for (const block of entry.blocks) {
        if (block.type === 'tool' && !block.is_error && block.result) {
          const name = block.name;
          if (
            name === 'edit_file' || 
            name === 'replace_file_content' || 
            name === 'multi_replace_file_content' || 
            name === 'write_to_file' ||
            name === 'edit' ||
            name === 'write_file'
          ) {
            const args = getParsedArgs(block.args);
            const path = args?.file_path || args?.TargetFile || args?.path;
            if (path) {
              files.set(path, { result: block.result, timestamp: entry.endTime || 0, args });
            }
          }
        }
      }
    }
    
    return Array.from(files.entries()).map(([path, data]) => {
      let diffRows: any[] = [];
      const diffStart = data.result.indexOf('--- ');
      if (diffStart !== -1) {
        diffRows = parseUnifiedDiff(data.result.slice(diffStart));
      } else {
        diffRows = parseUnifiedDiff(data.result);
      }
      
      let additions = diffRows.filter(r => r.type === 'add').length;
      let deletions = diffRows.filter(r => r.type === 'del').length;
      
      if (additions === 0 && deletions === 0) {
        const summary = parseEditSummary(data.result);
        if (summary) {
          additions = summary.additions;
          deletions = summary.deletions;
        } else if (data.args?.content) {
          additions = data.args.content.split('\n').length;
          deletions = 0;
        } else if (data.result.includes("Successfully wrote")) {
          additions = 1;
          deletions = 0;
        }
      }

      return {
        path,
        name: basename(path),
        result: data.result,
        additions,
        deletions,
        diffRows
      };
    });
  }, [entries]);

  const getPathHint = (path: string) => {
    if (!activeProject?.path) return '';
    const normalizedPath = path.replace(/\\/g, '/');
    const normalizedRoot = activeProject.path.replace(/\\/g, '/');
    
    if (normalizedPath.startsWith(normalizedRoot)) {
      const rel = normalizedPath.slice(normalizedRoot.length).replace(/^\//, '');
      const parts = rel.split('/');
      parts.pop(); // remove file name
      return parts.join('/');
    }
    
    const parts = normalizedPath.split('/');
    parts.pop();
    const parent = parts.pop();
    return parent ? `.../${parent}` : '';
  };

  return (
    <div className="review-tab-container">
      <div className="review-header">
        <span className="review-title">Review</span>
        <div className="review-actions">
          <button className="icon-btn"><MoreVerticalIcon size={14} /></button>
          <button className="icon-btn"><SearchIcon size={14} /></button>
          <button className="icon-btn"><ListIcon size={14} /></button>
        </div>
      </div>
      
      <div className="review-body" style={{ flex: 1, overflowY: 'auto', display: 'flex', flexDirection: 'column' }}>
        {modifiedFiles.length === 0 ? (
          <div className="empty-message">No changes to review</div>
        ) : (
          <div className="review-files-list" style={{ display: 'flex', flexDirection: 'column' }}>
            {modifiedFiles.map((file) => {
              const isExpanded = expandedFiles.has(file.path);
              const pathHint = getPathHint(file.path);
              
              return (
                <div 
                  key={file.path} 
                  id={`review-file-${file.path}`}
                  className="review-file-item" 
                  style={{ borderBottom: '1px solid var(--border-color)' }}
                >
                  <div 
                    className={`review-file-header ${isExpanded ? 'active' : ''}`}
                    onClick={() => toggleFileExpanded(file.path)}
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      padding: '12px 16px',
                      cursor: 'pointer',
                      userSelect: 'none',
                      transition: 'background-color 0.15s',
                      backgroundColor: isExpanded ? 'var(--bg-1, #ffffff)' : 'transparent'
                    }}
                    onMouseEnter={(e) => {
                      if (!isExpanded) e.currentTarget.style.backgroundColor = 'var(--overlay-0_04, rgba(0,0,0,0.02))';
                    }}
                    onMouseLeave={(e) => {
                      if (!isExpanded) e.currentTarget.style.backgroundColor = 'transparent';
                    }}
                  >
                    <span className="file-icon-wrapper" style={{ marginRight: '8px', display: 'flex', alignItems: 'center' }}>
                      {getFileIcon(file.name)}
                    </span>
                    <span className="file-name" style={{ fontSize: '13px', fontWeight: 500, color: 'var(--text-main)' }}>
                      {file.name}
                    </span>
                    {pathHint && (
                      <span className="path-hint" style={{ fontSize: '11px', color: 'var(--text-muted)', marginLeft: '6px', flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                        {pathHint}
                      </span>
                    )}
                    {!pathHint && <div style={{ flex: 1 }} />}
                    
                    <div style={{ display: 'flex', alignItems: 'center', gap: '8px', marginRight: '12px', fontSize: '12px', fontWeight: 600 }}>
                      {file.additions > 0 && <span style={{ color: 'var(--success)' }}>+{file.additions}</span>}
                      {file.deletions > 0 && <span style={{ color: 'var(--danger)' }}>-{file.deletions}</span>}
                      {file.additions === 0 && file.deletions === 0 && <span style={{ color: 'var(--text-muted)' }}>0</span>}
                    </div>
                    
                    <span style={{ color: 'var(--text-muted)', display: 'flex', alignItems: 'center' }}>
                      {isExpanded ? <ChevronDownIcon size={14} /> : <ChevronRightIcon size={14} />}
                    </span>
                  </div>
                  
                  {isExpanded && (
                    <FileDiffViewer 
                      diffRows={file.diffRows}
                      fileContent={fileContents[file.path]}
                      onFetchContent={() => fetchFileContent(file.path)}
                    />
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
