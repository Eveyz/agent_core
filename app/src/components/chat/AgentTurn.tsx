import { useState, useEffect, memo, useMemo, useCallback } from 'react';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import ZapIcon from 'lucide-react/dist/esm/icons/zap.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import LoaderIcon from 'lucide-react/dist/esm/icons/loader.mjs';
import CheckCircle2Icon from 'lucide-react/dist/esm/icons/check-circle-2.mjs';
import XCircleIcon from 'lucide-react/dist/esm/icons/x-circle.mjs';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import { invoke } from '@tauri-apps/api/core';
import { toolApprovalResponded } from '../../features/chat/chatSlice';
import type { ChatEntry, TurnBlock, SubagentEntry, SubagentBlock } from '../../features/chat/chatSlice';
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

const ThinkingBlockUI = memo(function ThinkingBlockUI({
  text,
  isStreaming,
  startTime,
  endTime,
}: {
  text: string;
  isStreaming: boolean;
  startTime?: number;
  endTime?: number;
}) {
  const [collapsed, setCollapsed] = useState(false);

  const durationText = useMemo(() => {
    if (isStreaming) return 'Thinking...';
    if (startTime && endTime) {
      return `Thought for ${formatTime(endTime - startTime)}`;
    }
    return 'Thought';
  }, [isStreaming, startTime, endTime]);

  return (
    <div className="block-wrapper">
      <div
        className={`thinking-toggle ${isStreaming ? 'thinking-pulse' : ''} ${!collapsed ? 'expanded' : ''}`}
        onClick={() => setCollapsed(!collapsed)}
        style={{ cursor: 'pointer' }}
      >
        {durationText} {collapsed ? <ChevronRightIcon size={12} style={ml4} /> : <ChevronDownIcon size={12} style={ml4} />}
      </div>
      {!collapsed ? (
        <div className="thinking-block">
          {text}
          {isStreaming ? <span className="typing-dot" style={{ display: 'inline-block', marginLeft: '4px' }}>...</span> : null}
        </div>
      ) : null}
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

  useEffect(() => {
    if (active === false) setCollapsed(true);
    else if (active === true) setCollapsed(false);
  }, [active]);

  const statusIcon = useMemo(() => {
    if (active) return <LoaderIcon className="tool-loader-icon" size={14} />;
    if (is_error) return <XCircleIcon color="#f87171" size={14} />;
    return <CheckCircle2Icon color="#4ade80" size={14} />;
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

  return (
    <div className={`tool-block-wrapper ${active ? 'active' : ''} ${is_error ? 'error' : ''}`}>
      <div
        className={`tool-block-header ${!collapsed ? 'expanded' : ''}`}
        onClick={() => setCollapsed(!collapsed)}
      >
        <span className="tool-status-icon">{statusIcon}</span>
        <span className="tool-name">{name}</span>
        {collapsed ? <ChevronRightIcon size={14} className="ml-auto" /> : <ChevronDownIcon size={14} className="ml-auto" />}
      </div>
      {!collapsed && (
        <div className="tool-block-body">
          {formattedArgs && (
            <div className="tool-section">
              <div className="tool-section-label">INPUT</div>
              <pre className="tool-args-pre">{formattedArgs}</pre>
            </div>
          )}
          {result && !active && (
            <div className="tool-section">
              <div className="tool-section-label">OUTPUT</div>
              <MarkdownContent content={result} className="tool-result-content assistant-msg" />
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

const SubagentBlockUI = memo(function SubagentBlockUI({ block }: { block: SubagentBlock }) {
  if (block.type === 'thinking') {
    const text = typeof block.text === 'string' ? block.text : JSON.stringify(block.text);
    return (
      <ThinkingBlockUI
        text={text}
        isStreaming={!!block.isStreaming}
        startTime={block.startTime}
        endTime={block.endTime}
      />
    );
  }
  if (block.type === 'assistant') {
    const text = typeof block.text === 'string' ? block.text : JSON.stringify(block.text);
    return (
      <MarkdownContent
        content={text}
        className="assistant-msg"
        isStreaming={!!block.isStreaming}
      />
    );
  }
  if (block.type === 'tool') {
    const name = typeof block.name === 'string' ? block.name : JSON.stringify(block.name);
    const result = typeof block.result === 'string' ? block.result : JSON.stringify(block.result);
    return <ToolBlockUI name={name || ''} result={result} active={!!block.active} />;
  }
  if (block.type === 'error') {
    const text = typeof block.text === 'string' ? block.text : JSON.stringify(block.text);
    return <div className="error-msg">{text}</div>;
  }
  if (block.type === 'approval') {
    return <ApprovalBlockUI block={block} />;
  }
  return null;
});

const SubagentCard = memo(function SubagentCard({ subagent }: { subagent: SubagentEntry }) {
  const isDone = subagent.status === 'done' || subagent.status === 'error';
  const [collapsed, setCollapsed] = useState(isDone);

  useEffect(() => {
    if (isDone) {
      setCollapsed(true);
    }
  }, [isDone]);

  const statusIcon = useMemo(() => {
    if (subagent.status === 'working') return <ZapIcon size={12} color="#facc15" />;
    if (subagent.status === 'done') return <CheckIcon size={12} color="#4ade80" />;
    return <XIcon size={12} color="#f87171" />;
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
          {subagent.blocks?.map((b, idx) => (
            <SubagentBlockUI key={`${b.type}-${idx}`} block={b} />
          ))}
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

    return 'Processing data...';
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
        <button className="turn-copy-btn" onClick={handleCopy} title="Copy raw output">
          {copied ? <CheckIcon size={11} color="#4ade80" /> : <CopyIcon size={11} />}
        </button>
      )}
      {endTimeText && <span className="turn-end-time">{endTimeText}</span>}
    </div>
  );
});

export const AgentTurnUI = memo(function AgentTurnUI({ entry }: { entry: ChatEntry }) {
  const [collapsed, setCollapsed] = useState(false);

  const thoughtDuration = useMemo(() => {
    let totalMs = 0;
    entry.blocks?.forEach((b: TurnBlock) => {
      if (b.type === 'thinking' && b.startTime && b.endTime) {
        totalMs += b.endTime - b.startTime;
      }
    });
    if (totalMs === 0) return null;
    return `Thought for ${formatTime(totalMs)}`;
  }, [entry.blocks]);

  const isProcessing = !!(entry.startTime && !entry.endTime);

  const totalTime =
    entry.startTime && entry.endTime ? `Processed ${formatTime(entry.endTime - entry.startTime)}` : null;

  return (
    <div className="agent-turn">
      <div
        className={`turn-header ${isProcessing ? 'processing-pulse' : ''}`}
        style={{ cursor: !isProcessing ? 'pointer' : 'default' }}
        onClick={() => {
          if (!isProcessing) setCollapsed(!collapsed);
        }}
      >
        {isProcessing ? (
          <>
            <ProcessingTimer startTime={entry.startTime} endTime={entry.endTime} />
            <ChevronDownIcon size={12} style={ml4} />
          </>
        ) : (
          <>
            {collapsed ? (thoughtDuration ? `${totalTime} · ${thoughtDuration}` : totalTime) : totalTime}
            {collapsed ? <ChevronRightIcon size={12} style={ml4} /> : <ChevronDownIcon size={12} style={ml4} />}
          </>
        )}
      </div>

      {entry.blocks?.map((b: TurnBlock, idx: number) => {
        if (collapsed && b.type !== 'assistant' && b.type !== 'error') {
          return null;
        }

        if (b.type === 'thinking') {
          const text = typeof b.text === 'string' ? b.text : JSON.stringify(b.text);
          return (
            <ThinkingBlockUI
              key={`thinking-${idx}`}
              text={text}
              isStreaming={b.isStreaming}
              startTime={b.startTime}
              endTime={b.endTime}
            />
          );
        } else if (b.type === 'tool') {
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
          return <ApprovalBlockUI key={`approval-${b.prompt_id}-${idx}`} block={b} />;
        } else if (b.type === 'assistant') {
          const text = typeof b.text === 'string' ? b.text : JSON.stringify(b.text);
          return (
            <MarkdownContent
              key={`assistant-${idx}`}
              content={text}
              className="assistant-msg"
              isStreaming={!!b.isStreaming}
            />
          );
        } else if (b.type === 'error') {
          const text = typeof b.text === 'string' ? b.text : JSON.stringify(b.text);
          return <div key={`error-${idx}`} className="error-msg">{text}</div>;
        } else if (b.type === 'subagent_ref') {
          const sa = entry.subagents?.[b.subagent_id];
          if (!sa) return null;
          return (
            <div key={`subagent-${b.subagent_id}-${idx}`} className="subagents-section">
              <SubagentCard subagent={sa} />
            </div>
          );
        }
        return null;
      })}

      {isProcessing ? <DynamicWorkingIndicator entry={entry} /> : <TurnFooter entry={entry} />}
    </div>
  );
});
