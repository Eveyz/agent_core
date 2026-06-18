import { useState, useEffect, useCallback, memo, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSelector, useStore, shallowEqual } from 'react-redux';
import PencilIcon from 'lucide-react/dist/esm/icons/pencil.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import BoxIcon from 'lucide-react/dist/esm/icons/box.mjs';
import MessageSquareIcon from 'lucide-react/dist/esm/icons/message-square.mjs';
import TerminalSquareIcon from 'lucide-react/dist/esm/icons/terminal-square.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import Maximize2Icon from 'lucide-react/dist/esm/icons/maximize-2.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import { RootState } from './store';
import {
  agentEventReceived,
  userMessageSent,
  retryFromEntry,
  agentAborted,
  selectEntryById,
  selectPendingApprovalCount,
} from './features/chat/chatSlice';
import { openSettings, fetchConfig } from './features/settings/settingsSlice';
import {
  fetchProjects,
  fetchProjectSessions,
  createSession,
  renameSession,
  resumeSession,
  setActiveSession,
} from './features/project/projectSlice';
import { useAppDispatch } from './hooks/useAppDispatch';
import { useAgentEventListener } from './hooks/useAgentEventListener';
import { useAutoSaveSession } from './hooks/useAutoSaveSession';
import { useAutoScroll } from './hooks/useAutoScroll';
import { Sidebar } from './components/layout/Sidebar';
import { CosmicBackground } from './components/layout/CosmicBackground';
import { EmptyState } from './components/chat/EmptyState';
import { UserRow } from './components/chat/UserRow';
import { AgentRow } from './components/chat/AgentRow';
import { ChatInput } from './components/chat/ChatInput';
import SettingsModal from './components/settings/SettingsModal';
import './App.css';

function getActiveSessionTitle(projectState: RootState['project']): string {
  if (!projectState.activeSessionId || !projectState.activeProjectId) return '';
  const list = projectState.sessions[projectState.activeProjectId] ?? [];
  const s = list.find((s) => s.id === projectState.activeSessionId);
  return s?.title ?? '';
}

