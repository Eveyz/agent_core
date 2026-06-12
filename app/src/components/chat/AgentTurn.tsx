import { useState, useEffect, memo, useMemo } from 'react';
import { useDispatch } from 'react-redux';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import ZapIcon from 'lucide-react/dist/esm/icons/zap.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
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
        <pre>{typeof block.tool_input === 'string' ? block.tool_input : JSON.stringify(block.tool_input, null, 2)}</pre>
      </div>
      {block.status === 'pending' ? (
        <div className="approval-actions">
          <button className="btn-deny" onClick={() => handleApprove('deny')}>Deny</button>
          <button className="btn-allow" onClick={() => handleApprove('allow_session')}>Allow Once</button>
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
  return null;
});

const SubagentCard = memo(function SubagentCard({ subagent }: { subagent: any }) {
  const [collapsed, setCollapsed] = useState(false);

  const statusIcon = useMemo(() => {
    if (subagent.status === 'working') return <ZapIcon size={12} color="#facc15" />;
    if (subagent.status === 'done') return <CheckIcon size={12} color="#4ade80" />;
    return <XIcon size={12} color="#f87171" />;
  }, [subagent.status]);

  const statusText = useMemo(() => {
    if (subagent.status === 'working') {
      const elapsed = subagent.endTime
        ? formatTime(subagent.endTime - subagent.startTime)
        : formatTime(Date.now() - subagent.startTime);
      return `Working · ${elapsed}`;
    }
    const iterText = subagent.iterations_used ? `${subagent.iterations_used} iter` : '';
    const timeText = subagent.endTime && subagent.startTime
      ? formatTime(subagent.endTime - subagent.startTime)
      : '';
    return `${subagent.status === 'done' ? 'Done' : 'Failed'}${iterText ? ` · ${iterText}` : ''}${timeText ? ` · ${timeText}` : ''}`;
  }, [subagent]);

  const idText = typeof subagent.id === 'string' ? subagent.id : JSON.stringify(subagent.id);

  return (
    <div className={`subagent-card ${subagent.status === 'working' ? 'subagent-working' : ''}`}>
      <div className="subagent-header" onClick={() => setCollapsed(!collapsed)}>
        <span className="subagent-icon">{statusIcon}</span>
        <span className="subagent-id">{idText}</span>
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

  const totalTime = entry.endTime ? `Processed ${formatTime(entry.endTime - entry.startTime)}` : null;

  const subagentList = useMemo(() => {
    if (!entry.subagents) return [];
    return Object.values(entry.subagents);
  }, [entry.subagents]);

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
          const text = typeof b.text === 'string' ? b.text : JSON.stringify(b.text);
          return <ThinkingBlockUI key={idx} text={text} isStreaming={b.isStreaming} startTime={b.startTime} endTime={b.endTime} />;
        } else if (b.type === 'tool') {
          const name = typeof b.name === 'string' ? b.name : JSON.stringify(b.name);
          const result = typeof b.result === 'string' ? b.result : JSON.stringify(b.result);
          return <ToolBlockUI key={idx} name={name} result={result} active={b.active} />;
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
        }
        return null;
      })}

      {subagentList.length > 0 && (
        <div className="subagents-section">
          {subagentList.map((sa: any) => (
            <SubagentCard key={sa.id} subagent={sa} />
          ))}
        </div>
      )}

      {!entry.endTime ? <DynamicWorkingIndicator entry={entry} /> : null}
    </div>
  );
});
