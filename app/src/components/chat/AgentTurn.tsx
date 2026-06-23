import React, { useState, useEffect, memo, useMemo, useRef, useCallback } from 'react';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import LoaderIcon from 'lucide-react/dist/esm/icons/loader.mjs';
import CheckCircle2Icon from 'lucide-react/dist/esm/icons/check-circle-2.mjs';
import XCircleIcon from 'lucide-react/dist/esm/icons/x-circle.mjs';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import BrainIcon from 'lucide-react/dist/esm/icons/brain.mjs';
import { invoke } from '@tauri-apps/api/core';
import { toolApprovalResponded } from '../../features/chat/chatSlice';
import type { ChatEntry, TurnBlock, SubagentEntry } from '../../features/chat/chatSlice';
import { MarkdownContent, formatTime, parseMarkdown } from './MarkdownContent';

export { formatTime, parseMarkdown };

const ml4 = { marginLeft: '4px' };

const ProcessingTimer = memo(function ProcessingTimer({
  startTime,
  endTime,
}: {
  startTime?: number;
  endTime?: number;
}) {
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    if (!startTime || endTime) return;
    const interval = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(interval);
  }, [startTime, endTime]);

  if (!startTime) return null;
  if (endTime) return <span>Processed {formatTime(endTime - startTime)}</span>;
  const diff = now - startTime;
  return <span>Processed {formatTime(diff)}</span>;
});


interface TurnIteration {
  id: string;
  thinkingBlock?: TurnBlock;
  toolBlocks: TurnBlock[];
  errorBlocks: TurnBlock[];
  isLast: boolean;
}

export type TurnRenderItem =
  | { type: 'iteration'; data: TurnIteration }
  | { type: 'assistant'; data: TurnBlock };

function groupBlocksIntoItems(blocks: TurnBlock[]): TurnRenderItem[] {
  const items: TurnRenderItem[] = [];
  let currentIter: TurnIteration | null = null;
  
  const pushCurrentIter = () => {
    if (currentIter) {
      items.push({ type: 'iteration', data: currentIter });
      currentIter = null;
    }
  };

  blocks.forEach((b, idx) => {
    if (b.type === 'assistant') {
      pushCurrentIter();
      items.push({ type: 'assistant', data: b });
      return;
    }
    
    if (b.type === 'thinking') {
      pushCurrentIter();
      currentIter = { id: `iter-${idx}`, thinkingBlock: b, toolBlocks: [], errorBlocks: [], isLast: false };
    } else if (b.type === 'error') {
      if (!currentIter) currentIter = { id: `iter-init-${idx}`, toolBlocks: [], errorBlocks: [], isLast: false };
      currentIter.errorBlocks.push(b);
    } else {
      if (!currentIter) currentIter = { id: `iter-init-${idx}`, toolBlocks: [], errorBlocks: [], isLast: false };
      currentIter.toolBlocks.push(b);
    }
  });
  
  pushCurrentIter();
  
  const lastIter = items.slice().reverse().find(i => i.type === 'iteration');
  if (lastIter && lastIter.type === 'iteration') {
    lastIter.data.isLast = true;
  }
  
  return items;
}

