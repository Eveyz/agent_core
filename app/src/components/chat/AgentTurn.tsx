import React, { useState, useEffect, useMemo, memo } from 'react';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import LoaderIcon from 'lucide-react/dist/esm/icons/loader.mjs';
import AlertTriangleIcon from 'lucide-react/dist/esm/icons/alert-triangle.mjs';
import BanIcon from 'lucide-react/dist/esm/icons/ban.mjs';
import FileCheckIcon from 'lucide-react/dist/esm/icons/file-check.mjs';
import type { ChatEntry, TurnBlock } from '../../features/chat/chatSlice';
import { formatTime } from '../../utils/format';
import { MarkdownContent } from './MarkdownContent';
import ProcessingTimer from './ProcessingTimer';
import TurnIterationUI from './TurnIterationUI';
import TurnFooter from './TurnFooter';
import { isSubagentTool, groupBlocksIntoItems, basename, parseEditSummary, parseUnifiedDiff } from './turnHelpers';
import { getFileIcon } from '../layout/FileTree';

interface FileChangeItem {
  path: string;
  name: string;
  additions: number;
  deletions: number;
  isNew: boolean;
}



function FilesChangedCard({ files }: { files: FileChangeItem[] }) {
  const [expanded, setExpanded] = useState(false);
  
  const { totalFiles, totalAdditions, totalDeletions } = useMemo(() => {
    let adds = 0;
    let dels = 0;
    files.forEach(f => {
      adds += f.additions;
      dels += f.deletions;
    });
    return {
      totalFiles: files.length,
      totalAdditions: adds,
      totalDeletions: dels,
    };
  }, [files]);

  const handleReviewClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    window.dispatchEvent(new CustomEvent('open-right-sidebar', { detail: { tab: 'review' } }));
  };

  const getDirname = (path: string) => {
    const parts = path.split('/');
    if (parts.length <= 1) return '';
    return parts.slice(0, -1).join('/');
  };

  return (
    <div className="files-changed-card">
      <div className="files-changed-header" role="button" tabIndex={0} aria-expanded={expanded} onClick={() => setExpanded(!expanded)} onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); setExpanded(!expanded); } }}>
        <div className="files-changed-summary">
          {expanded ? <ChevronDownIcon size={14} className="files-changed-chevron" /> : <ChevronRightIcon size={14} className="files-changed-chevron" />}
          <span>
            {totalFiles} file{totalFiles > 1 ? 's' : ''} changed
            <span className="files-changed-summary-text">
              {' '}(<span className="file-change-stat-add">+{totalAdditions}</span>{' '}
              <span className="file-change-stat-del">−{totalDeletions}</span>)
            </span>
          </span>
        </div>
        <button className="files-changed-review-btn" onClick={handleReviewClick}>
          <FileCheckIcon size={12} />
          Review
        </button>
      </div>
      
      {expanded && (
        <div className="files-changed-list">
          {files.map(file => {
            const dirname = getDirname(file.path);
            const handleRowClick = () => {
              window.dispatchEvent(new CustomEvent('open-right-sidebar', { detail: { tab: 'review', filePath: file.path } }));
            };
            return (
              <div 
                key={file.path} 
                className="file-change-row" 
                onClick={handleRowClick}
                style={{ cursor: 'pointer' }}
              >
                <div className="file-change-left">
                  <span className="file-icon-wrapper" style={{ marginRight: '6px', display: 'flex', alignItems: 'center' }}>
                    {getFileIcon(file.name)}
                  </span>
                  <span className="file-change-name">{file.name}</span>
                  {dirname && <span className="file-change-path" title={file.path}>{dirname}</span>}
                </div>
                <div className="file-change-stats">
                  {file.additions > 0 && <span className="file-change-stat-add">+{file.additions}</span>}
                  {file.deletions > 0 && <span className="file-change-stat-del">-{file.deletions}</span>}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

export const AgentTurnUI = memo(function AgentTurnUI({
  entry,
  onSend,
}: {
  entry: ChatEntry;
  onSend?: (msg: string) => void;
}) {
  const isProcessing = !!(entry.startTime && !entry.endTime);
  const isDone = !!(entry.endTime);

  // Auto-collapse intermediate steps when turn is done
  const [collapsed, setCollapsed] = useState(false);
  useEffect(() => {
    if (isDone) setCollapsed(true);
  }, [isDone]);

  const { toolCount, thoughtCount } = useMemo(() => {
    let tools = 0, thoughts = 0;
    entry.blocks?.forEach((b: TurnBlock) => {
      if (b.type === 'tool') {
        if (!isSubagentTool(b)) tools++;
      }
      if (b.type === 'thinking') thoughts++;
    });
    return { toolCount: tools, thoughtCount: thoughts };
  }, [entry.blocks]);

  const totalTimeText = useMemo(() => {
    if (!entry.startTime || !entry.endTime) return null;
    return formatTime(entry.endTime - entry.startTime);
  }, [entry.startTime, entry.endTime]);

  const summaryParts = useMemo(() => {
    const parts: string[] = [];
    if (totalTimeText) parts.push(totalTimeText);
    if (toolCount > 0) parts.push(`${toolCount} tool${toolCount > 1 ? 's' : ''}`);
    if (thoughtCount > 0) parts.push(`${thoughtCount} thought${thoughtCount > 1 ? 's' : ''}`);
    return parts;
  }, [totalTimeText, toolCount, thoughtCount]);

  // Check if there are any intermediate blocks at all
  const hasIntermediateSteps = toolCount > 0 || thoughtCount > 0;

  const renderItems = useMemo(() => groupBlocksIntoItems(entry.blocks || []), [entry.blocks]);
  const lastIterIdx = useMemo(() => {
    for (let i = renderItems.length - 1; i >= 0; i--) {
      if (renderItems[i].type === 'iteration') return i;
    }
    return -1;
  }, [renderItems]);

  const turnFilesChanged = useMemo(() => {
    if (!entry.blocks) return [];
    const files = new Map<string, FileChangeItem>();
    
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

    const getFileDiffStats = (result?: string, toolName?: string, rawArgs?: any) => {
      if (!result) return { additions: 0, deletions: 0 };
      
      const summary = parseEditSummary(result);
      if (summary) {
        return { additions: summary.additions, deletions: summary.deletions };
      }
      
      const diffStart = result.indexOf('--- ');
      if (diffStart !== -1) {
        const rows = parseUnifiedDiff(result.slice(diffStart));
        let additions = 0;
        let deletions = 0;
        for (const r of rows) {
          if (r.type === 'add') additions++;
          else if (r.type === 'del') deletions++;
        }
        return { additions, deletions };
      }

      const args = getParsedArgs(rawArgs);
      if (
        (toolName === 'write_to_file' && args?.CodeContent) ||
        (toolName === 'write_file' && args?.content)
      ) {
        const contentStr = args?.CodeContent || args?.content || '';
        const lines = contentStr.split('\n').length;
        return { additions: lines, deletions: 0 };
      }

      return { additions: 0, deletions: 0 };
    };

    for (const b of entry.blocks) {
      if (b.type === 'tool' && !b.is_error && b.result) {
        const name = b.name;
        if (
          name === 'edit_file' || 
          name === 'replace_file_content' || 
          name === 'multi_replace_file_content' || 
          name === 'write_to_file' ||
          name === 'edit' ||
          name === 'write_file'
        ) {
          const args = getParsedArgs(b.args);
          const path = args?.file_path || args?.TargetFile || args?.path;
          if (path) {
            const stats = getFileDiffStats(b.result, name, b.args);
            const isNew = (name === 'write_to_file' && !args?.Overwrite) || (name === 'write_file' && !args?.overwrite);
            const existing = files.get(path);
            if (existing) {
              existing.additions += stats.additions;
              existing.deletions += stats.deletions;
            } else {
              files.set(path, {
                path,
                name: basename(path),
                additions: stats.additions,
                deletions: stats.deletions,
                isNew,
              });
            }
          }
        }
      }
    }
    return Array.from(files.values()).filter(f => f.additions > 0 || f.deletions > 0);
  }, [entry.blocks]);

  return (
    <div className="agent-turn">
      {(hasIntermediateSteps || isProcessing) && (
        <>
          <div
            className={`turn-header ${isProcessing ? 'processing-pulse step-row-default' : 'step-row-pointer'}`}
            onClick={() => {
              if (!isProcessing) setCollapsed(!collapsed);
            }}
          >
            {isProcessing ? (
              <>
                <LoaderIcon className="tool-loader-icon" size={12} />
                <ProcessingTimer startTime={entry.startTime} endTime={entry.endTime} />
                <ChevronDownIcon size={12} className="ml-4" />
              </>
            ) : (
              <>
                <span>Worked {summaryParts.join(' · ')}</span>
                {collapsed ? <ChevronRightIcon size={12} className="ml-4" /> : <ChevronDownIcon size={12} className="ml-4" />}
              </>
            )}
          </div>
          <div className="turn-divider" />
        </>
      )}

      {renderItems.map((item, idx) => {
        return (
          <React.Fragment key={item.type === 'iteration' ? item.data.id : `block-${idx}`}>
            {item.type === 'assistant' ? (
              (lastIterIdx === -1 || idx > lastIterIdx) && (
                <MarkdownContent
                  content={item.data.text}
                  className="assistant-msg"
                />
              )
            ) : item.type === 'error' ? (
              (() => {
                const text = item.data.text;
                const isRecovery = text.includes("retrying model call") ||
                                   text.includes("compacting to") ||
                                   text.includes("escalating max_tokens") ||
                                   text.includes("switching to fallback model") ||
                                   text.includes("retrying in");
                
                if (isRecovery && idx < renderItems.length - 1) {
                  return null;
                }

                if (!collapsed || idx === renderItems.length - 1) {
                  if (
                    text.includes("maximum number of steps") ||
                    text.includes("Reached the maximum number of steps") ||
                    text.includes("reached the maximum number of steps")
                  ) {
                    return (
                      <div className="warning-block-style">
                        <div className="warning-block-content">
                          <AlertTriangleIcon size={16} style={{ flexShrink: 0 }} />
                          <span style={{ lineHeight: '1.4' }}>You have been working in this project in a while.</span>
                        </div>
                        {onSend && (
                          <button 
                            className="continue-btn" 
                            onClick={() => onSend("continue")}
                            disabled={isProcessing}
                          >
                            Continue
                          </button>
                        )}
                      </div>
                    );
                  }
                  
                  if (text.toLowerCase().includes("interrupted")) {
                    return (
                      <div className="interrupted-block-style">
                        <div className="interrupted-content">
                          <BanIcon size={14} className="interrupted-icon" style={{ flexShrink: 0 }} />
                          <div className="interrupted-text-wrapper">
                            <span className="interrupted-title">Execution Interrupted</span>
                            <span className="interrupted-subtitle">The task was cancelled before completion</span>
                          </div>
                        </div>
                      </div>
                    );
                  }

                  if (isRecovery) {
                    return (
                      <div className="warning-block-style-simple">
                        <AlertTriangleIcon size={16} style={{ flexShrink: 0 }} />
                        <span style={{ lineHeight: '1.4' }}>{text}</span>
                      </div>
                    );
                  }

                  return (
                    <div className="error-block-style">
                      <AlertTriangleIcon size={16} style={{ flexShrink: 0 }} />
                      <span style={{ lineHeight: '1.4' }}>{text}</span>
                    </div>
                  );
                }
                
                return null;
              })()
            ) : (
              !collapsed && <TurnIterationUI iteration={item.data} />
            )}
          </React.Fragment>
        );
      })}

      {isDone && turnFilesChanged.length > 0 && (
        <FilesChangedCard files={turnFilesChanged} />
      )}

      <TurnFooter entry={entry} />
    </div>
  );
});
