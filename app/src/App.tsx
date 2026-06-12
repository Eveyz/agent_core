import { useState, useEffect, useRef, memo, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useDispatch, useSelector } from 'react-redux';
import BotIcon from 'lucide-react/dist/esm/icons/bot.mjs';
import SendIcon from 'lucide-react/dist/esm/icons/send.mjs';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import TerminalSquareIcon from 'lucide-react/dist/esm/icons/terminal-square.mjs';
import SearchIcon from 'lucide-react/dist/esm/icons/search.mjs';
import BoxIcon from 'lucide-react/dist/esm/icons/box.mjs';
import LayoutGridIcon from 'lucide-react/dist/esm/icons/layout-grid.mjs';
import MessageSquareIcon from 'lucide-react/dist/esm/icons/message-square.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ClockIcon from 'lucide-react/dist/esm/icons/clock.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import SettingsIcon from 'lucide-react/dist/esm/icons/settings.mjs';
import SmartphoneIcon from 'lucide-react/dist/esm/icons/smartphone.mjs';
import Maximize2Icon from 'lucide-react/dist/esm/icons/maximize-2.mjs';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import Edit2Icon from 'lucide-react/dist/esm/icons/edit-2.mjs';
import { RootState } from './store';
import { agentEventReceived, userMessageSent, ChatEntry } from './features/chat/chatSlice';
import { AgentTurnUI } from './components/chat/AgentTurn';
import './App.css';

const flexGap8 = { display: 'flex', gap: '8px' } as const;
const flexColumnEnd = { display: 'flex', flexDirection: 'column' as const, alignItems: 'flex-end', gap: '6px' };
const flexRowMeta = { display: 'flex', gap: '12px', color: '#555', fontSize: '11px', paddingRight: '4px' };
const cursorPointer = { cursor: 'pointer' as const };

const Sidebar = memo(function Sidebar({
  activeTab,
  onTabChange,
}: {
  activeTab: 'code' | 'write';
  onTabChange: (tab: 'code' | 'write') => void;
}) {
  return (
    <aside className="sidebar">
      <div className="sidebar-header-actions">
        <button className="icon-btn"><LayoutGridIcon size={16} /></button>
      </div>

      <div className="toggle-group">
        <button
          className={`toggle-btn ${activeTab === 'code' ? 'active' : ''}`}
          onClick={() => onTabChange('code')}
        >
          <BotIcon size={14} /> Code
        </button>
        <button
          className={`toggle-btn ${activeTab === 'write' ? 'active' : ''}`}
          onClick={() => onTabChange('write')}
        >
          <MessageSquareIcon size={14} /> Write
        </button>
      </div>

      <div className="sidebar-nav">
        <div className="nav-item"><PlusIcon size={14} /> New Agent</div>
        <div className="nav-item"><MessageSquareIcon size={14} /> New requirement</div>
        <div className="nav-item"><BoxIcon size={14} /> Plugins</div>
        <div className="nav-item"><ClockIcon size={14} /> Scheduled tasks</div>
      </div>

      <div className="projects-header">
        <span>Projects</span>
        <div style={flexGap8}>
          <SearchIcon size={12} />
          <BoxIcon size={12} />
          <FolderIcon size={12} />
        </div>
      </div>

      <div className="sidebar-nav" style={{ marginTop: '8px' }}>
        <div className="project-item">
          <span style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
            <FolderIcon size={14} color="#808080" /> agent_core
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
        <div className="nav-item"><SmartphoneIcon size={14} /> Connect phone</div>
        <div className="nav-item"><SettingsIcon size={14} /> Settings</div>
      </div>
    </aside>
  );
});

const UserRow = memo(function UserRow({ entry }: { entry: ChatEntry }) {
  return (
    <div className="message-row user-row">
      <div style={flexColumnEnd}>
        <div className="user-msg">{entry.text}</div>
        <div style={flexRowMeta}>
          <span>deepseek-v4-pro</span>
          <CopyIcon size={12} style={cursorPointer} />
          <Edit2Icon size={12} style={cursorPointer} />
        </div>
      </div>
    </div>
  );
});

