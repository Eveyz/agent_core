import { useState, useRef, useCallback, useEffect, useMemo, memo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import SendIcon from 'lucide-react/dist/esm/icons/send.mjs';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import TerminalSquareIcon from 'lucide-react/dist/esm/icons/terminal-square.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import FileIcon from 'lucide-react/dist/esm/icons/file.mjs';
import { parseMentions, findMentionBoundaries } from '../../utils/mentions';
import { ModelSelector } from './ModelSelector';

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
  entriesLength,
  currentModel,
}: {
  isProcessing: boolean;
  onSend: (msg: string) => void;
  entriesLength: number;
  currentModel: string;
}) {
  const [input, setInput] = useState('');
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const overlayRef = useRef<HTMLPreElement>(null);

  const [showAutocomplete, setShowAutocomplete] = useState(false);
  const [autocompleteItems, setAutocompleteItems] = useState<AutocompleteItem[]>([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [triggerInfo, setTriggerInfo] = useState<{ start: number; end: number; type: '@' | '/' } | null>(null);

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
