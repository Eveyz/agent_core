import { useState, useRef, useCallback, useEffect, memo } from 'react';
import { useSelector } from 'react-redux';
import DOMPurify from 'dompurify';
import { RootState } from '../../store';
import SendIcon from 'lucide-react/dist/esm/icons/send.mjs';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import TerminalSquareIcon from 'lucide-react/dist/esm/icons/terminal-square.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronUpIcon from 'lucide-react/dist/esm/icons/chevron-up.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import FileIcon from 'lucide-react/dist/esm/icons/file.mjs';
import FileCodeIcon from 'lucide-react/dist/esm/icons/file-code.mjs';
import FileJsonIcon from 'lucide-react/dist/esm/icons/file-json.mjs';
import FileTextIcon from 'lucide-react/dist/esm/icons/file-text.mjs';
import ImageIcon from 'lucide-react/dist/esm/icons/image.mjs';
import GitBranchIcon from 'lucide-react/dist/esm/icons/git-branch.mjs';
import SquareIcon from 'lucide-react/dist/esm/icons/square.mjs';
import ZapIcon from 'lucide-react/dist/esm/icons/zap.mjs';
import ClockIcon from 'lucide-react/dist/esm/icons/clock.mjs';
import MessageSquareIcon from 'lucide-react/dist/esm/icons/message-square.mjs';
import BookOpenIcon from 'lucide-react/dist/esm/icons/book-open.mjs';
import TargetIcon from 'lucide-react/dist/esm/icons/target.mjs';
import BotIcon from 'lucide-react/dist/esm/icons/bot.mjs';
import Trash2Icon from 'lucide-react/dist/esm/icons/trash-2.mjs';
import HelpCircleIcon from 'lucide-react/dist/esm/icons/help-circle.mjs';
import TodoPanel from './TodoPanel';

import { 
  SiJavascript, SiTypescript, SiReact, SiPython, SiGo, SiCss, SiHtml5, SiRust 
} from 'react-icons/si';

import { ModelSelector } from './ModelSelector';
import { SkillSelector } from './SkillSelector';
import ModeSelector from './ModeSelector';
import { useAutocomplete } from '../../hooks/useAutocomplete';
import { useGitBranch } from '../../hooks/useGitBranch';
import { useTokenCount, useTurnCount, useCacheHitRate } from '../../hooks/useTokenCount';
import type { SkillManifest } from '../../features/chat/types';
import '../../styles/skill-selector.css';