function App() {
  const dispatch = useAppDispatch();
  const store = useStore<RootState>();

  const entryIds = useSelector((state: RootState) => state.chat.entries.map((e) => e.id), shallowEqual);
  const entriesLength = useSelector((state: RootState) => state.chat.entries.length);
  const isProcessing = useSelector((state: RootState) => state.chat.isProcessing);
  const defaultModel = useSelector((state: RootState) => state.settings.config?.default_model || '');

  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const sessionTitle = useSelector((state: RootState) => getActiveSessionTitle(state.project));

  const activeProject = projects.find((p) => p.id === activeProjectId);
  const [activeTab, setActiveTab] = useState<'code' | 'write'>('code');

  const { ref: scrollRef, scrollToBottom, forceStickToBottom, isAtBottom } = useAutoScroll<HTMLDivElement>();

  useAgentEventListener();

  useAutoSaveSession({
    activeSessionId,
    activeProjectPath: activeProject?.path ?? null,
    defaultModel,
  });

  // Track pending approvals — scroll to bottom when a new one appears
  const pendingApprovalCount = useSelector(selectPendingApprovalCount);
  const prevPendingRef = useRef(0);
  useEffect(() => {
    if (pendingApprovalCount > prevPendingRef.current) {
      scrollToBottom();
    }
    prevPendingRef.current = pendingApprovalCount;
  }, [pendingApprovalCount, scrollToBottom]);

  // When a session is opened/switched, force-stick to the bottom. This pins the
  // view through the async reflows (markdown, code blocks, tool calls) that happen
  // as a loaded session renders, so the latest message — and its copy icon — lands
  // fully in view. Covers both sync cache restore and async backend resume.
  const prevSessionIdRef = useRef<string | null | undefined>(activeSessionId);
  useEffect(() => {
    if (prevSessionIdRef.current !== activeSessionId) {
      prevSessionIdRef.current = activeSessionId;
      if (activeSessionId) {
        forceStickToBottom();
      }
    }
  }, [activeSessionId, forceStickToBottom]);

  // Esc key to abort agent during processing
  useEffect(() => {
    if (!isProcessing) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        dispatch(agentAborted());
        invoke('abort_agent').catch((e) => console.error('Failed to abort agent:', e));
      }
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [isProcessing, dispatch]);

  const handleAbort = useCallback(() => {
    dispatch(agentAborted());
    invoke('abort_agent').catch((e) => console.error('Failed to abort agent:', e));
  }, [dispatch]);

  useEffect(() => {
    dispatch(fetchConfig());
    dispatch(fetchProjects());
  }, [dispatch]);

  const projectsLoaded = projects.length > 0;
  useEffect(() => {
    if (!projectsLoaded || !activeProjectId || !activeSessionId) return;
    dispatch(fetchProjectSessions(activeProjectId));
    dispatch(resumeSession(activeSessionId)).then((result) => {
      if (!resumeSession.fulfilled.match(result)) {
        dispatch(setActiveSession(null));
      }
    });
  }, [projectsLoaded, dispatch, activeProjectId, activeSessionId]);

  const handleSend = useCallback(
    async (msg: string) => {
      let sessionId = activeSessionId;
      let isNewSession = false;
      if (!sessionId) {
        if (!activeProjectId) {
          console.error('No active project to create session in');
          return;
        }
        try {
          const result = await dispatch(createSession(activeProjectId));
          if (!createSession.fulfilled.match(result)) {
            console.error('Failed to create session');
            return;
          }
          sessionId = result.payload.session.id;
          isNewSession = true;
        } catch (e) {
          console.error('Failed to create session:', e);
          return;
        }
      }

      dispatch(userMessageSent(msg));
      scrollToBottom();

      const shouldRename = isNewSession || sessionTitle === 'New Session' || sessionTitle === '';
      if (shouldRename && sessionId && activeProjectId) {
        const preview = msg.trim().slice(0, 30) + (msg.trim().length > 30 ? '...' : '');
        dispatch(renameSession({ sessionId, projectId: activeProjectId, newTitle: preview }));
      }

      try {
        await invoke('send_message', { message: msg, sessionId });
      } catch (e) {
        console.error('Invoke error:', e);
        dispatch(agentEventReceived({ Error: String(e) }));
      }
    },
    [dispatch, activeProjectId, activeSessionId, sessionTitle, scrollToBottom]
  );

  const handleRetry = useCallback(
    async (entryId: string, editedText?: string) => {
      const entries = store.getState().chat.entries;
      const entry = entries.find((e) => e.id === entryId);
      if (!entry) return;
      const msg = editedText ?? entry.text ?? '';
      if (!msg.trim() || !activeSessionId) return;

      dispatch(retryFromEntry(entryId));
      scrollToBottom();

      try {
        await invoke('send_message', { message: msg, sessionId: activeSessionId });
      } catch (e) {
        console.error('Retry invoke error:', e);
        dispatch(agentEventReceived({ Error: String(e) }));
      }
    },
    [dispatch, activeSessionId, store]
  );

  const [isEditingTitle, setIsEditingTitle] = useState(false);
  const [titleEditValue, setTitleEditValue] = useState('');

  const startEditingTitle = useCallback(() => {
    setTitleEditValue(sessionTitle || 'New Session');
    setIsEditingTitle(true);
  }, [sessionTitle]);

  const commitTitleEdit = useCallback(() => {
    const trimmed = titleEditValue.trim();
    if (trimmed && activeSessionId && activeProjectId) {
      dispatch(renameSession({ sessionId: activeSessionId, projectId: activeProjectId, newTitle: trimmed }));
    }
    setIsEditingTitle(false);
  }, [dispatch, titleEditValue, activeSessionId, activeProjectId]);

  const cancelTitleEdit = useCallback(() => {
    setIsEditingTitle(false);
  }, []);

  const handleOpenSettings = useCallback(() => {
    dispatch(openSettings());
  }, [dispatch]);

  return (
    <div className="app-container">
      <Sidebar activeTab={activeTab} onTabChange={setActiveTab} onOpenSettings={handleOpenSettings} />
      <SettingsModal />

      <main className="main-area">
        <CosmicBackground />
        <header className="main-header">
          <div className="header-title">
            {isEditingTitle ? (
              <>
                <input
                  className="header-title-input"
                  value={titleEditValue}
                  onChange={(e) => setTitleEditValue(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') commitTitleEdit();
                    if (e.key === 'Escape') cancelTitleEdit();
                  }}
                  autoFocus
                />
                <button className="icon-btn header-edit-btn" onClick={commitTitleEdit} title="Save" style={{ opacity: 1 }}>
                  <CheckIcon size={12} />
                </button>
                <button className="icon-btn header-edit-btn" onClick={cancelTitleEdit} title="Cancel" style={{ opacity: 1 }}>
                  <XIcon size={12} />
                </button>
              </>
            ) : (
              <>
                <span className="header-session-name">{sessionTitle || 'New Session'}</span>
                <button className="icon-btn header-edit-btn" onClick={startEditingTitle} title="Edit session title">
                  <PencilIcon size={12} />
                </button>
              </>
            )}
          </div>
          <div className="header-actions">
            <button className="icon-btn"><BoxIcon size={14} /></button>
            <button className="icon-btn"><MessageSquareIcon size={14} /></button>
            <button className="icon-btn"><TerminalSquareIcon size={14} /></button>
            <button className="icon-btn"><FolderIcon size={14} /></button>
            <button className="icon-btn"><Maximize2Icon size={14} /></button>
          </div>
        </header>

        {entriesLength === 0 ? (
          <EmptyState onSend={handleSend} />
        ) : (
          <>
            <div className="chat-history" ref={scrollRef}>
              {entryIds.map((id) => (
                <EntryRow
                  key={id}
                  entryId={id}
                  defaultModel={defaultModel}
                  handleRetry={handleRetry}
                  isProcessing={isProcessing}
                />
              ))}
            </div>
            {!isAtBottom && (
              <button className="scroll-to-bottom-btn" onClick={scrollToBottom} title="Scroll to latest">
                <ChevronDownIcon size={18} />
              </button>
            )}
          </>
        )}

        <ChatInput isProcessing={isProcessing} onSend={handleSend} currentModel={defaultModel} onAbort={handleAbort} />
      </main>
    </div>
  );
}

const EntryRow = memo(function EntryRow({
  entryId,
  defaultModel,
  handleRetry,
  isProcessing,
}: {
  entryId: string;
  defaultModel: string;
  handleRetry: (id: string, text?: string) => void;
  isProcessing: boolean;
}) {
  const entry = useSelector((state: RootState) => selectEntryById(state, entryId));
  if (!entry) return null;

  if (entry.type === 'user') {
    return <UserRow entry={entry} modelName={defaultModel} onRetry={handleRetry} isProcessing={isProcessing} />;
  } else {
    return <AgentRow entry={entry} />;
  }
});

export default App;
