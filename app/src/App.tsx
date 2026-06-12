import { useState, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useDispatch, useSelector } from 'react-redux';
import { 
  Bot, Send, Plus, TerminalSquare, Search, Box, LayoutGrid, 
  MessageSquare, ChevronDown, Clock, Folder, Settings, Smartphone, Maximize2,
  ChevronRight, Copy, Edit2
} from 'lucide-react';
import { marked } from 'marked';
import DOMPurify from 'dompurify';
import { RootState } from './store';
import { agentEventReceived, userMessageSent, toolApprovalResponded } from './features/chat/chatSlice';
import './App.css';

marked.setOptions({
  breaks: true,
  gfm: true
});

const parseMarkdown = (raw: string) => {
  const html = marked.parse(raw) as string;
  return { __html: DOMPurify.sanitize(html) };
};

const formatTime = (ms: number) => {
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  const m = Math.floor(ms / 60000);
  const s = Math.floor((ms % 60000) / 1000);
  return `${m}m ${s}s`;
};

const ProcessingTimer = ({ startTime, endTime }: { startTime?: number, endTime?: number }) => {
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    if (endTime) return;
    const interval = setInterval(() => setNow(Date.now()), 100);
    return () => clearInterval(interval);
  }, [endTime]);

  if (!startTime) return null;
  const diff = (endTime || now) - startTime;
  return <span>Processed {formatTime(diff)}</span>;
};

const ThinkingBlockUI = ({ text, isStreaming, startTime, endTime }: { text: string; isStreaming: boolean; startTime?: number; endTime?: number }) => {
  const [collapsed, setCollapsed] = useState(false);
  
  const getDurationString = () => {
    if (isStreaming) return 'Thinking...';
    if (startTime && endTime) {
      const diff = endTime - startTime;
      return `Thought for ${formatTime(diff)}`;
    }
    return 'Thought';
  };

  return (
    <div className="block-wrapper">
      <div 
        className={`thinking-toggle ${isStreaming ? 'thinking-pulse' : ''}`} 
        onClick={() => setCollapsed(!collapsed)}
        style={{ cursor: 'pointer' }}
      >
        {getDurationString()} {collapsed ? <ChevronRight size={12} style={{ marginLeft: '4px' }} /> : <ChevronDown size={12} style={{ marginLeft: '4px' }} />}
      </div>
      {!collapsed && (
        <div className="thinking-block">
          {text}
          {isStreaming && <span className="typing-dot" style={{ display: 'inline-block', marginLeft: '4px' }}>...</span>}
        </div>
      )}
    </div>
  );
};

const ToolBlockUI = ({ name, result }: { name: string; result?: string }) => {
  const [collapsed, setCollapsed] = useState(true);
  
  return (
    <div className="block-wrapper">
      <div 
        className="thinking-toggle" 
        style={{ cursor: 'pointer' }} 
        onClick={() => setCollapsed(!collapsed)}
      >
        Used tool: {name} {collapsed ? <ChevronRight size={12} style={{ marginLeft: '4px' }} /> : <ChevronDown size={12} style={{ marginLeft: '4px' }} />}
      </div>
      {!collapsed && result && (
        <div 
          className="tool-result-block assistant-msg" 
          dangerouslySetInnerHTML={parseMarkdown(result)} 
        />
      )}
    </div>
  );
};

const ApprovalBlockUI = ({ block }: { block: any }) => {
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
        <span className={`danger-badge danger-${block.danger_level.toLowerCase()}`}>{block.danger_level}</span>
      </div>
      <div className="approval-explanation">{block.explanation}</div>
      <div className="approval-args">
        <pre>{JSON.stringify(block.tool_input, null, 2)}</pre>
      </div>
      {block.status === 'pending' ? (
        <div className="approval-actions">
          <button className="btn-deny" onClick={() => handleApprove('deny')}>Deny</button>
          <button className="btn-allow" onClick={() => handleApprove('allow_session')}>Allow</button>
        </div>
      ) : (
        <div className="approval-status">
          Status: <span className={`status-${block.status}`}>{block.status.toUpperCase()}</span>
        </div>
      )}
    </div>
  );
};

