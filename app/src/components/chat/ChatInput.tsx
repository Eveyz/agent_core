import { useState, useRef, useCallback, useEffect, useMemo, memo } from 'react';
import { useSelector } from 'react-redux';
import { invoke } from '@tauri-apps/api/core';
import { RootState } from '../../store';
import SendIcon from 'lucide-react/dist/esm/icons/send.mjs';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import TerminalSquareIcon from 'lucide-react/dist/esm/icons/terminal-square.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronUpIcon from 'lucide-react/dist/esm/icons/chevron-up.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import FileIcon from 'lucide-react/dist/esm/icons/file.mjs';
import GitBranchIcon from 'lucide-react/dist/esm/icons/git-branch.mjs';
import { parseMentions, findMentionBoundaries } from '../../utils/mentions';
import { ModelSelector } from './ModelSelector';
import { roughTokenCount } from '../../App';

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

export const ChatInput = memo(function ChatInput({
  isProcessing,
  onSend,
  currentModel,
}: {
  isProcessing: boolean;
  onSend: (msg: string) => void;
  currentModel: string;
}) {
  const [input, setInput] = useState('');
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const overlayRef = useRef<HTMLPreElement>(null);

  const [showAutocomplete, setShowAutocomplete] = useState(false);
  const [autocompleteItems, setAutocompleteItems] = useState<AutocompleteItem[]>([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [triggerInfo, setTriggerInfo] = useState<{ start: number; end: number; type: '@' | '/' } | null>(null);

  const [showBranchDropdown, setShowBranchDropdown] = useState(false);
  const branchDropdownRef = useRef<HTMLDivElement>(null);

  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const activeProject = projects.find((p) => p.id === activeProjectId);

  useEffect(() => {
    if (activeSessionId || activeProjectId) {
      setTimeout(() => {
        textareaRef.current?.focus();
      }, 150);
    }
  }, [activeSessionId, activeProjectId]);

  const tokenCount = useSelector((state: RootState) => {
    return state.chat.entries.reduce((sum, e) => {
      if (e.type === 'user' && e.text) return sum + roughTokenCount(e.text);
      if (e.type === 'turn' && e.blocks) return sum + e.blocks.reduce((s, b) => {
        if (b.type === 'assistant' || b.type === 'thinking') return s + roughTokenCount(b.text || '');
        if (b.type === 'tool') return s + roughTokenCount(b.result || '');
        return s;
      }, 0);
      return sum;
    }, 0);
  });
  const turnCount = useSelector((state: RootState) => state.chat.entries.filter(e => e.type === 'turn').length);

  const [branches, setBranches] = useState<string[]>([]);
  const [activeBranch, setActiveBranch] = useState<string>('');
  const [branchError, setBranchError] = useState<string>('');

  useEffect(() => {
    if (activeProject?.path) {
      invoke<string[]>('list_git_branches', { path: activeProject.path })
        .then((b) => {
          setBranches(b);
          setBranchError('');
          if (b.length > 0 && !activeBranch) {
            setActiveBranch(b[0]);
          }
        })
        .catch((e) => {
          setBranches([]);
          setBranchError(String(e));
        });
    }
  }, [activeProject?.path]);

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (branchDropdownRef.current && !branchDropdownRef.current.contains(e.target as Node)) {
        setShowBranchDropdown(false);
      }
    }
    if (showBranchDropdown) {
      document.addEventListener('mousedown', handleClick);
      return () => document.removeEventListener('mousedown', handleClick);
    }
  }, [showBranchDropdown]);

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

    let triggerStart = -1;
    let triggerType: '@' | '/' | null = null;

    for (let i = cursorPos - 1; i >= 0; i--) {
      const char = value[i];
      if (char === '@' || char === '/') {
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

  const handleSwitchBranch = useCallback(async (branch: string) => {
    if (!activeProject?.path || branch === activeBranch) {
      setShowBranchDropdown(false);
      return;
    }
    try {
      await invoke('switch_git_branch', { path: activeProject.path, branch });
      setActiveBranch(branch);
      setBranchError('');
    } catch (e) {
      setBranchError(String(e));
    }
    setShowBranchDropdown(false);
  }, [activeProject?.path, activeBranch]);

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
          autoFocus={true}
          placeholder="Ask the agent...  Type @ for files, / for commands"
          rows={1}
        />
        <div className="input-actions">
          <div className="input-actions-left">
            <button className="icon-btn"><PlusIcon size={16} /></button>
          </div>
          <div className="input-actions-right">
            <ModelSelector currentModel={currentModel} />
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
          <div ref={branchDropdownRef} style={{ position: 'relative' }}>
            <span
              style={{ display: 'flex', alignItems: 'center', gap: '4px', cursor: 'pointer', color: branchError ? '#ff5f57' : undefined }}
              onClick={() => setShowBranchDropdown((s) => !s)}
              title={branchError || undefined}
            >
              <GitBranchIcon size={10} />
              {activeBranch || (activeProject ? activeProject.name : 'No project')}
              {showBranchDropdown ? <ChevronUpIcon size={10} /> : <ChevronDownIcon size={10} />}
            </span>
            {showBranchDropdown && (
              <div className="dropdown-menu dropdown-menu-up" style={{ bottom: '24px', left: 0, minWidth: '180px', maxHeight: '240px', overflowY: 'auto' }}>
                {branches.length === 0 && (
                  <div className="dropdown-item" style={{ color: '#808080', cursor: 'default' }}>
                    {branchError ? 'Not a git repo' : 'No branches'}
                  </div>
                )}
                {branches.map((branch) => (
                  <div
                    key={branch}
                    className={`dropdown-item ${activeBranch === branch ? 'dropdown-item-active' : ''}`}
                    onClick={() => handleSwitchBranch(branch)}
                  >
                    <GitBranchIcon size={12} />
                    <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {branch}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
          <span>{tokenCount >= 1000 ? `${(tokenCount / 1000).toFixed(1)}k` : tokenCount} tokens</span>
          <span>{turnCount} turns</span>
        </div>
        <div>Type @ for files, / for commands</div>
      </div>
    </div>
  );
});
