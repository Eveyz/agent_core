import { useState, useEffect, memo, useMemo } from 'react';
import { useDispatch } from 'react-redux';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
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
    if (endTime) return;
    const interval = setInterval(() => setNow(Date.now()), 100);
    return () => clearInterval(interval);
  }, [endTime]);

  if (!startTime) return null;
  const diff = (endTime || now) - startTime;
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
  result,
  active,
}: {
  name: string;
  result?: string;
  active?: boolean;
}) {
  const [collapsed, setCollapsed] = useState(!active);

  useEffect(() => {
    if (active === false) setCollapsed(true);
    else if (active === true) setCollapsed(false);
  }, [active]);

  return (
    <div className="block-wrapper">
      <div
        className={`thinking-toggle ${!collapsed ? 'expanded' : ''}`}
        style={{ cursor: 'pointer' }}
        onClick={() => setCollapsed(!collapsed)}
      >
        Used tool: {name} {collapsed ? <ChevronRightIcon size={12} style={ml4} /> : <ChevronDownIcon size={12} style={ml4} />}
      </div>
      {!collapsed && result ? (
        <div
          className="tool-result-block assistant-msg"
          dangerouslySetInnerHTML={parseMarkdown(result)}
        />
      ) : null}
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
        <pre>{block.tool_input}</pre>
      </div>
      {block.status === 'pending' ? (
        <div className="approval-actions">
          <button className="btn-deny" onClick={() => handleApprove('deny')}>Deny</button>
          <button className="btn-allow" onClick={() => handleApprove('allow')}>Allow Once</button>
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

  useEffect(() => {
    const interval = setInterval(() => {
      setPhraseIndex(prev => (prev + 1) % THINKING_PHRASES.length);
    }, 2500);
    return () => clearInterval(interval);
  }, []);

  if (entry.endTime) return null;

  const statusText = useMemo(() => {
    if (!entry.blocks || entry.blocks.length === 0) {
      return "Waking up the agent...";
    }
    const lastBlock = entry.blocks[entry.blocks.length - 1];

    if (lastBlock.type === 'thinking' && lastBlock.isStreaming) {
      return THINKING_PHRASES[phraseIndex];
    } else if (lastBlock.type === 'tool' && lastBlock.active) {
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
      <span className="working-spinner">⚙️</span> {statusText}
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

  const totalTime = entry.endTime ? `Processed ${formatTime(entry.endTime - entry.startTime)}` : null;

  return (
    <div className="agent-turn">
      <div
        className={`turn-header ${!entry.endTime ? 'processing-pulse' : ''}`}
        style={{ cursor: entry.endTime ? 'pointer' : 'default' }}
        onClick={() => { if (entry.endTime) setCollapsed(!collapsed); }}
      >
        {!entry.endTime ? (
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
          return <ThinkingBlockUI key={idx} text={b.text} isStreaming={b.isStreaming} startTime={b.startTime} endTime={b.endTime} />;
        } else if (b.type === 'tool') {
          return <ToolBlockUI key={idx} name={b.name} result={b.result} active={b.active} />;
        } else if (b.type === 'approval') {
          return <ApprovalBlockUI key={idx} block={b} />;
        } else if (b.type === 'assistant') {
          return (
            <div key={idx} className="assistant-msg" dangerouslySetInnerHTML={parseMarkdown(b.text)} />
          );
        } else if (b.type === 'error') {
          return <div key={idx} className="error-msg">{b.text}</div>;
        }
        return null;
      })}

      {!entry.endTime ? <DynamicWorkingIndicator entry={entry} /> : null}
    </div>
  );
});
