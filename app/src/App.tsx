import { useState, useEffect, useRef, memo, useCallback, useMemo } from 'react';
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
import FileIcon from 'lucide-react/dist/esm/icons/file.mjs';
import SparklesIcon from 'lucide-react/dist/esm/icons/sparkles.mjs';
import Code2Icon from 'lucide-react/dist/esm/icons/code-2.mjs';
import RefreshCwIcon from 'lucide-react/dist/esm/icons/refresh-cw.mjs';
import CompassIcon from 'lucide-react/dist/esm/icons/compass.mjs';
import { RootState } from './store';
import { agentEventReceived, userMessageSent, ChatEntry } from './features/chat/chatSlice';
import { AgentTurnUI } from './components/chat/AgentTurn';
import './App.css';

const flexGap8 = { display: 'flex', gap: '8px' } as const;
const flexColumnEnd = { display: 'flex', flexDirection: 'column' as const, alignItems: 'flex-end', gap: '6px' };
const flexRowMeta = { display: 'flex', gap: '12px', color: '#555', fontSize: '11px', paddingRight: '4px' };
const cursorPointer = { cursor: 'pointer' as const };

const PROMPT_SUGGESTIONS = [
  {
    icon: 'search',
    label: 'Search for TODO comments and FIXME notes across the codebase',
  },
  {
    icon: 'refactor',
    label: 'Refactor error handling to use thiserror and anyhow consistently',
  },
  {
    icon: 'explore',
    label: 'Explain how subagents are spawned and how they share the tool registry',
  },
  {
    icon: 'create',
    label: 'Write comprehensive unit tests for the permission module',
  },
];

const CosmicBackground = memo(function CosmicBackground() {
  return (
    <>
      <div className="cosmic-glow cosmic-glow-1" />
      <div className="cosmic-glow cosmic-glow-2" />
      <div className="cosmic-glow cosmic-glow-3" />
      <div className="cosmic-glow cosmic-glow-4" />
      <div className="star-field" />
    </>
  );
});

