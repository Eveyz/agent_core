import { useState, useCallback, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { parseMentions, findMentionBoundaries } from '../utils/mentions';

export type IconType = 'folder' | 'file' | 'command' | 'file-code' | 'file-json' | 'file-image' | 'file-text'
  | 'lang-js' | 'lang-ts' | 'lang-jsx' | 'lang-tsx' | 'lang-py' | 'lang-go' | 'lang-css' | 'lang-rs' | 'lang-html';

export interface AutocompleteItem {
  label: string;
  value: string;
  icon: IconType;
}

function getFileIcon(filename: string): IconType {
  const ext = filename.split('.').pop()?.toLowerCase();
  switch (ext) {
    case 'js': return 'lang-js';
    case 'jsx': return 'lang-jsx';
    case 'ts': return 'lang-ts';
    case 'tsx': return 'lang-tsx';
    case 'py': return 'lang-py';
    case 'go': return 'lang-go';
    case 'css': return 'lang-css';
    case 'rs': return 'lang-rs';
    case 'html': return 'lang-html';
    case 'java':
    case 'c':
    case 'cpp':
    case 'h':
    case 'php':
    case 'rb':
      return 'file-code';
    case 'json':
      return 'file-json';
    case 'md':
    case 'txt':
      return 'file-text';
    case 'png':
    case 'jpg':
    case 'jpeg':
    case 'gif':
    case 'svg':
      return 'file-image';
    default:
      return 'file';
  }
}

const COMMANDS: AutocompleteItem[] = [
  { label: 'subagents', value: '/subagents ', icon: 'command' },
  { label: 'btw', value: '/btw ', icon: 'command' },
  { label: 'clear', value: '/clear', icon: 'command' },
  { label: 'help', value: '/help', icon: 'command' },
];

export function useAutocomplete(
  input: string,
  setInput: (v: string) => void,
  textareaRef: React.RefObject<HTMLTextAreaElement | null>,
  projectPath?: string
) {
  const [showAutocomplete, setShowAutocomplete] = useState(false);
  const [autocompleteItems, setAutocompleteItems] = useState<AutocompleteItem[]>([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [triggerInfo, setTriggerInfo] = useState<{ start: number; end: number; type: '@' | '/' } | null>(null);

  const fetchDirectoryEntries = useCallback(async (query: string) => {
    try {
      const entries = await invoke<Array<{ name: string; type: string }>>('search_files', { 
        query, 
        path: projectPath || null 
      });
      const mapped = entries.map((e) => {
        const basename = e.name.split('/').pop() || e.name;
        return {
          label: e.name,
          value: basename,
          icon: e.type === 'dir' ? 'folder' : getFileIcon(e.name),
        };
      });
      setAutocompleteItems(mapped);
      setSelectedIndex(0);
    } catch {
      setAutocompleteItems([]);
    }
  }, [projectPath]);

  const closeAutocomplete = useCallback(() => {
    setShowAutocomplete(false);
    setAutocompleteItems([]);
    setTriggerInfo(null);
  }, []);

  const insertAutocompleteItem = useCallback(
    (item: AutocompleteItem) => {
      if (!triggerInfo || !textareaRef.current) return;
      const before = input.slice(0, triggerInfo.start);
      const after = input.slice(triggerInfo.end);
      let insertValue = item.value;
      if (triggerInfo.type === '@') {
        const isDir = item.icon === 'folder';
        insertValue = `@${item.label}${isDir ? '/' : ''} `;
      }
      const newValue = before + insertValue + after;
      setInput(newValue);
      closeAutocomplete();
      setTimeout(() => {
        const pos = before.length + insertValue.length;
        textareaRef.current?.setSelectionRange(pos, pos);
        textareaRef.current?.focus();
      }, 0);
    },
    [input, triggerInfo, closeAutocomplete, setInput, textareaRef]
  );

  const handleAutocompleteKeydown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>): boolean => {
      if (showAutocomplete && autocompleteItems.length > 0) {
        if (e.key === 'ArrowDown') {
          e.preventDefault();
          setSelectedIndex((prev) => (prev + 1) % autocompleteItems.length);
          return true;
        }
        if (e.key === 'ArrowUp') {
          e.preventDefault();
          setSelectedIndex((prev) => (prev - 1 + autocompleteItems.length) % autocompleteItems.length);
          return true;
        }
        if (e.key === 'Enter' || e.key === 'Tab') {
          e.preventDefault();
          insertAutocompleteItem(autocompleteItems[selectedIndex]);
          return true;
        }
        if (e.key === 'Escape') {
          e.preventDefault();
          closeAutocomplete();
          return true;
        }
      }
      return false;
    },
    [showAutocomplete, autocompleteItems, selectedIndex, insertAutocompleteItem, closeAutocomplete]
  );

  const handleMentionBackspace = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>): boolean => {
      const el = textareaRef.current;
      if (!el) return false;
      const cursorPos = el.selectionStart;

      if (e.key === 'Backspace' && !e.shiftKey && !e.metaKey && !e.ctrlKey) {
        const boundaries = findMentionBoundaries(input, cursorPos);
        if (boundaries) {
          e.preventDefault();
          const [start, end] = boundaries;
          const newValue = input.slice(0, start) + input.slice(end);
          setInput(newValue);
          closeAutocomplete();
          setTimeout(() => {
            el.setSelectionRange(start, start);
            el.focus();
          }, 0);
          return true;
        }
      }

      if (e.key === 'Delete' && !e.shiftKey && !e.metaKey && !e.ctrlKey) {
        const boundaries = findMentionBoundaries(input, cursorPos + 1);
        if (boundaries && boundaries[0] === cursorPos) {
          e.preventDefault();
          const [start, end] = boundaries;
          const newValue = input.slice(0, start) + input.slice(end);
          setInput(newValue);
          closeAutocomplete();
          setTimeout(() => {
            el.setSelectionRange(start, start);
            el.focus();
          }, 0);
          return true;
        }
      }

      return false;
    },
    [input, setInput, textareaRef]
  );

  const handleChange = useCallback(
    (value: string, cursorPos: number) => {
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
        if (/\s/.test(char)) break;
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
    },
    [fetchDirectoryEntries, closeAutocomplete]
  );

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

  return {
    showAutocomplete,
    autocompleteItems,
    selectedIndex,
    setSelectedIndex,
    insertAutocompleteItem,
    closeAutocomplete,
    handleAutocompleteKeydown,
    handleMentionBackspace,
    handleChange,
    highlightedHTML,
  };
}