const TurnIterationUI = memo(function TurnIterationUI({
  iteration,
  entry,
}: {
  iteration: TurnIteration;
  entry?: ChatEntry | null;
}) {
  const isStreaming = !!(iteration.thinkingBlock as any)?.isStreaming;
  const [collapsed, setCollapsed] = useState(!isStreaming && !iteration.isLast);

  useEffect(() => {
    if (!isStreaming && !iteration.isLast) setCollapsed(true);
    else setCollapsed(false);
  }, [isStreaming, iteration.isLast]);

  const toolCount = useMemo(() => {
    return iteration.toolBlocks.filter(b => b.type === 'tool' && b.name !== 'invoke_subagent' && b.name !== 'subagent' && b.name !== 'subagents').length;
  }, [iteration.toolBlocks]);

  const durationText = useMemo(() => {
    let text = 'Thought';
    if (isStreaming) {
      text = 'Thinking...';
    } else if ((iteration.thinkingBlock as any)?.startTime && (iteration.thinkingBlock as any)?.endTime) {
      text = `Thought for ${formatTime((iteration.thinkingBlock as any).endTime - (iteration.thinkingBlock as any).startTime)}`;
    }
    if (toolCount > 0) {
      text += ` · ${toolCount} tool${toolCount > 1 ? 's' : ''}`;
    }
    return text;
  }, [isStreaming, iteration.thinkingBlock, toolCount]);

  const hasErrors = iteration.errorBlocks.length > 0;

  return (
    <div className="step-block">
      <div
        className={`step-row ${isStreaming ? 'step-row-active' : ''} ${hasErrors ? 'step-row-error' : ''}`}
        onClick={() => setCollapsed(!collapsed)}
      >
        <BrainIcon size={13} className={`step-icon ${isStreaming ? 'step-icon-thinking' : ''}`} color={isStreaming ? undefined : "#888"} />
        <span className="step-label" style={{ fontWeight: 500 }}>{durationText}</span>
        {collapsed ? <ChevronRightIcon size={12} className="step-chevron" /> : <ChevronDownIcon size={12} className="step-chevron" />}
      </div>
      {!collapsed && (
        <div className="iteration-body" style={{ marginLeft: '6px', paddingLeft: '12px', borderLeft: '1px solid var(--text-muted)', display: 'flex', flexDirection: 'column', gap: '8px', marginTop: '6px', paddingBottom: '4px' }}>
          {iteration.thinkingBlock && (
            <div className="thinking-block">
              {typeof (iteration.thinkingBlock as any).text === 'string' ? (iteration.thinkingBlock as any).text : JSON.stringify((iteration.thinkingBlock as any).text)}
              {isStreaming ? <span className="typing-dot" style={{ display: 'inline-block', marginLeft: '4px' }}>...</span> : null}
            </div>
          )}
          {iteration.toolBlocks.map((b: TurnBlock, idx: number) => {
            if (b.type === 'tool') {
              const name = typeof b.name === 'string' ? b.name : JSON.stringify(b.name);
              if (name === 'invoke_subagent' || name === 'subagent' || name === 'subagents') {
                return null;
              }
              const result = typeof b.result === 'string' ? b.result : JSON.stringify(b.result);
              return (
                <ToolBlockUI
                  key={`tool-${b.call_id}-${idx}`}
                  name={name}
                  args={b.args}
                  result={result}
                  active={b.active}
                  is_error={b.is_error}
                />
              );
            } else if (b.type === 'approval') {
              return <ApprovalBlockUI key={`approval-${b.prompt_id}-${idx}`} block={b as ApprovalBlock} />;
            } else if (b.type === 'subagent_ref') {
              const sa = entry?.subagents?.[b.subagent_id!];
              if (!sa) return null;
              return (
                <div key={`subagent-${b.subagent_id}-${idx}`} className="subagents-section">
                  <SubagentCard subagent={sa} entry={entry} />
                </div>
              );
            }
            return null;
          })}
          {iteration.errorBlocks.map((b: TurnBlock, idx: number) => {
             const text = typeof (b as any).text === 'string' ? (b as any).text : JSON.stringify((b as any).text);
             return <div key={`error-${idx}`} className="error-msg">{text}</div>;
          })}
        </div>
      )}
    </div>
  );
});




