import React, { useState, useEffect, useMemo, memo } from 'react';
import { useSelector } from 'react-redux';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import LoaderIcon from 'lucide-react/dist/esm/icons/loader.mjs';
import RefreshCwIcon from 'lucide-react/dist/esm/icons/refresh-cw.mjs';
import AlertTriangleIcon from 'lucide-react/dist/esm/icons/alert-triangle.mjs';
import BanIcon from 'lucide-react/dist/esm/icons/ban.mjs';
import FileCheckIcon from 'lucide-react/dist/esm/icons/file-check.mjs';
import type { ChatEntry, TurnBlock } from '../../features/chat/chatSlice';
import type { RootState } from '../../store';
import { formatTime } from '../../utils/format';
import { MarkdownContent } from './MarkdownContent';
import ProcessingTimer from './ProcessingTimer';
import TurnIterationUI from './TurnIterationUI';
import TurnFooter from './TurnFooter';
import { isSubagentTool, groupBlocksIntoItems, basename, parseEditSummary, parseUnifiedDiff, isTrivialAssistantText } from './turnHelpers';
import { getFileIcon } from '../layout/FileTree';
import { useTranslation } from 'react-i18next';

interface FileChangeItem {
  path: string;
  name: string;
  additions: number;
  deletions: number;
  isNew: boolean;
}

/** Mid-turn narration should be short; if the model dumps a long essay into
 *  content, collapse it so the timeline stays scannable. */
const PROGRESS_COLLAPSE_CHARS = 160;

const REMOTE_RETRY_RE =
  /Failed to connect to remote model \(([^)]+)\), retrying in (\d+)s \(attempt (\d+)\/(\d+)\)/i;

function parseRemoteRetry(text: string) {
  const match = text.match(REMOTE_RETRY_RE);
  if (!match) return null;
  return {
    reasonKey: match[1].replace(' ', '_'),
    delaySec: Number(match[2]),
    attempt: match[3],
    maxAttempts: match[4],
  };
}

function translateRecoveryMessage(text: string, t: (key: string, opts?: Record<string, unknown>) => string, countdownSec?: number): string {
  // 1. Compacting
  const compactMatch = text.match(/context too long;\s*compacting to\s*(\d+)%\s*before retry/i);
  if (compactMatch) {
    return t('chat.recovery.compacting', { percentage: compactMatch[1] });
  }

  // 2. Escalating
  const escalateMatch = text.match(/escalating max_tokens to\s*(\d+)/i);
  if (escalateMatch) {
    return t('chat.recovery.escalating', { maxTokens: escalateMatch[1] });
  }

  // 3. Retrying delay
  const delayMatch = text.match(/retrying model call after\s*(\d+)ms/i);
  if (delayMatch) {
    return t('chat.recovery.retryingDelay', { delay: delayMatch[1] });
  }

  // 4. Switching model
  const switchMatch = text.match(/switching to fallback model:\s*(.*)/i);
  if (switchMatch) {
    return t('chat.recovery.switchingModel', { model: switchMatch[1] });
  }

  // 5. Structured remote model retry
  const remoteRetry = parseRemoteRetry(text);
  if (remoteRetry) {
    return t(`chat.recovery.remoteRetry.${remoteRetry.reasonKey}`, {
      time: countdownSec ?? remoteRetry.delaySec,
      attempt: remoteRetry.attempt,
      maxAttempts: remoteRetry.maxAttempts,
    });
  }

  // 6. Legacy retrying in
  const retryingInMatch = text.match(/retrying in\s*(.*)/i);
  if (retryingInMatch) {
    return t('chat.recovery.retryingIn', { time: retryingInMatch[1] });
  }

  // 7. Generic retrying model call
  if (text.toLowerCase().includes('retrying model call')) {
    return t('chat.recovery.retrying');
  }

  return text;
}