const EmptyState = memo(function EmptyState({ onSend }: { onSend: (msg: string) => void }) {
  return (
    <div className="empty-state">
      <div className="empty-state-content">
        {/* Sun + orbiting planets */}
        <div className="solar-system">
          <div className="sun" />
          <div className="planet-orbit orbit-1">
            <div className="planet planet-1" />
          </div>
          <div className="planet-orbit orbit-2">
            <div className="planet planet-2" />
          </div>
          <div className="planet-orbit orbit-3">
            <div className="planet planet-3" />
          </div>
        </div>

        <h1 className="empty-state-title">What can I help you build?</h1>
        <p className="empty-state-subtitle">
          Spawn subagents, analyze code, and orchestrate complex tasks.
        </p>

        <div className="prompt-suggestions">
          {PROMPT_SUGGESTIONS.map((s, i) => (
            <button
              key={i}
              className="prompt-card"
              onClick={() => onSend(s.label)}
            >
              <div className="prompt-card-icon">
                {s.icon === 'search' && <Code2Icon size={16} />}
                {s.icon === 'refactor' && <RefreshCwIcon size={16} />}
                {s.icon === 'explore' && <CompassIcon size={16} />}
                {s.icon === 'create' && <SparklesIcon size={16} />}
              </div>
              <span className="prompt-card-text">{s.label}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
});

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

interface AutocompleteItem {
  label: string;
  value: string;
  icon: 'folder' | 'file' | 'command';
}

const COMMANDS: AutocompleteItem[] = [
  { label: 'subagents', value: '/subagents ', icon: 'command' },
  { label: 'btw', value: '/btw ', icon: 'command' },
  { label: 'clear', value: '/clear', icon: 'command' },
  { label: 'help', value: '/help', icon: 'command' },
];

// Parse @mentions from text. Returns array of { type, value }.
function parseMentions(text: string): Array<{ type: 'text' | 'mention'; value: string }> {
  const tokens: Array<{ type: 'text' | 'mention'; value: string }> = [];
  let lastIndex = 0;
  const regex = /@[^\s]+/g;
  let match: RegExpExecArray | null;
  while ((match = regex.exec(text)) !== null) {
    if (match.index > lastIndex) {
      tokens.push({ type: 'text', value: text.slice(lastIndex, match.index) });
    }
    tokens.push({ type: 'mention', value: match[0] });
    lastIndex = match.index + match[0].length;
  }
  if (lastIndex < text.length) {
    tokens.push({ type: 'text', value: text.slice(lastIndex) });
  }
  if (tokens.length === 0 && text) {
    tokens.push({ type: 'text', value: text });
  }
  return tokens;
}

// Find the mention token that contains or is adjacent to position.
// Returns [start, end] of the mention, or null.
function findMentionBoundaries(text: string, pos: number): [number, number] | null {
  const regex = /@[^\s]+/g;
  let match: RegExpExecArray | null;
  while ((match = regex.exec(text)) !== null) {
    const start = match.index;
    const end = start + match[0].length;
    // If cursor is inside or immediately after the mention, treat it as part of the mention
    if (pos > start && pos <= end) {
      return [start, end];
    }
  }
  return null;
}

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
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const overlayRef = useRef<HTMLPreElement>(null);

  // Autocomplete state
  const [showAutocomplete, setShowAutocomplete] = useState(false);
  const [autocompleteItems, setAutocompleteItems] = useState<AutocompleteItem[]>([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [triggerInfo, setTriggerInfo] = useState<{ start: number; end: number; type: '@' | '/' } | null>(null);

  // Fetch directory entries for @ trigger
  const fetchDirectoryEntries = useCallback(async (query: string) => {
    try {
      const entries = await invoke<Array<{ name: string; type: string }>>('list_directory', { path: null });
      const filtered = entries
        .filter((e) => e.name.toLowerCase().includes(query.toLowerCase()))
        .map((e) => ({
          label: e.name,
          value: e.name,
          icon: (e.type === 'directory' ? 'folder' : 'file') as 'folder' | 'file',
        }));
      setAutocompleteItems(filtered);
      setSelectedIndex(0);
    } catch (e) {
      console.error('Failed to list directory:', e);
      setAutocompleteItems([]);
    }
  }, []);

  const closeAutocomplete = useCallback(() => {
    setShowAutocomplete(false);
    setAutocompleteItems([]);
    setTriggerInfo(null);
  }, []);

  const insertAutocompleteItem = useCallback((item: AutocompleteItem) => {
    if (!triggerInfo || !textareaRef.current) return;
    const before = input.slice(0, triggerInfo.start);
    const after = input.slice(triggerInfo.end);
    let insertValue = item.value;
    if (triggerInfo.type === '@') {
      insertValue = `@${item.value} `;
    }
    const newValue = before + insertValue + after;
    setInput(newValue);
    closeAutocomplete();
    // Restore cursor after insertion
    setTimeout(() => {
      const pos = before.length + insertValue.length;
      textareaRef.current?.setSelectionRange(pos, pos);
      textareaRef.current?.focus();
    }, 0);
  }, [input, triggerInfo, closeAutocomplete]);

  const handleSend = useCallback(() => {
    const trimmed = input.trim();
    if (!trimmed || isProcessing) return;
    setInput('');
    onSend(trimmed);
    closeAutocomplete();
  }, [input, isProcessing, onSend, closeAutocomplete]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (showAutocomplete && autocompleteItems.length > 0) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelectedIndex((prev) => (prev + 1) % autocompleteItems.length);
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelectedIndex((prev) => (prev - 1 + autocompleteItems.length) % autocompleteItems.length);
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        insertAutocompleteItem(autocompleteItems[selectedIndex]);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        closeAutocomplete();
        return;
      }
    }

    const el = textareaRef.current;
    if (!el) return;
    const cursorPos = el.selectionStart;

    // Backspace: if cursor is inside or right after a mention, delete the whole mention
    if (e.key === 'Backspace' && !e.shiftKey && !e.metaKey && !e.ctrlKey) {
      const boundaries = findMentionBoundaries(input, cursorPos);
      if (boundaries) {
        e.preventDefault();
        const [start, end] = boundaries;
        const newValue = input.slice(0, start) + input.slice(end);
        setInput(newValue);
        setTimeout(() => {
          el.setSelectionRange(start, start);
          el.focus();
        }, 0);
        return;
      }
    }

    // Delete: if cursor is right before a mention, delete the whole mention
    if (e.key === 'Delete' && !e.shiftKey && !e.metaKey && !e.ctrlKey) {
      const boundaries = findMentionBoundaries(input, cursorPos + 1);
      if (boundaries && boundaries[0] === cursorPos) {
        e.preventDefault();
        const [start, end] = boundaries;
        const newValue = input.slice(0, start) + input.slice(end);
        setInput(newValue);
        setTimeout(() => {
          el.setSelectionRange(start, start);
          el.focus();
        }, 0);
        return;
      }
    }

    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  }, [showAutocomplete, autocompleteItems, selectedIndex, insertAutocompleteItem, closeAutocomplete, handleSend, input]);

  const handleChange = useCallback((e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const value = e.target.value;
    const cursorPos = e.target.selectionStart;
    setInput(value);

    // Find the nearest trigger (@ or /) before cursor
    let triggerStart = -1;
    let triggerType: '@' | '/' | null = null;

    for (let i = cursorPos - 1; i >= 0; i--) {
      const char = value[i];
      if (char === '@' || char === '/') {
        // Only trigger if it's at start or preceded by whitespace
        if (i === 0 || /\s/.test(value[i - 1])) {
          triggerStart = i;
          triggerType = char;
        }
        break;
      }
      if (/\s/.test(char)) {
        break;
      }
    }

    if (triggerStart !== -1 && triggerType) {
      const query = value.slice(triggerStart + 1, cursorPos);
      setTriggerInfo({ start: triggerStart, end: cursorPos, type: triggerType });
      setShowAutocomplete(true);

      if (triggerType === '@') {
        fetchDirectoryEntries(query);
      } else if (triggerType === '/') {
        const filtered = COMMANDS.filter((c) => c.label.toLowerCase().includes(query.toLowerCase()));
        setAutocompleteItems(filtered);
        setSelectedIndex(0);
      }
    } else {
      closeAutocomplete();
    }
  }, [fetchDirectoryEntries, closeAutocomplete]);

  // Auto-resize textarea & sync overlay scroll
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = Math.min(el.scrollHeight, 200) + 'px';
  }, [input]);

  const handleScroll = useCallback(() => {
    if (overlayRef.current && textareaRef.current) {
      overlayRef.current.scrollTop = textareaRef.current.scrollTop;
      overlayRef.current.scrollLeft = textareaRef.current.scrollLeft;
    }
  }, []);

  // Build highlighted HTML for the overlay
  const highlightedHTML = useMemo(() => {
    const tokens = parseMentions(input);
    return tokens
      .map((t) => {
        if (t.type === 'mention') {
          const escaped = t.value
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;');
          return `<span class="mention-token">${escaped}</span>`;
        }
        return t.value
          .replace(/&/g, '&amp;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;');
      })
      .join('');
  }, [input]);

  return (
    <div className="input-area">
      <div className="input-container" style={{ position: 'relative' }}>
        {showAutocomplete && autocompleteItems.length > 0 && (
          <div className="autocomplete-dropdown">
            {autocompleteItems.map((item, idx) => (
              <div
                key={item.value + idx}
                className={`autocomplete-item ${idx === selectedIndex ? 'selected' : ''}`}
                onClick={() => insertAutocompleteItem(item)}
                onMouseEnter={() => setSelectedIndex(idx)}
              >
                {item.icon === 'folder' && <FolderIcon size={14} color="#808080" />}
                {item.icon === 'file' && <FileIcon size={14} color="#808080" />}
                {item.icon === 'command' && <TerminalSquareIcon size={14} color="#52A8FF" />}
                <span className="autocomplete-label">{item.label}</span>
              </div>
            ))}
          </div>
        )}
        {/* Highlight overlay: shows colored @mentions behind the textarea */}
        <pre
          ref={overlayRef}
          className="highlight-overlay"
          aria-hidden="true"
          dangerouslySetInnerHTML={{ __html: highlightedHTML + '<br />' }}
        />
        <textarea
          ref={textareaRef}
          className="chat-input"
          value={input}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          onScroll={handleScroll}
          placeholder="Ask the agent...  Type @ for files, / for commands"
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
        <div>Type @ for files, / for commands</div>
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
        <CosmicBackground />
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
          <EmptyState onSend={handleSend} />
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