const ToolBlockUI = memo(function ToolBlockUI({
  name,
  args,
  result,
  active,
  is_error,
}: {
  name: string;
  args?: unknown;
  result?: string;
  active?: boolean;
  is_error?: boolean;
}) {
  const [collapsed, setCollapsed] = useState(!active);
  const [showMore, setShowMore] = useState(false);

  useEffect(() => {
    if (active === false) setCollapsed(true);
    else if (active === true) setCollapsed(false);
  }, [active]);

  const statusIcon = useMemo(() => {
    if (active) return <LoaderIcon className="tool-loader-icon" size={13} color="var(--text-muted)" />;
    if (is_error) return <XCircleIcon color="#f87171" size={13} />;
    return <CheckCircle2Icon color="var(--success)" size={13} />;
  }, [active, is_error]);

  const formattedArgs = useMemo(() => {
    if (!args) return '';
    if (typeof args === 'string') return args;
    try {
      return JSON.stringify(args, null, 2);
    } catch {
      return String(args);
    }
  }, [args]);

  const displayResult = useMemo(() => {
    if (!result) return '';
    if (!showMore && result.length > 500) {
      return result.substring(0, 500) + '...\n\n*(Truncated. Click Show More to see full output)*';
    }
    return result;
  }, [result, showMore]);

  return (
    <div className="step-block">
      <div
        className={`step-row ${active ? 'step-row-active' : ''} ${is_error ? 'step-row-error' : ''}`}
        onClick={() => setCollapsed(!collapsed)}
      >
        <span className="step-icon">{statusIcon}</span>
        <span className="step-label tool-name">{name}</span>
        {collapsed ? <ChevronRightIcon size={12} className="step-chevron" /> : <ChevronDownIcon size={12} className="step-chevron" />}
      </div>
      {!collapsed && (
        <div className="step-body">
          {formattedArgs && (
            <div className="tool-section">
              <div className="tool-section-label">INPUT</div>
              <pre className="tool-args-pre">{formattedArgs}</pre>
            </div>
          )}
          {result && !active && (
            <div className="tool-section">
              <div className="tool-section-label">OUTPUT</div>
              <MarkdownContent content={displayResult} className="tool-result-content assistant-msg" />
              {!showMore && result.length > 500 && (
                <div 
                  className="tool-show-more-btn"
                  onClick={(e) => { e.stopPropagation(); setShowMore(true); }}
                  style={{
                    color: 'var(--accent)',
                    cursor: 'pointer',
                    fontSize: '12px',
                    textAlign: 'center',
                    padding: '6px 0',
                    marginTop: '8px',
                    borderRadius: '6px',
                    background: 'var(--overlay-0_04)',
                    border: '1px solid var(--overlay-0_06)',
                    transition: 'background 0.2s ease',
                  }}
                  onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--overlay-0_06)')}
                  onMouseLeave={(e) => (e.currentTarget.style.background = 'var(--overlay-0_04)')}
                >
                  Show More
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
});

interface ApprovalBlock {
  prompt_id?: string;
  tool_name?: string;
  tool_input?: unknown;
  danger_level?: string;
  explanation?: string;
  status?: 'pending' | 'approved' | 'denied';
}

const APPROVAL_LABELS: Record<string, string> = {
  deny: 'Denied (once)',
  deny_persistent: 'Denied (always)',
  allow_once: 'Allowed (once)',
  allow_session: 'Allowed (session)',
  allow_persistent: 'Allowed (always)',
};

const ApprovalBlockUI = memo(function ApprovalBlockUI({ block }: { block: ApprovalBlock }) {
  const dispatch = useAppDispatch();
  const [chosenAction, setChosenAction] = useState<string | null>(null);

  const promptId = block.prompt_id ?? '';
  const handleApprove = async (choice: string) => {
    setChosenAction(choice);
    dispatch(toolApprovalResponded({ promptId, approved: !choice.startsWith('deny') }));
    try {
      await invoke('approve_tool', { promptId, choice });
    } catch (e) {
      console.error('Failed to approve tool', e);
    }
  };

  const isResolved = block.status === 'approved' || block.status === 'denied';
  const statusLabel = chosenAction
    ? APPROVAL_LABELS[chosenAction] ?? (block.status === 'approved' ? 'Approved' : 'Denied')
    : block.status === 'approved' ? 'Approved' : block.status === 'denied' ? 'Denied' : '';

  if (isResolved) {
    return (
      <div className="approval-block approval-resolved">
        <div className="approval-header">
          <span className="approval-title">{block.tool_name}</span>
          {block.danger_level ? (
            <span className={`danger-badge danger-${block.danger_level}`}>{block.danger_level}</span>
          ) : null}
          <span className={`approval-status-badge ${block.status === 'approved' ? 'status-approved' : 'status-denied'}`}>
            {statusLabel}
          </span>
        </div>
      </div>
    );
  }

  return (
    <div className="approval-block">
      <div className="approval-header">
        <span className="approval-title">Approval Required: {block.tool_name}</span>
        {block.danger_level ? (
          <span className={`danger-badge danger-${block.danger_level}`}>{block.danger_level}</span>
        ) : null}
      </div>
      <div className="approval-explanation">{block.explanation}</div>
      <div className="approval-args">
        <pre>{typeof block.tool_input === 'string' ? block.tool_input : JSON.stringify(block.tool_input, null, 2)}</pre>
      </div>
      <div className="approval-actions" style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
        <button className="btn-deny" onClick={() => handleApprove('deny')}>Deny Once</button>
        <button className="btn-deny" onClick={() => handleApprove('deny_persistent')}>Deny Always</button>
        <button className="btn-allow" onClick={() => handleApprove('allow_once')}>Allow Once</button>
        <button className="btn-allow" onClick={() => handleApprove('allow_session')}>Allow Session</button>
        <button className="btn-allow" onClick={() => handleApprove('allow_persistent')}>Always Allow</button>
      </div>
    </div>
  );
});
const SubagentCard = memo(function SubagentCard({ subagent, entry }: { subagent: SubagentEntry; entry?: ChatEntry | null }) {
  const isDone = subagent.status === 'done' || subagent.status === 'error';
  const [collapsed, setCollapsed] = useState(isDone);

  useEffect(() => {
    if (isDone) {
      setCollapsed(true);
    }
  }, [isDone]);

  const statusIcon = useMemo(() => {
    if (subagent.status === 'working') return <div className="black-hole-spinner" style={{ width: 12, height: 12 }} />;
    if (subagent.status === 'done') return <CheckIcon size={12} color="var(--success)" />;
    if (subagent.status === 'error') return <XIcon size={12} color="#f87171" />;
    return null;
  }, [subagent.status]);

  const toolCount = useMemo(() => {
    return subagent.blocks?.filter((b) => b.type === 'tool').length || 0;
  }, [subagent.blocks]);

  const statusText = useMemo(() => {
    if (subagent.status === 'working') {
      const elapsed = subagent.endTime
        ? formatTime(subagent.endTime - subagent.startTime)
        : formatTime(Date.now() - subagent.startTime);
      return `Working · ${toolCount} tools · ${elapsed}`;
    }
    const iterText = subagent.iterations_used ? `${subagent.iterations_used} iter` : '';
    const toolText = toolCount > 0 ? `${toolCount} tools` : '';
    const timeText =
      subagent.endTime && subagent.startTime ? formatTime(subagent.endTime - subagent.startTime) : '';
    const parts = [subagent.status === 'done' ? 'Done' : 'Failed'];
    if (iterText) parts.push(iterText);
    if (toolText) parts.push(toolText);
    if (timeText) parts.push(timeText);
    return parts.join(' · ');
  }, [subagent, toolCount]);

  const displayStr = subagent.role_name || subagent.id;
  const idText = typeof displayStr === 'string' ? displayStr : JSON.stringify(displayStr);

  const hasPendingApproval = useMemo(() => {
    return subagent.blocks?.some((b) => b.type === 'approval' && b.status === 'pending');
  }, [subagent.blocks]);

  return (
    <div
      className={`subagent-card ${subagent.status === 'working' ? 'subagent-working' : ''} ${hasPendingApproval ? 'subagent-needs-approval' : ''}`}
    >
      <div className="subagent-header" onClick={() => setCollapsed(!collapsed)}>
        <span className="subagent-icon">{statusIcon}</span>
        <span className="subagent-id">{idText}</span>
        {hasPendingApproval && <span className="subagent-badge-pending">Approval Required</span>}
        <span className="subagent-status">{statusText}</span>
        {collapsed ? <ChevronRightIcon size={12} /> : <ChevronDownIcon size={12} />}
      </div>
      {!collapsed && (
        <div className="subagent-body">
          <div className="subagent-task">
            {typeof subagent.task === 'string' ? subagent.task : JSON.stringify(subagent.task)}
          </div>
          {groupBlocksIntoItems((subagent.blocks as any) || []).map((item, idx) => {
             if (item.type === 'iteration') {
               return <TurnIterationUI key={item.data.id} iteration={item.data} entry={entry} />;
             } else {
               const text = typeof (item.data as any).text === 'string' ? (item.data as any).text : JSON.stringify((item.data as any).text);
               return <MarkdownContent key={`sub-assistant-${idx}`} content={text} className="assistant-msg" isStreaming={!!(item.data as any).isStreaming} />;
             }
          })}
        </div>
      )}
    </div>
  );
});

const THINKING_PHRASES = [
  'Analyzing context...',
  'Synthesizing logic...',
  'Exploring possibilities...',
  'Simulating outcomes...',
  'Consulting neural pathways...',
  'Formulating strategy...',
];

const DynamicWorkingIndicator = memo(function DynamicWorkingIndicator({ entry }: { entry: ChatEntry }) {
  const [phraseIndex, setPhraseIndex] = useState(0);

  const isActive = !!(entry.startTime && !entry.endTime);
  useEffect(() => {
    if (!isActive) return;
    const interval = setInterval(() => {
      setPhraseIndex((prev) => (prev + 1) % THINKING_PHRASES.length);
    }, 2500);
    return () => clearInterval(interval);
  }, [isActive]);

  if (!isActive) return null;

  const statusText = useMemo(() => {
    if (!entry.blocks || entry.blocks.length === 0) {
      return 'Waking up the agent...';
    }
    const lastBlock = entry.blocks[entry.blocks.length - 1];

    if (lastBlock.type === 'thinking' && lastBlock.isStreaming) {
      return THINKING_PHRASES[phraseIndex];
    } else if (lastBlock.type === 'tool' && lastBlock.active) {
      if (lastBlock.name === 'invoke_subagent') return 'Waiting for subagents...';
      return `Interfacing with ${lastBlock.name}...`;
    } else if (lastBlock.type === 'approval' && lastBlock.status === 'pending') {
      return 'Awaiting human authorization...';
    } else if (lastBlock.type === 'assistant' && lastBlock.isStreaming) {
      return 'Transmitting response...';
    }

    return 'Working...';
  }, [entry.blocks, phraseIndex]);

  return (
    <div className="working-indicator">
      <div className="black-hole-spinner" />
      <span className="light-wave-text">{statusText}</span>
    </div>
  );
});

const TurnFooter = memo(function TurnFooter({ entry }: { entry: ChatEntry }) {
  const [copied, setCopied] = useState(false);

  const rawOutput = useMemo(() => {
    if (!entry.blocks) return '';
    return entry.blocks
      .filter((b): b is Extract<TurnBlock, { type: 'assistant' }> => b.type === 'assistant')
      .map((b) => b.text)
      .join('\n');
  }, [entry.blocks]);

  const endTimeText = useMemo(() => {
    if (!entry.endTime) return null;
    const d = new Date(entry.endTime);
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }, [entry.endTime]);

  const handleCopy = useCallback(async () => {
    if (!rawOutput) return;
    try {
      await navigator.clipboard.writeText(rawOutput);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // ignore
    }
  }, [rawOutput]);

  if (!endTimeText && !rawOutput) return null;

  return (
    <div className="turn-footer">
      {rawOutput && (
        <button className="turn-copy-btn" onClick={handleCopy} title="Copy Raw Assistant Output">
          {copied ? <CheckIcon size={11} color="var(--success)" /> : <CopyIcon size={11} />}
        </button>
      )}
      {endTimeText && <span className="turn-end-time">{endTimeText}</span>}
    </div>
  );
});

export const AgentTurnUI = memo(function AgentTurnUI({ entry }: { entry: ChatEntry }) {
  const isProcessing = !!(entry.startTime && !entry.endTime);
  const isDone = !!(entry.endTime);

  // Auto-collapse intermediate steps when turn is done
  const [collapsed, setCollapsed] = useState(false);
  useEffect(() => {
    if (isDone) setCollapsed(true);
  }, [isDone]);

  const { toolCount, thoughtCount } = useMemo(() => {
    let tools = 0, thoughts = 0, errors = 0;
    entry.blocks?.forEach((b: TurnBlock) => {
      if (b.type === 'tool') {
        const name = typeof b.name === 'string' ? b.name : '';
        if (name !== 'invoke_subagent' && name !== 'subagent' && name !== 'subagents') tools++;
      }
      if (b.type === 'thinking') thoughts++;
      if (b.type === 'error') errors++;
    });
    return { toolCount: tools, thoughtCount: thoughts, errorCount: errors };
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
  const firstIterIdx = useMemo(() => renderItems.findIndex(i => i.type === 'iteration'), [renderItems]);
  const lastIterIdx = useMemo(() => renderItems.findLastIndex(i => i.type === 'iteration'), [renderItems]);

  return (
    <div className="agent-turn">
      {renderItems.map((item, idx) => {
        const isFirstIter = idx === firstIterIdx;
        const showHeaderHere = isFirstIter || (firstIterIdx === -1 && idx === renderItems.length - 1);

        return (
          <React.Fragment key={item.type === 'iteration' ? item.data.id : `assistant-${idx}`}>
            {showHeaderHere && (hasIntermediateSteps || isProcessing) && (
              <>
                <div
                  className={`turn-header ${isProcessing ? 'processing-pulse' : ''}`}
                  style={{ cursor: !isProcessing ? 'pointer' : 'default' }}
                  onClick={() => {
                    if (!isProcessing) setCollapsed(!collapsed);
                  }}
                >
                  {isProcessing ? (
                    <>
                      <LoaderIcon className="tool-loader-icon" size={12} />
                      <ProcessingTimer startTime={entry.startTime} endTime={entry.endTime} />
                      <ChevronDownIcon size={12} style={ml4} />
                    </>
                  ) : (
                    <>
                      <span>Worked {summaryParts.join(' · ')}</span>
                      {collapsed ? <ChevronRightIcon size={12} style={ml4} /> : <ChevronDownIcon size={12} style={ml4} />}
                    </>
                  )}
                </div>
                <div className="turn-divider" />
              </>
            )}

            {item.type === 'assistant' ? (
              (!collapsed || lastIterIdx === -1 || idx > lastIterIdx) && (
                <MarkdownContent
                  content={typeof (item.data as any).text === 'string' ? (item.data as any).text : JSON.stringify((item.data as any).text)}
                  className="assistant-msg"
                  isStreaming={!!(item.data as any).isStreaming}
                />
              )
            ) : (
              !collapsed && <TurnIterationUI iteration={item.data} entry={entry} />
            )}
          </React.Fragment>
        );
      })}

      {firstIterIdx === -1 && renderItems.length === 0 && (hasIntermediateSteps || isProcessing) && (
        <>
          <div
            className={`turn-header ${isProcessing ? 'processing-pulse' : ''}`}
            style={{ cursor: !isProcessing ? 'pointer' : 'default' }}
            onClick={() => {
              if (!isProcessing) setCollapsed(!collapsed);
            }}
          >
            {isProcessing ? (
              <>
                <LoaderIcon className="tool-loader-icon" size={12} />
                <ProcessingTimer startTime={entry.startTime} endTime={entry.endTime} />
                <ChevronDownIcon size={12} style={ml4} />
              </>
            ) : (
              <>
                <span>Worked {summaryParts.join(' · ')}</span>
                {collapsed ? <ChevronRightIcon size={12} style={ml4} /> : <ChevronDownIcon size={12} style={ml4} />}
              </>
            )}
          </div>
          <div className="turn-divider" />
        </>
      )}



      {isProcessing ? <DynamicWorkingIndicator entry={entry} /> : <TurnFooter entry={entry} />}
    </div>
  );
});