const AgentRow = memo(function AgentRow({ entry }: { entry: ChatEntry }) {
  return (
    <div className="message-row agent-row">
      <AgentTurnUI entry={entry} />
    </div>
  );
});

const ChatInput = memo(function ChatInput({
  isProcessing,
  onSend,
  entriesLength,
}: {
  isProcessing: boolean;
  onSend: (msg: string) => void;
  entriesLength: number;
}) {
  const [input, setInput] = useState('');

  const handleSend = useCallback(() => {
    const trimmed = input.trim();
    if (!trimmed || isProcessing) return;
    setInput('');
    onSend(trimmed);
  }, [input, isProcessing, onSend]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  }, [handleSend]);

  return (
    <div className="input-area">
      <div className="input-container">
        <textarea
          className="chat-input"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Ask the agent..."
          rows={1}
        />
        <div className="input-actions">
          <div className="input-actions-left">
            <button className="icon-btn"><PlusIcon size={16} /></button>
          </div>
          <div className="input-actions-right">
            <div className="model-selector">
              deepseek-v4-pro <span style={{ color: '#555' }}>Ultra</span> <ChevronDownIcon size={12} />
            </div>
            <button
              className="send-btn"
              onClick={handleSend}
              disabled={!input.trim() || isProcessing}
            >
              <SendIcon size={14} />
            </button>
          </div>
        </div>
      </div>

      <div className="input-footer">
        <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
          <span style={{ display: 'flex', alignItems: 'center', gap: '4px' }}><TerminalSquareIcon size={10} /> tauri <ChevronDownIcon size={10} /></span>
          <span>10.3k tokens</span>
          <span>$0.0003</span>
          <span>cache 97%</span>
          <span>{Math.floor(entriesLength / 2)} turns</span>
        </div>
        <div>Type / for commands</div>
      </div>
    </div>
  );
});

function App() {
  const dispatch = useDispatch();
  const entries = useSelector((state: RootState) => state.chat.entries);
  const isProcessing = useSelector((state: RootState) => state.chat.isProcessing);
  const [activeTab, setActiveTab] = useState<'code' | 'write'>('code');
  const messagesEndRef = useRef<HTMLDivElement>(null);

  // Scroll only when new entries are added or processing state toggles,
  // not on every streaming token update.
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [entries.length, isProcessing]);

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

  const handleSend = useCallback(async (msg: string) => {
    dispatch(userMessageSent(msg));
    try {
      await invoke('send_message', { message: msg });
    } catch (e) {
      console.error('Invoke error:', e);
      dispatch(agentEventReceived({ Error: String(e) }));
    }
  }, [dispatch]);

  return (
    <div className="app-container">
      <Sidebar activeTab={activeTab} onTabChange={setActiveTab} />

      <main className="main-area">
        <header className="main-header">
          <div className="header-title">check the weather for Shenzhen<br/><span style={{ fontSize: '11px', color: '#555' }}>agent_core &middot; Agent &middot; now &middot; 10.3k tok / $0.0003 / cache 97%</span></div>
          <div className="header-actions">
            <button className="icon-btn"><BoxIcon size={14} /></button>
            <button className="icon-btn"><MessageSquareIcon size={14} /></button>
            <button className="icon-btn"><TerminalSquareIcon size={14} /></button>
            <button className="icon-btn"><FolderIcon size={14} /></button>
            <button className="icon-btn"><Maximize2Icon size={14} /></button>
          </div>
        </header>

        {entries.length === 0 ? (
          <div className="empty-state">
            <div className="hero-logo">
              <BotIcon className="hero-icon" />
            </div>
          </div>
        ) : (
          <div className="chat-history">
            {entries.map((entry) =>
              entry.type === 'user' ? (
                <UserRow key={entry.id} entry={entry} />
              ) : (
                <AgentRow key={entry.id} entry={entry} />
              )
            )}
            <div ref={messagesEndRef} />
          </div>
        )}

        <ChatInput isProcessing={isProcessing} onSend={handleSend} entriesLength={entries.length} />
      </main>
    </div>
  );
}

export default App;
