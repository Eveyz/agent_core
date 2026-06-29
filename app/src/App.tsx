import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSelector, useStore, shallowEqual } from 'react-redux';
import { RootState } from './store';
import {
  agentEventReceived,
  userMessageSent,
  agentAborted,
  runIdSet,
  selectEntryIds,
  selectPendingApprovalCount,
  selectSubagentById,
} from './features/chat/chatSlice';
import { openSettings, fetchConfig } from './features/settings/settingsSlice';
import {
  fetchProjects,
  createSession,
  renameSession,
} from './features/project/projectSlice';
import { useAppDispatch } from './hooks/useAppDispatch';
import { useAgentEventListener } from './hooks/useAgentEventListener';
import { useAutoSaveSession } from './hooks/useAutoSaveSession';
import { useAutoScroll } from './hooks/useAutoScroll';
import { useThemeEffect } from './hooks/useThemeEffect';
import { useKeyboardShortcuts } from './hooks/useKeyboardShortcuts';
import { useWindowShow } from './hooks/useWindowShow';
import { useSessionLoader } from './hooks/useSessionLoader';

import { Sidebar } from './components/layout/Sidebar';
import { CosmicBackground } from './components/layout/CosmicBackground';
import { EmptyState } from './components/chat/EmptyState';
import { ChatInput } from './components/chat/ChatInput';
import SettingsModal from './components/settings/SettingsModal';
import { CustomTitleBar } from './components/layout/CustomTitleBar';
import { AppHeader } from './components/layout/AppHeader';
import { ChatArea } from './components/layout/ChatArea';
import { SubagentDetailPage } from './components/chat/SubagentDetailPage';
import { getActiveSessionTitle } from './utils/chatUtils';

import './App.css';

function App() {
  const dispatch = useAppDispatch();
  const store = useStore<RootState>();

  const entryIds = useSelector(selectEntryIds);
  const entriesLength = useSelector((state: RootState) => state.chat.entries.length);
  const isProcessing = useSelector((state: RootState) => state.chat.isProcessing);
  const defaultModel = useSelector((state: RootState) => state.settings.config?.default_model || '');
  const appearance = useSelector((state: RootState) => state.settings.appearance);
  
  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const sessionTitle = useSelector((state: RootState) => getActiveSessionTitle(state.project));

  const activeProject = projects.find((p) => p.id === activeProjectId);
  const [activeTab, setActiveTab] = useState<'code' | 'write'>('code');
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);

  const { scrollRef, contentRef, scrollToBottom, isAtBottom } = useAutoScroll<HTMLDivElement, HTMLDivElement>({
    deps: [entriesLength, activeSessionId],
    isProcessing,
  });

  useAgentEventListener();

  useAutoSaveSession({
    activeSessionId,
    activeProjectPath: activeProject?.path ?? null,
    defaultModel,
  });

  useThemeEffect(appearance);

  const runId = useSelector((state: RootState) => state.chat.runId);
  
  useKeyboardShortcuts({ isProcessing, runId });
  useWindowShow();

  const projectsLoaded = projects.length > 0;
  useSessionLoader({
    projectsLoaded,
    activeProjectId,
    activeSessionId,
    scrollToBottom,
  });

  // Track pending approvals — scroll to bottom when a new one appears
  const pendingApprovalCount = useSelector(selectPendingApprovalCount);
  const viewingSubagentPath = useSelector((state: RootState) => state.chat.viewingSubagentPath, shallowEqual);

  const activeSubagentId = viewingSubagentPath.length > 0 ? viewingSubagentPath[viewingSubagentPath.length - 1].id : null;
  const activeSubagent = useSelector((state: RootState) =>
    activeSubagentId ? selectSubagentById(state, activeSubagentId) : undefined
  );

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
        // Delay scroll to ensure content has rendered
        setTimeout(() => scrollToBottom(), 100);
      }
    }
  }, [activeSessionId, scrollToBottom]);

  const handleAbort = useCallback(() => {
    dispatch(agentAborted());
    invoke('abort_agent', { runId }).catch((e) => console.error('Failed to abort agent:', e));
  }, [dispatch, runId]);

  const handleSteer = useCallback((message: string) => {
    if (!runId || !message.trim()) return;
    invoke('steer_run', { runId, message }).catch((e) => console.error('Failed to steer run:', e));
  }, [runId]);

  useEffect(() => {
    dispatch(fetchConfig());
    dispatch(fetchProjects());
  }, [dispatch]);

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
        const id = await invoke<string>('send_message', { message: msg, sessionId });
        dispatch(runIdSet(id));
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

      dispatch(userMessageSent(msg));
      scrollToBottom();

      try {
        const id = await invoke<string>('send_message', { message: msg, sessionId: activeSessionId });
        dispatch(runIdSet(id));
      } catch (e) {
        console.error('Retry invoke error:', e);
        dispatch(agentEventReceived({ Error: String(e) }));
      }
    },
    [dispatch, activeSessionId, store, scrollToBottom]
  );

  const handleOpenSettings = useCallback(() => {
    dispatch(openSettings());
  }, [dispatch]);

  return (
    <div className="app-container">
      <div className="app-body">
        <div className={`sidebar-column${sidebarCollapsed ? ' sidebar-collapsed' : ''}`}>
          <CustomTitleBar
            sidebarCollapsed={sidebarCollapsed}
            onToggleSidebar={() => setSidebarCollapsed(!sidebarCollapsed)}
          />
          <Sidebar
            activeTab={activeTab}
            onTabChange={setActiveTab}
            onOpenSettings={handleOpenSettings}
            collapsed={sidebarCollapsed}
          />
        </div>
        <SettingsModal />

        <main className="main-area">
          <CosmicBackground />
          <AppHeader
            sessionTitle={sessionTitle}
            viewingSubagentPath={viewingSubagentPath}
            activeSessionId={activeSessionId}
            activeProjectId={activeProjectId}
            sidebarCollapsed={sidebarCollapsed}
            onExpandSidebar={() => setSidebarCollapsed(false)}
          />

          {viewingSubagentPath.length > 0 && activeSubagent ? (
            <SubagentDetailPage
              subagent={activeSubagent}
              isProcessing={isProcessing}
              defaultModel={defaultModel}
            />
          ) : entriesLength === 0 ? (
            <EmptyState onSend={handleSend} />
          ) : (
            <ChatArea
              entryIds={entryIds}
              defaultModel={defaultModel}
              isProcessing={isProcessing}
              scrollRef={scrollRef}
              contentRef={contentRef}
              isAtBottom={isAtBottom}
              scrollToBottom={scrollToBottom}
              handleRetry={handleRetry}
            />
          )}

          {viewingSubagentPath.length === 0 && (
            <ChatInput isProcessing={isProcessing} onSend={handleSend} currentModel={defaultModel} onAbort={handleAbort} onSteer={handleSteer} />
          )}
        </main>
      </div>
    </div>
  );
}

export default App;