function RecoveryNotice({ text, code }: { text: string; code?: string }) {
  const { t } = useTranslation();
  const remoteRetry = useMemo(() => parseRemoteRetry(text), [text]);
  const isActiveRetry =
    code === 'model_retry' ||
    !!remoteRetry ||
    /retrying|compacting|escalating|switching to fallback/i.test(text);

  const [remaining, setRemaining] = useState<number | null>(
    remoteRetry ? remoteRetry.delaySec : null,
  );

  useEffect(() => {
    if (!remoteRetry) {
      setRemaining(null);
      return;
    }
    setRemaining(remoteRetry.delaySec);
    const startedAt = Date.now();
    const id = window.setInterval(() => {
      const left = Math.max(
        0,
        remoteRetry.delaySec - Math.floor((Date.now() - startedAt) / 1000),
      );
      setRemaining(left);
    }, 200);
    return () => window.clearInterval(id);
  }, [text, remoteRetry]);

  const label = translateRecoveryMessage(
    text,
    t as (key: string, opts?: Record<string, unknown>) => string,
    remaining ?? undefined,
  );

  return (
    <div className={`recovery-notice${isActiveRetry ? ' recovery-notice--active' : ''}`}>
      {isActiveRetry ? (
        <RefreshCwIcon size={14} className="recovery-notice-icon" />
      ) : (
        <AlertTriangleIcon size={16} className="recovery-notice-icon-static" />
      )}
      <span className="recovery-notice-text">{label}</span>
    </div>
  );
}

function ProgressNarration({
  text,
  isStreaming,
}: {
  text: string;
  isStreaming: boolean;
}) {
  const { t } = useTranslation();
  const trimmed = text.trim();
  const isLong =
    trimmed.length > PROGRESS_COLLAPSE_CHARS ||
    trimmed.split(/\n/).filter(Boolean).length > 2;
  const [expanded, setExpanded] = useState(false);

  if (isStreaming || !isLong || expanded) {
    if (isStreaming) {
      return (
        <div className="assistant-msg" style={{ whiteSpace: 'pre-wrap' }}>
          {text}
        </div>
      );
    }
    return (
      <div>
        <MarkdownContent content={text} className="assistant-msg" />
        {isLong && (
          <button
            type="button"
            className="progress-expand-btn"
            onClick={() => setExpanded(false)}
          >
            {t('chat.turn.showLess')}
          </button>
        )}
      </div>
    );
  }

  const preview =
    trimmed.length <= PROGRESS_COLLAPSE_CHARS
      ? trimmed
      : `${trimmed.slice(0, PROGRESS_COLLAPSE_CHARS).replace(/\s+\S*$/, '')}…`;

  return (
    <div className="assistant-msg assistant-msg--progress-collapsed">
      <span>{preview}</span>{' '}
      <button
        type="button"
        className="progress-expand-btn"
        onClick={() => setExpanded(true)}
      >
        {t('chat.turn.showMore')}
      </button>
    </div>
  );
}

