import { useState, useEffect, useRef, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useDispatch, useSelector } from 'react-redux';
import BoxIcon from 'lucide-react/dist/esm/icons/box.mjs';
import MessageSquareIcon from 'lucide-react/dist/esm/icons/message-square.mjs';
import TerminalSquareIcon from 'lucide-react/dist/esm/icons/terminal-square.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import Maximize2Icon from 'lucide-react/dist/esm/icons/maximize-2.mjs';
import PencilIcon from 'lucide-react/dist/esm/icons/pencil.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import { RootState } from './store';
import { agentEventReceived, userMessageSent, entriesToMessages } from './features/chat/chatSlice';
import { openSettings, fetchConfig } from './features/settings/settingsSlice';
import { fetchProjects, fetchProjectSessions, createSession, saveSessionMessages, renameSession, resumeSession, setActiveSession } from './features/project/projectSlice';
import { Sidebar } from './components/layout/Sidebar';
import { CosmicBackground } from './components/layout/CosmicBackground';
import { EmptyState } from './components/chat/EmptyState';
import { UserRow } from './components/chat/UserRow';
import { AgentRow } from './components/chat/AgentRow';
import { ChatInput } from './components/chat/ChatInput';
import SettingsModal from './components/settings/SettingsModal';
import './App.css';

// ── Helpers ──────────────────────────────────────────────────────────

function getActiveSessionTitle(projectState: RootState['project']): string {
  if (!projectState.activeSessionId || !projectState.activeProjectId) return '';
  const list = projectState.sessions[projectState.activeProjectId] ?? [];
  const s = list.find((s) => s.id === projectState.activeSessionId);
  return s?.title ?? '';
}

function App() {
  const dispatch = useDispatch();
  const entries = useSelector((state: RootState) => state.chat.entries);
  const isProcessing = useSelector((state: RootState) => state.chat.isProcessing);
  const defaultModel = useSelector((state: RootState) => state.settings.config?.default_model || '');
  const projectState = useSelector((state: RootState) => state.project);
  const { activeProjectId, activeSessionId, projects } = projectState;
  const activeProject = projects.find((p) => p.id === activeProjectId);
  const [activeTab, setActiveTab] = useState<'code' | 'write'>('code');
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const lastAgentEndRef = useRef(false);

  const handleOpenSettings = useCallback(() => {
    dispatch(openSettings());
  }, [dispatch]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [entries.length, isProcessing]);

  // Load config and projects on mount
  useEffect(() => {
    dispatch(fetchConfig() as any);
    dispatch(fetchProjects() as any);
  }, [dispatch]);

  // Restore last active session after projects are loaded
  const projectsLoaded = projects.length > 0;
  useEffect(() => {
    if (!projectsLoaded || !activeProjectId || !activeSessionId) return;
    dispatch(fetchProjectSessions(activeProjectId) as any);
    dispatch(resumeSession(activeSessionId) as any).then((result: any) => {
      if (!resumeSession.fulfilled.match(result)) {
        // Session no longer exists, clear it
        dispatch(setActiveSession(null));
      }
    });
  }, [projectsLoaded, dispatch]);

  // Listen for agent events
  useEffect(() => {
    let isMounted = true;
    let unlistenFn: (() => void) | undefined;
    const setupListener = async () => {
      const fn = await listen<any>('agent-event', (event) => {
        dispatch(agentEventReceived(event.payload));
      });
      if (!isMounted) { fn(); } else { unlistenFn = fn; }
    };
    setupListener();
    return () => { isMounted = false; if (unlistenFn) unlistenFn(); };
  }, [dispatch]);

  // Save session messages after AgentEnd
  useEffect(() => {
    if (isProcessing) {
      lastAgentEndRef.current = false;
      return;
    }
    // Detect transition from processing → done (AgentEnd just happened)
    if (!lastAgentEndRef.current && entries.length > 0) {
      lastAgentEndRef.current = true;
      if (activeSessionId && activeProject) {
        const msgs = entriesToMessages(entries);
        if (msgs.length > 0) {
          dispatch(saveSessionMessages({
            sessionId: activeSessionId,
            messages: msgs,
            cwd: activeProject.path,
            modelUsed: defaultModel,
          }) as any);
        }
      }
    }
  }, [isProcessing, entries, activeSessionId, activeProject, defaultModel, dispatch]);

  const sessionTitle = getActiveSessionTitle(projectState);

  const handleSend = useCallback(async (msg: string) => {
    // Auto-create session if none active
    let sessionId = activeSessionId;
    let isNewSession = false;
    if (!sessionId) {
      if (!activeProjectId) {
        console.error('No active project to create session in');
        return;
      }
      try {
        const result = await dispatch(createSession(activeProjectId) as any);
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

    // Auto-rename "New Session" to first user message preview
    const shouldRename = isNewSession || sessionTitle === 'New Session' || sessionTitle === '';
    if (shouldRename && sessionId && activeProjectId) {
      const preview = msg.trim().slice(0, 30) + (msg.trim().length > 30 ? '...' : '');
      dispatch(renameSession({ sessionId, projectId: activeProjectId, newTitle: preview }) as any);
    }

    try {
      await invoke('send_message', { message: msg });
    } catch (e) {
      console.error('Invoke error:', e);
      dispatch(agentEventReceived({ Error: String(e) }));
    }
  }, [dispatch, activeProjectId, activeSessionId, sessionTitle]);

  const [isEditingTitle, setIsEditingTitle] = useState(false);
  const [titleEditValue, setTitleEditValue] = useState('');

  const startEditingTitle = useCallback(() => {
    setTitleEditValue(sessionTitle || 'New Session');
    setIsEditingTitle(true);
  }, [sessionTitle]);

  const commitTitleEdit = useCallback(() => {
    const trimmed = titleEditValue.trim();
    if (trimmed && activeSessionId && activeProjectId) {
      dispatch(renameSession({ sessionId: activeSessionId, projectId: activeProjectId, newTitle: trimmed }) as any);
    }
    setIsEditingTitle(false);
  }, [dispatch, titleEditValue, activeSessionId, activeProjectId]);

  const cancelTitleEdit = useCallback(() => {
    setIsEditingTitle(false);
  }, []);

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
                <span className="header-session-name">
                  {sessionTitle || 'New Session'}
                </span>
                <button
                  className="icon-btn header-edit-btn"
                  onClick={startEditingTitle}
                  title="Edit session title"
                >
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

        {entries.length === 0 ? (
          <EmptyState onSend={handleSend} />
        ) : (
          <div className="chat-history">
            {entries.map((entry) =>
              entry.type === 'user' ? (
                <UserRow key={entry.id} entry={entry} modelName={defaultModel} />
              ) : (
                <AgentRow key={entry.id} entry={entry} />
              )
            )}
            <div ref={messagesEndRef} />
          </div>
        )}

        <ChatInput
          isProcessing={isProcessing}
          onSend={handleSend}
          entriesLength={entries.length}
          currentModel={defaultModel}
        />
      </main>
    </div>
  );
}

export default App;
