import { useState, useRef, useCallback, useEffect, memo } from 'react';
import { useSelector } from 'react-redux';
import { RootState } from '../../store';
import SendIcon from 'lucide-react/dist/esm/icons/send.mjs';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import TerminalSquareIcon from 'lucide-react/dist/esm/icons/terminal-square.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronUpIcon from 'lucide-react/dist/esm/icons/chevron-up.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import FileIcon from 'lucide-react/dist/esm/icons/file.mjs';
import GitBranchIcon from 'lucide-react/dist/esm/icons/git-branch.mjs';
import { ModelSelector } from './ModelSelector';
import { useAutocomplete } from '../../hooks/useAutocomplete';
import { useGitBranch } from '../../hooks/useGitBranch';
import { useTokenCount, useTurnCount } from '../../hooks/useTokenCount';

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

  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const activeProject = projects.find((p) => p.id === activeProjectId);

  const tokenCount = useTokenCount();
  const turnCount = useTurnCount();

  const {
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
  } = useAutocomplete(input, setInput, textareaRef);

  const {
    branches,
    activeBranch,
    branchError,
    showBranchDropdown,
    setShowBranchDropdown,
    branchDropdownRef,
    handleSwitchBranch,
  } = useGitBranch(activeProject?.path);

  useEffect(() => {
    if (activeSessionId || activeProjectId) {
      setTimeout(() => {
        textareaRef.current?.focus();
      }, 150);
    }
  }, [activeSessionId, activeProjectId]);

  const handleSend = useCallback(() => {
    const trimmed = input.trim();
    if (!trimmed || isProcessing) return;
    setInput('');
    onSend(trimmed);
    closeAutocomplete();
  }, [input, isProcessing, onSend, closeAutocomplete]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (handleAutocompleteKeydown(e)) return;
      if (handleMentionBackspace(e)) return;

      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [handleAutocompleteKeydown, handleMentionBackspace, handleSend]
  );

  const onChange = useCallback(
    (e: React.ChangeEvent<HTMLTextAreaElement>) => {
      const value = e.target.value;
      const cursorPos = e.target.selectionStart;
      setInput(value);
      handleChange(value, cursorPos);
    },
    [handleChange]
  );

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
          onChange={onChange}
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
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: '4px',
                cursor: 'pointer',
                color: branchError ? '#ff5f57' : undefined,
              }}
              onClick={() => setShowBranchDropdown((s) => !s)}
              title={branchError || undefined}
            >
              <GitBranchIcon size={10} />
              {activeBranch || (activeProject ? activeProject.name : 'No project')}
              {showBranchDropdown ? <ChevronUpIcon size={10} /> : <ChevronDownIcon size={10} />}
            </span>
            {showBranchDropdown && (
              <div
                className="dropdown-menu dropdown-menu-up"
                style={{ bottom: '24px', left: 0, minWidth: '180px', maxHeight: '240px', overflowY: 'auto' }}
              >
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
                    <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{branch}</span>
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
