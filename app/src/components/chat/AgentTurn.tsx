import { useState, useEffect, memo, useMemo } from 'react';
import { useDispatch } from 'react-redux';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import ZapIcon from 'lucide-react/dist/esm/icons/zap.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import LoaderIcon from 'lucide-react/dist/esm/icons/loader.mjs';
import CheckCircle2Icon from 'lucide-react/dist/esm/icons/check-circle-2.mjs';
import XCircleIcon from 'lucide-react/dist/esm/icons/x-circle.mjs';
import { invoke } from '@tauri-apps/api/core';
import DOMPurify from 'dompurify';
import { marked } from 'marked';
import { toolApprovalResponded } from '../../features/chat/chatSlice';

export const parseMarkdown = (raw: string) => {
  const html = marked.parse(raw) as string;
  return { __html: DOMPurify.sanitize(html) };
};

export const formatTime = (ms: number) => {
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  const m = Math.floor(ms / 60000);
  const s = Math.floor((ms % 60000) / 1000);
  return `${m}m ${s}s`;
};

const ml4 = { marginLeft: '4px' };

const ProcessingTimer = memo(function ProcessingTimer({ startTime, endTime }: { startTime?: number; endTime?: number }) {
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    // Only start the interval if the turn is actively processing (has startTime, no endTime yet)
    if (!startTime || endTime) return;
    const interval = setInterval(() => setNow(Date.now()), 100);
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

  useEffect(() => {
    setCollapsed(!isStreaming);
  }, [isStreaming]);

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
        className={`thinking-toggle ${isStreaming ? 'thinking-pulse' : ''}`}
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
  args?: any;
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
              <div
                className="tool-result-content assistant-msg"
                dangerouslySetInnerHTML={parseMarkdown(result)}
              />
            </div>
          )}
        </div>
      )}
    </div>
  );
});

const ApprovalBlockUI = memo(function ApprovalBlockUI({ block }: { block: any }) {
  const dispatch = useDispatch();

  const handleApprove = async (choice: string) => {
    dispatch(toolApprovalResponded({ promptId: block.prompt_id, approved: choice !== 'deny' }));
    try {
      await invoke('approve_tool', { promptId: block.prompt_id, choice });
    } catch (e) {
      console.error('Failed to approve tool', e);
    }
  };

  return (
    <div className="approval-block">
      <div className="approval-header">
        <span className="approval-title">Approval Required: {block.tool_name}</span>
        {block.danger_level ? (
          <span className={`danger-badge danger-${block.danger_level}`}>
            {block.danger_level}
          </span>
        ) : null}
      </div>
      <div className="approval-explanation">{block.explanation}</div>
      <div className="approval-args">
        <pre>{typeof block.tool_input === 'string' ? block.tool_input : JSON.stringify(block.tool_input, null, 2)}</pre>
      </div>
      {block.status === 'pending' ? (
        <div className="approval-actions" style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
          <button className="btn-deny" onClick={() => handleApprove('deny')}>Deny Once</button>
          <button className="btn-deny" onClick={() => handleApprove('deny_persistent')}>Deny Always</button>
          <button className="btn-allow" onClick={() => handleApprove('allow_once')}>Allow Once</button>
          <button className="btn-allow" onClick={() => handleApprove('allow_session')}>Allow Session</button>
          <button className="btn-allow" onClick={() => handleApprove('allow_persistent')}>Always Allow</button>
        </div>
      ) : (
        <div className="approval-status">
          <span className={block.status === 'approved' ? 'status-approved' : 'status-denied'}>
            {block.status === 'approved' ? 'Approved' : 'Denied'}
          </span>
        </div>
      )}
    </div>
  );
});

const SubagentBlockUI = memo(function SubagentBlockUI({ block }: { block: any }) {
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
      <div className="assistant-msg" dangerouslySetInnerHTML={parseMarkdown(text)} />
    );
  }
  if (block.type === 'tool') {
    const name = typeof block.name === 'string' ? block.name : JSON.stringify(block.name);
    const result = typeof block.result === 'string' ? block.result : JSON.stringify(block.result);
    return (
      <ToolBlockUI
        name={name || ''}
        result={result}
        active={!!block.active}
      />
    );
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

const SubagentCard = memo(function SubagentCard({ subagent }: { subagent: any }) {
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
    return subagent.blocks?.filter((b: any) => b.type === 'tool').length || 0;
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
    const timeText = subagent.endTime && subagent.startTime
      ? formatTime(subagent.endTime - subagent.startTime)
      : '';
    const parts = [subagent.status === 'done' ? 'Done' : 'Failed'];
    if (iterText) parts.push(iterText);
    if (toolText) parts.push(toolText);
    if (timeText) parts.push(timeText);
    return parts.join(' · ');
  }, [subagent, toolCount]);

  const displayStr = subagent.role_name || subagent.id;
  const idText = typeof displayStr === 'string' ? displayStr : JSON.stringify(displayStr);

  const hasPendingApproval = useMemo(() => {
    return subagent.blocks?.some((b: any) => b.type === 'approval' && b.status === 'pending');
  }, [subagent.blocks]);

  return (
    <div className={`subagent-card ${subagent.status === 'working' ? 'subagent-working' : ''} ${hasPendingApproval ? 'subagent-needs-approval' : ''}`}>
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
          {subagent.blocks?.map((b: any, idx: number) => (
            <SubagentBlockUI key={idx} block={b} />
          ))}
        </div>
      )}
    </div>
  );
});