export const ChatInput = memo(function ChatInput({
  isProcessing,
  onSend,
  onAbort,
  currentModel,
  onSteer,
  onBtwQuery,
  disabled,
  disabledMessage,
  pendingSteerCount = 0,
}: {
  isProcessing: boolean;
  onSend: (msg: string) => void;
  onAbort: () => void;
  currentModel: string;
  onSteer?: (message: string) => void;
  onBtwQuery?: (question: string) => void;
  disabled?: boolean;
  disabledMessage?: string;
  pendingSteerCount?: number;
}) {
  const [input, setInput] = useState('');
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const overlayRef = useRef<HTMLPreElement>(null);
  const isComposingRef = useRef(false);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const [skillSelectorOpen, setSkillSelectorOpen] = useState(false);
  const [showSteerQueue, setShowSteerQueue] = useState(false);

  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const activeProject = projects.find((p) => p.id === activeProjectId);
  const steerQueue = useSelector((state: RootState) => state.chat.steerQueue[state.project.activeSessionId ?? '']);

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
  } = useAutocomplete(input, setInput, textareaRef, activeProject?.path, activeProjectId === '__adhoc_chat__');

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
    if (!trimmed) return;

    // /btw and /learn bypass the isProcessing gate (side-channel, parallel with the main run)
    if (trimmed === '/btw' || trimmed.startsWith('/btw ')) {
      const question = trimmed === '/btw' ? '' : trimmed.slice(5).trim();
      if (question) { setInput(''); onBtwQuery?.(question); closeAutocomplete(); }
      return;
    }


    if (isProcessing) return;
    setInput('');
    onSend(trimmed);
    closeAutocomplete();
  }, [input, isProcessing, onSend, onBtwQuery, closeAutocomplete]);

  const handleAbort = useCallback(() => {
    onAbort();
  }, [onAbort]);

  const handleSkillSelect = useCallback((skill: SkillManifest) => {
    setInput((prev) => {
      const el = textareaRef.current;
      const cursorPos = el?.selectionStart ?? prev.length;
      const before = prev.slice(0, cursorPos);
      const after = prev.slice(cursorPos);
      return `${before}@skill:${skill.name} ${after}`;
    });
    // Restore cursor position after state update
    requestAnimationFrame(() => {
      const el = textareaRef.current;
      if (el) {
        const insertLen = `@skill:${skill.name} `.length;
        const cursorPos = el.selectionStart;
        const newPos = cursorPos + insertLen;
        el.setSelectionRange(newPos, newPos);
        el.focus();
      }
    });
  }, [setInput, textareaRef]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      // IME composition: Enter confirms the candidate, doesn't send
      if (e.nativeEvent.isComposing || isComposingRef.current) {
        return;
      }

      // Cmd/Ctrl+K to open skill selector
      if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
        e.preventDefault();
        setSkillSelectorOpen(true);
        return;
      }

      if (handleAutocompleteKeydown(e)) return;
      if (handleMentionBackspace(e)) return;

      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        if (isProcessing && onSteer) {
          const trimmed = input.trim();
          if (trimmed) {
            onSteer(trimmed);
            setInput('');
            closeAutocomplete();
          }
        } else {
          handleSend();
        }
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

  const onCompositionStart = useCallback(() => {
    isComposingRef.current = true;
  }, []);

  const onCompositionEnd = useCallback(() => {
    // Delay clearing the composing flag because some browsers (e.g., Safari/Chrome on macOS)
    // fire `compositionend` immediately BEFORE the `keydown` event for Enter.
    // If we clear it synchronously, the Enter keydown will incorrectly send the message.
    setTimeout(() => {
      isComposingRef.current = false;
    }, 50);
  }, []);

  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = Math.min(el.scrollHeight, 200) + 'px';
  }, [input]);

  useEffect(() => {
    if (showAutocomplete && dropdownRef.current) {
      const selected = dropdownRef.current.querySelector('.autocomplete-item.selected') as HTMLElement;
      if (selected) {
        selected.scrollIntoView({ block: 'nearest' });
      }
    }
  }, [selectedIndex, showAutocomplete]);

  const handleScroll = useCallback(() => {
    if (overlayRef.current && textareaRef.current) {
      overlayRef.current.scrollTop = textareaRef.current.scrollTop;
      overlayRef.current.scrollLeft = textareaRef.current.scrollLeft;
    }
  }, []);

  return (
    <div className="input-area">
      <TodoPanel />
      <div className={`input-container${isProcessing ? ' steer-active' : ''}`} style={{ position: 'relative' }}>
        {showAutocomplete && autocompleteItems.length > 0 && (
          <div className="autocomplete-dropdown" ref={dropdownRef}>
            {autocompleteItems.map((item, idx) => (
              <div
                key={item.value + idx}
                className={`autocomplete-item ${idx === selectedIndex ? 'selected' : ''}`}
                onClick={() => insertAutocompleteItem(item)}
                onMouseEnter={() => setSelectedIndex(idx)}
              >
                {item.icon === 'folder' && <FolderIcon size={14} color="var(--text-tertiary)" />}
                {item.icon === 'file' && <FileIcon size={14} color="var(--text-tertiary)" />}
                {item.icon === 'file-code' && <FileCodeIcon size={14} color="var(--text-tertiary)" />}
                {item.icon === 'file-json' && <FileJsonIcon size={14} color="var(--text-tertiary)" />}
                {item.icon === 'file-text' && <FileTextIcon size={14} color="var(--text-tertiary)" />}
                {item.icon === 'file-image' && <ImageIcon size={14} color="var(--text-tertiary)" />}
                {item.icon === 'lang-js' && <SiJavascript size={14} color="#f7df1e" />}
                {item.icon === 'lang-ts' && <SiTypescript size={14} color="#3178c6" />}
                {item.icon === 'lang-jsx' && <SiReact size={14} color="#61dafb" />}
                {item.icon === 'lang-tsx' && <SiReact size={14} color="#61dafb" />}
                {item.icon === 'lang-py' && <SiPython size={14} color="#3776ab" />}
                {item.icon === 'lang-go' && <SiGo size={14} color="#00add8" />}
                {item.icon === 'lang-css' && <SiCss size={14} color="#1572b6" />}
                {item.icon === 'lang-rs' && <SiRust size={14} color="#dea584" />}
                {item.icon === 'lang-html' && <SiHtml5 size={14} color="#e34f26" />}
                {item.icon === 'command' && <TerminalSquareIcon size={14} color="var(--accent)" />}
                {item.icon === 'cmd-btw' && <MessageSquareIcon size={14} color="var(--info)" />}
                {item.icon === 'cmd-learn' && <BookOpenIcon size={14} color="var(--amber-500)" />}
                {item.icon === 'cmd-goal' && <TargetIcon size={14} color="var(--danger)" />}
                {item.icon === 'cmd-subagents' && <BotIcon size={14} color="#6366f1" />}
                {item.icon === 'cmd-clear' && <Trash2Icon size={14} color="var(--gray-400)" />}
                {item.icon === 'cmd-help' && <HelpCircleIcon size={14} color="var(--accent)" />}
                {item.icon === 'skill' && <ZapIcon size={14} color="var(--violet-500)" />}
                <span className="autocomplete-label">{item.label}</span>
                {item.description && (
                  <span style={{ marginLeft: 8, fontSize: 11, opacity: 0.6 }}>{item.description}</span>
                )}
              </div>
            ))}
          </div>
        )}
        <pre
          ref={overlayRef}
          className="highlight-overlay"
          aria-hidden="true"
          dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(highlightedHTML + '<br />') }}
        />
        <textarea
          ref={textareaRef}
          className="chat-input"
          value={input}
          onChange={onChange}
          onKeyDown={handleKeyDown}
          onCompositionStart={onCompositionStart}
          onCompositionEnd={onCompositionEnd}
          onScroll={handleScroll}
          autoFocus={!disabled}
          placeholder={disabled ? (disabledMessage || 'Chat is disabled') : isProcessing ? 'Type to steer the agent... (⏎ to inject mid-run)' : 'Ask the agent...  Type @ for files, / for commands'}
          rows={1}
          disabled={disabled}
        />
        {!disabled && (
          <div className="input-actions">
          <div className="input-actions-left">
            <ModeSelector />
            <ModelSelector currentModel={currentModel} />
            <button className="icon-btn"><PlusIcon size={16} /></button>
            <SkillSelector 
              onSelect={handleSkillSelect} 
              externalOpen={skillSelectorOpen}
              onExternalOpenChange={setSkillSelectorOpen}
            />
          </div>
          <div className="input-actions-right">
            {isProcessing ? (
              <div style={{ display: 'flex', gap: '4px', alignItems: 'center' }}>
                {pendingSteerCount > 0 && (
                  <div
                    className="steer-pending-badge"
                    onClick={() => setShowSteerQueue((s) => !s)}
                    title={`${pendingSteerCount} steering message(s) queued`}
                  >
                    <ClockIcon size={11} />
                    <span>{pendingSteerCount} queued</span>
                  </div>
                )}
                {showSteerQueue && pendingSteerCount > 0 && (
                  <div className="steer-queue-preview">
                    {steerQueue.filter((s) => s.status === 'pending').map((s) => (
                      <div key={s.steerId} className="steer-queue-item">
                        <span className="steer-queue-item-text">{s.text}</span>
                      </div>
                    ))}
                  </div>
                )}
                {onSteer && input.trim() && (
                  <button
                    className="send-btn steer-send-btn"
                    onClick={() => { onSteer(input.trim()); setInput(''); }}
                    title="Steer — inject this message mid-run"
                  >
                    <SendIcon size={14} />
                  </button>
                )}
                <button className="send-btn stop-btn" onClick={handleAbort} title="Stop (Esc)">
                  <SquareIcon size={14} fill="currentColor" />
                </button>
              </div>
            ) : (
              <button
                className="send-btn"
                onClick={handleSend}
                disabled={!input.trim()}
              >
                <SendIcon size={14} />
              </button>
            )}
          </div>
        </div>
        )}
      </div>

      {!disabled && (
      <div className="input-footer">
        <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
          {!(activeProjectId === '__adhoc_chat__') && (
            <div ref={branchDropdownRef} style={{ position: 'relative' }}>
              <span
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: '4px',
                  cursor: 'pointer',
                  color: branchError ? 'var(--danger)' : undefined,
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
                    <div className="dropdown-item" style={{ color: 'var(--text-tertiary)', cursor: 'default' }}>
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
          )}
          <ChatStats />
        </div>
        <div>{isProcessing ? 'Press Esc to stop' : 'Type @ for files, / for commands'}</div>
      </div>
      )}
    </div>
  );
});

const ChatStats = memo(function ChatStats() {
  const tokenCount = useTokenCount();
  const turnCount = useTurnCount();
  const cacheHitRate = useCacheHitRate();
  return (
    <>
      <span>{tokenCount >= 1000 ? `${(tokenCount / 1000).toFixed(1)}k` : tokenCount} tokens</span>
      <span>{turnCount} turns</span>
      {cacheHitRate !== null && (
        <span>Cache hit: {Math.round(cacheHitRate * 100)}%</span>
      )}
    </>
  );
});
