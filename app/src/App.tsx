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
import { agentEventReceived, userMessageSent } from './features/chat/chatSlice';
import './App.css';

const parseMarkdown = (raw: string) => {
  const html = marked.parse(raw) as string;
  return { __html: DOMPurify.sanitize(html) };
};

const ThinkingBlockUI = ({ text, isStreaming }: { text: string; isStreaming: boolean }) => {
  const [collapsed, setCollapsed] = useState(false);
  
  return (
    <div className="block-wrapper">
      <div className="thinking-toggle" onClick={() => setCollapsed(!collapsed)}>
        Thinking {collapsed ? <ChevronRight size={12} /> : <ChevronDown size={12} />}
      </div>
      {!collapsed && (
        <div className="thinking-block">
          {text}
          {isStreaming && <span className="typing-dot" style={{ display: 'inline-block', marginLeft: '4px' }}></span>}
        </div>
      )}
    </div>
  );
};

const ToolBlockUI = ({ name, active }: { name: string; active: boolean }) => {
  return (
    <div className="block-wrapper">
      <div className="tool-summary">
        Used 1 tool
      </div>
      <div className="thinking-toggle" style={{ cursor: 'default' }}>
        {name} {active ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
      </div>
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

  const getProcessingTime = (start?: number, end?: number) => {
    if (!start) return '';
    const diff = (end || Date.now()) - start;
    return `Processed ${(diff / 1000).toFixed(1)}s`;
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
                    <div className="agent-turn">
                      <div className="turn-header">
                        {getProcessingTime(entry.startTime, entry.endTime)}
                        <ChevronDown size={12} style={{ marginLeft: '4px', cursor: 'pointer' }}/>
                      </div>
                      
                      {entry.blocks?.map((b, idx) => {
                        if (b.type === 'thinking') {
                          return <ThinkingBlockUI key={idx} text={b.text} isStreaming={b.isStreaming} />;
                        } else if (b.type === 'tool') {
                          return <ToolBlockUI key={idx} name={b.name} active={b.active} />;
                        } else if (b.type === 'assistant') {
                          return (
                            <div key={idx} className="assistant-msg" dangerouslySetInnerHTML={parseMarkdown(b.text)} />
                          );
                        } else if (b.type === 'error') {
                          return <div key={idx} className="error-msg">{b.text}</div>;
                        }
                        return null;
                      })}
                      
                    </div>
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