const THINKING_PHRASES = [
  "Analyzing context...",
  "Synthesizing logic...",
  "Exploring possibilities...",
  "Simulating outcomes...",
  "Consulting neural pathways...",
  "Formulating strategy..."
];

const DynamicWorkingIndicator = memo(function DynamicWorkingIndicator({ entry }: { entry: any }) {
  const [phraseIndex, setPhraseIndex] = useState(0);

  // Only run the phrase rotation when the turn is actually active
  const isActive = entry.startTime && !entry.endTime;
  useEffect(() => {
    if (!isActive) return;
    const interval = setInterval(() => {
      setPhraseIndex(prev => (prev + 1) % THINKING_PHRASES.length);
    }, 2500);
    return () => clearInterval(interval);
  }, [isActive]);

  if (!isActive) return null;

  const statusText = useMemo(() => {
    if (!entry.blocks || entry.blocks.length === 0) {
      return "Waking up the agent...";
    }
    const lastBlock = entry.blocks[entry.blocks.length - 1];

    if (lastBlock.type === 'thinking' && lastBlock.isStreaming) {
      return THINKING_PHRASES[phraseIndex];
    } else if (lastBlock.type === 'tool' && lastBlock.active) {
      if (lastBlock.name === 'invoke_subagent') return "Waiting for subagents...";
      return `Interfacing with ${lastBlock.name}...`;
    } else if (lastBlock.type === 'approval' && lastBlock.status === 'pending') {
      return "Awaiting human authorization...";
    } else if (lastBlock.type === 'assistant' && lastBlock.isStreaming) {
      return "Transmitting response...";
    }

    return "Processing data...";
  }, [entry.blocks, phraseIndex]);

  return (
    <div className="working-indicator">
      <div className="black-hole-spinner" />
      <span className="light-wave-text">{statusText}</span>
    </div>
  );
});

export const AgentTurnUI = memo(function AgentTurnUI({ entry }: { entry: any }) {
  const [collapsed, setCollapsed] = useState(false);

  const thoughtDuration = useMemo(() => {
    let totalMs = 0;
    entry.blocks?.forEach((b: any) => {
      if (b.type === 'thinking' && b.startTime && b.endTime) {
        totalMs += (b.endTime - b.startTime);
      }
    });
    if (totalMs === 0) return null;
    return `Thought for ${formatTime(totalMs)}`;
  }, [entry.blocks]);

  // A turn is "actively processing" only if it has a startTime but no endTime.
  // Restored turns have neither, so they should show as completed (no timer, no pulse).
  const isProcessing = !!(entry.startTime && !entry.endTime);

  const totalTime = entry.startTime && entry.endTime ? `Processed ${formatTime(entry.endTime - entry.startTime)}` : null;



  return (
    <div className="agent-turn">
      <div
        className={`turn-header ${isProcessing ? 'processing-pulse' : ''}`}
        style={{ cursor: !isProcessing ? 'pointer' : 'default' }}
        onClick={() => { if (!isProcessing) setCollapsed(!collapsed); }}
      >
        {isProcessing ? (
          <>
            <ProcessingTimer startTime={entry.startTime} endTime={entry.endTime} />
            <ChevronDownIcon size={12} style={ml4}/>
          </>
        ) : (
          <>
            {collapsed ? (thoughtDuration ? `${totalTime} · ${thoughtDuration}` : totalTime) : totalTime}
            {collapsed ? <ChevronRightIcon size={12} style={ml4}/> : <ChevronDownIcon size={12} style={ml4}/>}
          </>
        )}
      </div>

      {entry.blocks?.map((b: any, idx: number) => {
        if (collapsed && b.type !== 'assistant' && b.type !== 'error') {
          return null;
        }

        if (b.type === 'thinking') {
          const text = typeof b.text === 'string' ? b.text : JSON.stringify(b.text);
          return <ThinkingBlockUI key={idx} text={text} isStreaming={b.isStreaming} startTime={b.startTime} endTime={b.endTime} />;
        } else if (b.type === 'tool') {
          const name = typeof b.name === 'string' ? b.name : JSON.stringify(b.name);
          if (name === 'invoke_subagent' || name === 'subagent') {
            return null; // hide redundant tool block since SubagentCard handles it
          }
          const result = typeof b.result === 'string' ? b.result : JSON.stringify(b.result);
          return <ToolBlockUI key={idx} name={name} args={b.args} result={result} active={b.active} is_error={b.is_error} />;
        } else if (b.type === 'approval') {
          return <ApprovalBlockUI key={idx} block={b} />;
        } else if (b.type === 'assistant') {
          const text = typeof b.text === 'string' ? b.text : JSON.stringify(b.text);
          return (
            <div key={idx} className="assistant-msg" dangerouslySetInnerHTML={parseMarkdown(text)} />
          );
        } else if (b.type === 'error') {
          const text = typeof b.text === 'string' ? b.text : JSON.stringify(b.text);
          return <div key={idx} className="error-msg">{text}</div>;
        } else if (b.type === 'subagent_ref') {
          const sa = entry.subagents?.[b.subagent_id];
          if (!sa) return null;
          return <div key={idx} className="subagents-section"><SubagentCard subagent={sa} /></div>;
        }
        return null;
      })}

      {isProcessing ? <DynamicWorkingIndicator entry={entry} /> : null}
    </div>
  );
});