const AgentTurnUI = ({ entry }: { entry: any }) => {
  const [collapsed, setCollapsed] = useState(false);
  
  const getThoughtDuration = () => {
    let totalMs = 0;
    entry.blocks?.forEach((b: any) => {
      if (b.type === 'thinking' && b.startTime && b.endTime) {
        totalMs += (b.endTime - b.startTime);
      }
    });
    if (totalMs === 0) return null;
    return `Thought for ${formatTime(totalMs)}`;
  };

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
            <ChevronDown size={12} style={{ marginLeft: '4px' }}/>
          </>
        ) : (
          <>
            {collapsed ? (getThoughtDuration() ? `${totalTime} · ${getThoughtDuration()}` : totalTime) : totalTime}
            {collapsed ? <ChevronRight size={12} style={{ marginLeft: '4px' }}/> : <ChevronDown size={12} style={{ marginLeft: '4px' }}/>}
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
          return <ToolBlockUI key={idx} name={b.name} result={b.result} />;
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
      
      {!entry.endTime && (
        <div className="working-indicator">
          <span className="working-spinner">⚙️</span> Working...
        </div>
      )}
    </div>
  );
};

function App() {
  const dispatch = useDispatch();
  const { entries, isProcessing } = useSelector((state: RootState) => state.chat);
  const [input, setInput] = useState('');
  const [activeTab, setActiveTab] = useState<'code' | 'write'>('code');
  const messagesEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [entries, isProcessing]);

  useEffect(() => {
    let isMounted = true;
    let unlistenFn: (() => void) | undefined;
    const setupListener = async () => {
      const fn = await listen<any>('agent-event', (event) => {
        dispatch(agentEventReceived(event.payload));
      });
      if (!isMounted) {
        fn(); // Component unmounted before listener attached
      } else {
        unlistenFn = fn;
      }
    };
    setupListener();
    return () => {
      isMounted = false;
      if (unlistenFn) unlistenFn();
    };
  }, [dispatch]);

  const handleSend = async () => {
    if (!input.trim() || isProcessing) return;
    const userMsg = input;
    setInput('');
    dispatch(userMessageSent(userMsg));

    try {
      await invoke('send_message', { message: userMsg });
    } catch (e) {
      console.error('Invoke error:', e);
      dispatch(agentEventReceived({ Error: String(e) }));
    }
  };

  return (
    <div className="app-container">
      {/* Sidebar */}
      <aside className="sidebar">
        <div className="sidebar-header-actions">
          <button className="icon-btn"><LayoutGrid size={16} /></button>
        </div>

        <div className="toggle-group">
          <button 
            className={`toggle-btn ${activeTab === 'code' ? 'active' : ''}`}
            onClick={() => setActiveTab('code')}
          >
            <Bot size={14} /> Code
          </button>
          <button 
            className={`toggle-btn ${activeTab === 'write' ? 'active' : ''}`}
            onClick={() => setActiveTab('write')}
          >
            <MessageSquare size={14} /> Write
          </button>
        </div>

        <div className="sidebar-nav">
          <div className="nav-item"><Plus size={14} /> New Agent</div>
          <div className="nav-item"><MessageSquare size={14} /> New requirement</div>
          <div className="nav-item"><Box size={14} /> Plugins</div>
          <div className="nav-item"><Clock size={14} /> Scheduled tasks</div>
        </div>

        <div className="projects-header">
          <span>Projects</span>
          <div style={{ display: 'flex', gap: '8px' }}>
            <Search size={12} />
            <Box size={12} />
            <Folder size={12} />
          </div>
        </div>

        <div className="sidebar-nav" style={{ marginTop: '8px' }}>
          <div className="project-item">
            <span style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
              <Folder size={14} color="#808080" /> agent_core
            </span>
            <span className="meta">rust-projects</span>
          </div>
          <div className="project-item">
            <span style={{ display: 'flex', alignItems: 'center', gap: '8px', paddingLeft: '22px', fontSize: '12px' }}>
              check the weather for Shenz...
            </span>
            <span className="meta" style={{ color: '#E2E2E2' }}>now</span>
          </div>
          <div className="project-item">
            <span style={{ display: 'flex', alignItems: 'center', gap: '8px', paddingLeft: '22px', fontSize: '12px' }}>
              what's the result for t...
            </span>
            <span className="meta">13 hours ago</span>
          </div>
          <div className="project-item">
            <span style={{ display: 'flex', alignItems: 'center', gap: '8px', paddingLeft: '22px', fontSize: '12px' }}>
              in the top status bar, c...
            </span>
            <span className="meta">3 days ago</span>
          </div>
        </div>

        <div className="sidebar-bottom">
          <div className="nav-item"><Smartphone size={14} /> Connect phone</div>
          <div className="nav-item"><Settings size={14} /> Settings</div>
        </div>
      </aside>
      
      {/* Main Area */}
      <main className="main-area">
        <header className="main-header">
          <div className="header-title">check the weather for Shenzhen<br/><span style={{ fontSize: '11px', color: '#555' }}>agent_core · Agent · now · 10.3k tok / $0.0003 / cache 97%</span></div>
          <div className="header-actions">
            <button className="icon-btn"><Box size={14} /></button>
            <button className="icon-btn"><MessageSquare size={14} /></button>
            <button className="icon-btn"><TerminalSquare size={14} /></button>
            <button className="icon-btn"><Folder size={14} /></button>
            <button className="icon-btn"><Maximize2 size={14} /></button>
          </div>
        </header>

        {entries.length === 0 ? (
          <div className="empty-state">
            <div className="hero-logo">
              <Bot className="hero-icon" />
            </div>
          </div>
        ) : (
          <div className="chat-history">
            {entries.map((entry) => {
              if (entry.type === 'user') {
                return (
                  <div key={entry.id} className="message-row user-row">
                    <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end', gap: '6px' }}>
                      <div className="user-msg">
                        {entry.text}
                      </div>
                      <div style={{ display: 'flex', gap: '12px', color: '#555', fontSize: '11px', paddingRight: '4px' }}>
                        <span>deepseek-v4-pro</span>
                        <Copy size={12} style={{ cursor: 'pointer' }}/>
                        <Edit2 size={12} style={{ cursor: 'pointer' }}/>
                      </div>
                    </div>
                  </div>
                );
              } else if (entry.type === 'turn') {
                return (
                  <div key={entry.id} className="message-row agent-row">
                    <AgentTurnUI entry={entry} />
                  </div>
                );
              }
            })}
            
            <div ref={messagesEndRef} />
          </div>
        )}
        
        <div className="input-area">
          <div className="input-container">
            <textarea 
              className="chat-input"
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault();
                  handleSend();
                }
              }}
              placeholder="Ask the agent..."
              rows={1}
            />
            <div className="input-actions">
              <div className="input-actions-left">
                <button className="icon-btn"><Plus size={16} /></button>
              </div>
              <div className="input-actions-right">
                <div className="model-selector">
                  deepseek-v4-pro <span style={{ color: '#555' }}>Ultra</span> <ChevronDown size={12} />
                </div>
                <button 
                  className="send-btn" 
                  onClick={handleSend}
                  disabled={!input.trim() || isProcessing}
                >
                  <Send size={14} />
                </button>
              </div>
            </div>
          </div>
          
          <div className="input-footer">
            <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
              <span style={{ display: 'flex', alignItems: 'center', gap: '4px' }}><TerminalSquare size={10} /> tauri <ChevronDown size={10} /></span>
              <span>10.3k tokens</span>
              <span>$0.0003</span>
              <span>cache 97%</span>
              <span>{Math.floor(entries.length / 2)} turns</span>
            </div>
            <div>Type / for commands</div>
          </div>
        </div>
      </main>
    </div>
  );
}

export default App;