function FilesChangedCard({ files }: { files: FileChangeItem[] }) {
  const { t } = useTranslation();
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

  const suffix = totalFiles > 1 ? '_plural' : '';

  return (
    <div className="files-changed-card">
      <div className="files-changed-header" role="button" tabIndex={0} aria-expanded={expanded} onClick={() => setExpanded(!expanded)} onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); setExpanded(!expanded); } }}>
        <div className="files-changed-summary">
          {expanded ? <ChevronDownIcon size={14} className="files-changed-chevron" /> : <ChevronRightIcon size={14} className="files-changed-chevron" />}
          <span>
            {t(`chat.turn.filesChanged${suffix}`, { count: totalFiles })}
            <span className="files-changed-summary-text">
              {' '}(<span className="file-change-stat-add">+{totalAdditions}</span>{' '}
              <span className="file-change-stat-del">−{totalDeletions}</span>)
            </span>
          </span>
        </div>
        <button className="files-changed-review-btn" onClick={handleReviewClick}>
          <FileCheckIcon size={12} />
          {t('chat.turn.review')}
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
  const { t } = useTranslation();
  const agentTrace = useSelector((state: RootState) => state.settings.agentTrace);
  const showThinking = agentTrace === 'verbose';
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
    if (toolCount > 0) {
      const suffix = toolCount > 1 ? '_plural' : '';
      parts.push(t(`chat.turn.tools${suffix}`, { count: toolCount }));
    }
    // Only mention thoughts in the collapsed summary when thinking is visible.
    if (showThinking && thoughtCount > 0) {
      const suffix = thoughtCount > 1 ? '_plural' : '';
      parts.push(t(`chat.turn.thoughts${suffix}`, { count: thoughtCount }));
    }
    return parts;
  }, [totalTimeText, toolCount, thoughtCount, showThinking, t]);

  // Intermediate steps: tools always count; thoughts only matter for the header when verbose
  // (or when tools exist — thinking is still stored either way).
  const hasIntermediateSteps = toolCount > 0 || (showThinking && thoughtCount > 0);

  const hasInterruptedBlock = useMemo(() => {
    return entry.blocks?.some(
      (b) => b.type === 'error' && b.text.toLowerCase().includes('interrupted')
    ) ?? false;
  }, [entry.blocks]);

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
                <span>{t('chat.turn.worked', { summary: summaryParts.join(' · ') })}</span>
                {collapsed ? <ChevronRightIcon size={12} className="ml-4" /> : <ChevronDownIcon size={12} className="ml-4" />}
              </>
            )}
          </div>
          <div className="turn-divider" />
        </>
      )}

      {renderItems.map((item, idx) => {
        const isFinalAssistant = lastIterIdx === -1 || idx > lastIterIdx;
        // Expanded (or still streaming): show all assistant narrations.
        // Collapsed when done: only the final answer.
        const showAssistant =
          item.type === 'assistant' &&
          (isProcessing || !collapsed || isFinalAssistant) &&
          !isTrivialAssistantText(item.data.text || '') &&
          (!!item.data.text?.trim() || item.data.isStreaming);

        return (
          <React.Fragment key={item.type === 'iteration' ? item.data.id : `block-${idx}`}>
            {item.type === 'assistant' ? (
              showAssistant && (
                isFinalAssistant ? (
                  item.data.isStreaming ? (
                    <div className="assistant-msg" style={{ whiteSpace: 'pre-wrap' }}>
                      {item.data.text}
                    </div>
                  ) : (
                    <MarkdownContent
                      content={item.data.text}
                      className="assistant-msg"
                    />
                  )
                ) : (
                  <ProgressNarration
                    text={item.data.text}
                    isStreaming={!!item.data.isStreaming}
                  />
                )
              )
            ) : item.type === 'notice' ? (
              <RecoveryNotice text={item.data.text} code={item.data.code} />
            ) : item.type === 'error' ? (
              (() => {
                const text = item.data.text;
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
                          <span style={{ lineHeight: '1.4' }}>{t('chat.turn.longWorkWarning')}</span>
                        </div>
                        {onSend && (
                          <button 
                            className="continue-btn" 
                            onClick={() => onSend("continue")}
                            disabled={isProcessing}
                          >
                            {t('chat.turn.continue')}
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
                            <span className="interrupted-title">{t('chat.turn.interruptedTitle')}</span>
                            <span className="interrupted-subtitle">{t('chat.turn.interruptedSubtitle')}</span>
                          </div>
                        </div>
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
              !collapsed && <TurnIterationUI iteration={item.data} showThinking={showThinking} />
            )}
          </React.Fragment>
        );
      })}

      {entry.interrupted && !hasInterruptedBlock && (
        <div className="interrupted-block-style">
          <div className="interrupted-content">
            <BanIcon size={14} className="interrupted-icon" style={{ flexShrink: 0 }} />
            <div className="interrupted-text-wrapper">
              <span className="interrupted-title">{t('chat.turn.interruptedTitle')}</span>
              <span className="interrupted-subtitle">{t('chat.turn.interruptedSubtitle')}</span>
            </div>
          </div>
        </div>
      )}

      {isDone && turnFilesChanged.length > 0 && (
        <FilesChangedCard files={turnFilesChanged} />
      )}

      <TurnFooter entry={entry} />
    </div>
  );
});
