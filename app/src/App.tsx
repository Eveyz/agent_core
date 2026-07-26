import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSelector, useStore, shallowEqual } from 'react-redux';
import { RootState } from './store';
import {
  userMessageSent,
  agentAborted,
  sendFailed,
  runIdSet,
  selectEntryIds,
  selectPendingApprovalCount,
  selectHasActivePendingApproval,
  selectActivePendingApproval,
  pendingApprovalEqual,
  selectHasActivePendingClarification,
  selectActivePendingClarification,
  pendingClarificationEqual,
  selectSubagentById,
  selectViewingSubagentPath,
  selectIsResumingActive,
  steerMessageQueued,
  steerMessageCancelled,
  btwAsked,
  goalCleared,
  plansHydrated,
} from './features/chat/chatSlice';
import { openSettings, fetchConfig } from './features/settings/settingsSlice';
import { fetchAgents } from './features/agents/agentSlice';
import {
  fetchProjects,
  createSession,
  renameSession,
  setActiveSession,
} from './features/project/projectSlice';
import { useAppDispatch } from './hooks/useAppDispatch';
import { useAgentEventListener } from './hooks/useAgentEventListener';
import { usePreviewEvents } from './hooks/usePreviewEvents';
import { usePreviewToolHandler } from './hooks/usePreviewToolHandler';
import { useAutoScroll } from './hooks/useAutoScroll';
import { useThemeEffect } from './hooks/useThemeEffect';
import { useKeyboardShortcuts } from './hooks/useKeyboardShortcuts';
import { useWindowShow } from './hooks/useWindowShow';
import { useSessionLoader } from './hooks/useSessionLoader';
import { useVisibilityResync } from './hooks/useVisibilityResync';

import { Sidebar } from './components/layout/Sidebar';
import { CosmicBackground } from './components/layout/CosmicBackground';
import { EmptyState } from './components/chat/EmptyState';
import type { SendPayload } from './components/chat/imageAttachments';
import type { WorkflowLibraryEntry } from './features/workflow/types';
import { ChatInput } from './components/chat/ChatInput';
import { setSessionDraft } from './hooks/sessionDraftStore';
import ApprovalBlockUI from './components/chat/ApprovalBlockUI';
import ClarificationOverlay from './components/chat/ClarificationOverlay';
import SettingsModal from './components/settings/SettingsModal';
import { CustomTitleBar } from './components/layout/CustomTitleBar';
import { AppHeader } from './components/layout/AppHeader';
import { ChatArea } from './components/layout/ChatArea';
import { SubagentDetailPage } from './components/chat/SubagentDetailPage';
import { getActiveSessionTitle } from './utils/chatUtils';
import { useResizableSidebar } from './hooks/useResizableSidebar';
import { RightSidebar } from './components/layout/RightSidebar';
import { AgentsPage } from './components/agents/AgentsPage';
import React, { Suspense } from 'react';
const WorkflowEditor = React.lazy(() =>
  import('./components/workflow/WorkflowEditor').then(m => ({ default: m.WorkflowEditor }))
);
import type { AppView } from './components/layout/Sidebar';
import LoaderIcon from 'lucide-react/dist/esm/icons/loader.mjs';

import './App.css';

const SessionLoader = () => (
  <div className="empty-state">
    <div className="star-field" />
    <div className="cosmic-glow cosmic-glow-1" />
    <div className="cosmic-glow cosmic-glow-2" />
    <div className="empty-state-content">
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
      <h1 className="empty-state-title" style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
        <LoaderIcon className="animate-spin" size={24} style={{ color: 'var(--accent)' }} />
        Resuming Session...
      </h1>
      <p className="empty-state-subtitle">Restoring conversation history and environment context.</p>
    </div>
  </div>
);

function App() {
  const dispatch = useAppDispatch();
  const store = useStore<RootState>();


  const entryIds = useSelector(selectEntryIds);
  const entriesLength = useSelector((state: RootState) => {
    const sid = state.project.activeSessionId;
    return sid ? (state.chat.entries[sid]?.length ?? 0) : 0;
  });
  const isProcessing = useSelector((state: RootState) => {
    const sid = state.project.activeSessionId;
    return sid ? !!state.chat.processing[sid] : false;
  });
  const isResuming = useSelector(selectIsResumingActive);
  const defaultModel = useSelector((state: RootState) => state.settings.config?.default_model || '');
  const appearance = useSelector((state: RootState) => state.settings.appearance);
  
  const activeProjectId = useSelector((state: RootState) => state.project.activeProjectId);
  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);
  const projects = useSelector((state: RootState) => state.project.projects);
  const sessionTitle = useSelector((state: RootState) => getActiveSessionTitle(state.project));

  const [activeTab, setActiveTab] = useState<'code' | 'write'>('code');
  const [activeView, setActiveView] = useState<AppView>('chat');
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [rightSidebarExpanded, setRightSidebarExpanded] = useState(false);

  const continueWorkflowInChat = useCallback((entry: WorkflowLibraryEntry) => {
    const text = `/workflow edit ${entry.workflow_id}`;
    if (activeSessionId) {
      setSessionDraft(activeSessionId, text);
    }
    setActiveView('chat');
    window.dispatchEvent(new CustomEvent('agverse:composer-prefill', { detail: text }));
  }, [activeSessionId]);

  // When a session is selected (via sidebar click), auto-switch to chat view.
  // This handles the case where the user was on Agents/Workflows view and
  // then clicked a session — the chat should take over the main area.
  useEffect(() => {
    if (activeSessionId) {
      setActiveView("chat");
    }
  }, [activeSessionId]);

  useEffect(() => {
    const handleOpenRightSidebar = () => {
      setRightSidebarExpanded(true);
    };
    window.addEventListener('open-right-sidebar', handleOpenRightSidebar);
    return () => window.removeEventListener('open-right-sidebar', handleOpenRightSidebar);
  }, []);

  const { sidebarRef: leftSidebarRef, onMouseDown: startLeftDrag } = useResizableSidebar(260, 200, 600, 'left');
  const { sidebarRef: rightSidebarRef, onMouseDown: startRightDrag } = useResizableSidebar(550, 300, 1200, 'right');

  const { scrollRef, contentRef, scrollToBottom, isAtBottom } = useAutoScroll<HTMLDivElement, HTMLDivElement>({
    deps: [entriesLength, activeSessionId],
    isProcessing,
  });

  useAgentEventListener();
  usePreviewEvents();
  usePreviewToolHandler();

  useThemeEffect(appearance);

  const runId = useSelector((state: RootState) => {
    const sid = state.project.activeSessionId;
    return sid ? (state.chat.runId[sid] ?? null) : null;
  });
  
  useKeyboardShortcuts({ isProcessing, runId, sessionId: activeSessionId });
  useWindowShow();
  useVisibilityResync();

  const projectsLoaded = projects.length > 0;
  useSessionLoader({
    projectsLoaded,
    activeProjectId,
    activeSessionId,
    scrollToBottom,
  });

  // Track pending approvals — scroll to bottom when a new one appears
  const pendingApprovalCount = useSelector(selectPendingApprovalCount);
  const hasActivePendingApproval = useSelector(selectHasActivePendingApproval);
  const activePendingApproval = useSelector(selectActivePendingApproval, pendingApprovalEqual);
  const hasActivePendingClarification = useSelector(selectHasActivePendingClarification);
  const activePendingClarification = useSelector(
    selectActivePendingClarification,
    pendingClarificationEqual
  );
  const pendingSteerCount = useSelector((state: RootState) => {
    const sid = state.project.activeSessionId;
    return sid ? (state.chat.steerQueue[sid]?.filter((s) => s.status === 'pending').length ?? 0) : 0;
  });
  const viewingSubagentPath = useSelector(selectViewingSubagentPath, shallowEqual);

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

  useEffect(() => {
    if (hasActivePendingClarification) {
      scrollToBottom();
    }
  }, [hasActivePendingClarification, scrollToBottom]);

  // When a session is opened/switched, force-stick to the bottom. This pins the
  // view through the async reflows (markdown, code blocks, tool calls) that happen
  // as a loaded session renders, so the latest message — and its copy icon — lands
  // fully in view. Covers both sync cache restore and async backend resume.
  // When returning to the main chat from a subagent detail view, scroll to bottom.
  const prevViewingSubagentPathLength = useRef(0);
  useEffect(() => {
    if (prevViewingSubagentPathLength.current > 0 && viewingSubagentPath.length === 0) {
      setTimeout(() => scrollToBottom('auto'), 100);
    }
    prevViewingSubagentPathLength.current = viewingSubagentPath.length;
  }, [viewingSubagentPath.length, scrollToBottom]);

  const prevSessionIdRef = useRef<string | null | undefined>(activeSessionId);
  useEffect(() => {
    if (prevSessionIdRef.current !== activeSessionId) {
      prevSessionIdRef.current = activeSessionId;
      if (activeSessionId) {
        // Delay scroll to ensure content has rendered
        setTimeout(() => scrollToBottom('auto'), 100);
      }
    }
  }, [activeSessionId, scrollToBottom]);

  const handleAbort = useCallback(() => {
    if (!activeSessionId) return;
    dispatch(agentAborted({ sessionId: activeSessionId }));
    invoke('abort_agent', { runId }).catch((e) => console.error('Failed to abort agent:', e));
  }, [dispatch, runId, activeSessionId]);

  const handleSteer = useCallback(async (message: string) => {
    if (!runId || !message.trim() || !activeSessionId) return;
    const steerId = crypto.randomUUID();
    dispatch(steerMessageQueued({ sessionId: activeSessionId, steerId, text: message.trim() }));
    try {
      await invoke('steer_run', { runId, steerId, message: message.trim() });
    } catch (e) {
      console.error('Failed to steer run:', e);
      dispatch(steerMessageCancelled({ sessionId: activeSessionId, steerId }));
    }
  }, [runId, dispatch, activeSessionId]);

  const handleBtwQuery = useCallback(async (question: string) => {
    if (!activeSessionId) return;
    try {
      const id = await invoke<string>('btw_query', { sessionId: activeSessionId, question });
      dispatch(btwAsked({ sessionId: activeSessionId, id, question }));
    } catch (e) {
      console.error('btw_query failed:', e);
    }
  }, [activeSessionId, dispatch]);

  useEffect(() => {
    dispatch(fetchConfig());
    dispatch(fetchProjects());
    dispatch(fetchAgents());
  }, [dispatch]);

  const handleSend = useCallback(
    async (payload: SendPayload | string) => {
      const msg = typeof payload === 'string' ? payload : payload.text;
      const pendingImages = typeof payload === 'string' ? undefined : payload.images;
      const agentMentions = typeof payload === 'string' ? undefined : payload.agentMentions;
      const workflowMentions = typeof payload === 'string' ? undefined : payload.workflowMentions;
      const trimmed = msg.trim();
      const isGoalClear =
        trimmed === '/goal clear' ||
        trimmed === '/goal stop' ||
        trimmed === '/goal cancel' ||
        trimmed === '/goal off';

      // /goal clear: drop session pin without starting a Run.
      if (isGoalClear) {
        const sessionId = activeSessionId;
        if (!sessionId) return;
        dispatch(goalCleared({ sessionId }));
        try {
          await invoke('clear_session_goal', { sessionId });
        } catch (e) {
          console.error('Failed to clear session goal:', e);
        }
        return;
      }

      const isPlanClear =
        trimmed === '/plan clear' ||
        trimmed === '/plan cancel' ||
        trimmed === '/plan stop';
      if (isPlanClear) {
        const sessionId = activeSessionId;
        if (!sessionId) return;
        try {
          const dto = await invoke<{
            items: { id: string; description: string; status: string }[];
            parked: { id: string; title: string; completed: number; total: number; updated_at: string }[];
            active_plan_id?: string | null;
            active_plan_title?: string | null;
          }>('clear_session_plans', { sessionId });
          dispatch(
            plansHydrated({
              sessionId,
              items: (dto.items ?? []) as import('./features/chat/types').TodoItem[],
              parked: dto.parked ?? [],
              activePlanId: dto.active_plan_id ?? null,
              activePlanTitle: dto.active_plan_title ?? null,
            }),
          );
        } catch (e) {
          console.error('Failed to clear session plans:', e);
        }
        return;
      }

      if (trimmed === '/plan park' || trimmed === '/plan pause') {
        const sessionId = activeSessionId;
        if (!sessionId) return;
        try {
          const dto = await invoke<{
            items: { id: string; description: string; status: string }[];
            parked: { id: string; title: string; completed: number; total: number; updated_at: string }[];
            active_plan_id?: string | null;
            active_plan_title?: string | null;
          }>('cancel_session_plan', { sessionId, planId: null });
          dispatch(
            plansHydrated({
              sessionId,
              items: (dto.items ?? []) as import('./features/chat/types').TodoItem[],
              parked: dto.parked ?? [],
              activePlanId: dto.active_plan_id ?? null,
              activePlanTitle: dto.active_plan_title ?? null,
            }),
          );
        } catch (e) {
          console.error('Failed to park session plan:', e);
        }
        return;
      }

      if (!trimmed && !(pendingImages && pendingImages.length > 0)) return;

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

      if (store.getState().project.activeSessionId !== sessionId) {
        dispatch(setActiveSession(sessionId));
      }

      const optimisticImages = pendingImages?.map((img) => ({
        id: img.id,
        previewUrl: img.previewUrl,
        mimeType: img.mimeType,
      }));

      dispatch(userMessageSent({
        text: msg,
        model: defaultModel,
        sessionId,
        images: optimisticImages,
      }));
      scrollToBottom('auto');

      const shouldRename = isNewSession || sessionTitle === 'New Session' || sessionTitle === '';
      if (shouldRename && sessionId && activeProjectId) {
        const previewSource = trimmed || (pendingImages?.length ? 'Image' : '');
        const preview = previewSource.slice(0, 30) + (previewSource.length > 30 ? '...' : '');
        if (preview) {
          dispatch(renameSession({ sessionId, projectId: activeProjectId, newTitle: preview }));
        }
      }

      try {
        const result = await invoke<{
          run_id: string;
          prompt_id?: string | null;
          images?: { path: string; mime_type: string; url?: string; sha256?: string }[];
        }>(
          'send_message',
          {
            message: msg,
            sessionId,
            model: defaultModel,
            images: pendingImages?.map((img) => ({
              mime_type: img.mimeType,
              data_base64: img.dataBase64,
            })),
            agentMentions,
            workflowMentions,
          },
        );
        dispatch(runIdSet({
          runId: result.run_id,
          sessionId,
          promptId: result.prompt_id ?? undefined,
          images: result.images?.map((img, idx) => ({
            id: optimisticImages?.[idx]?.id,
            previewUrl: optimisticImages?.[idx]?.previewUrl,
            mimeType: img.mime_type,
            path: img.path,
            url: img.url,
            sha256: img.sha256,
          })),
        }));
      } catch (e) {
        console.error('Invoke error:', e);
        dispatch(sendFailed({ sessionId, error: String(e) }));
      }
    },
    [dispatch, activeProjectId, activeSessionId, sessionTitle, scrollToBottom, defaultModel, store]
  );

  const handleRetry = useCallback(
    async (entryId: string, editedText?: string) => {
      const entries = store.getState().chat.entries[activeSessionId ?? ''] ?? [];
      const entry = entries.find((e) => e.id === entryId);
      if (!entry) return;
      const msg = editedText ?? entry.text ?? '';
      const entryImages = entry.images ?? [];
      if ((!msg.trim() && entryImages.length === 0) || !activeSessionId) return;

      if (store.getState().project.activeSessionId !== activeSessionId) {
        dispatch(setActiveSession(activeSessionId));
      }

      // Use the currently selected model (same as send), not the original prompt's.
      // That way switching models in the picker and hitting retry actually switches.
      const model = defaultModel;
      // Append-only: keep prior turns and send the same (or edited) text as a new prompt.
      // Reuse persisted attachment refs (path / agverse:// url) so images are included.
      const optimisticImages = entryImages.map((img) => ({
        id: img.id,
        previewUrl: img.previewUrl,
        mimeType: img.mimeType,
        path: img.path,
        url: img.url,
        sha256: img.sha256,
      }));
      dispatch(userMessageSent({
        text: msg,
        model,
        sessionId: activeSessionId,
        images: optimisticImages.length ? optimisticImages : undefined,
      }));
      scrollToBottom('auto');

      try {
        const result = await invoke<{
          run_id: string;
          prompt_id?: string | null;
          images?: { path: string; mime_type: string; url?: string; sha256?: string }[];
        }>(
          'send_message',
          {
            message: msg,
            sessionId: activeSessionId,
            model,
            images: entryImages.length
              ? entryImages.map((img) => ({
                  mime_type: img.mimeType,
                  path: img.path,
                  url: img.url,
                }))
              : undefined,
          },
        );
        dispatch(runIdSet({
          runId: result.run_id,
          sessionId: activeSessionId,
          promptId: result.prompt_id ?? undefined,
          images: result.images?.map((img, idx) => ({
            id: optimisticImages[idx]?.id,
            previewUrl: optimisticImages[idx]?.previewUrl,
            mimeType: img.mime_type,
            path: img.path,
            url: img.url,
            sha256: img.sha256,
          })),
        }));
      } catch (e) {
        console.error('Retry invoke error:', e);
        dispatch(sendFailed({ sessionId: activeSessionId, error: String(e) }));
      }
    },
    [dispatch, activeSessionId, store, scrollToBottom, defaultModel]
  );

  const handleOpenSettings = useCallback(() => {
    dispatch(openSettings());
  }, [dispatch]);

  return (
    <div className="app-container">
      <div className="app-body">
        <div 
          ref={leftSidebarRef}
          className={`sidebar-column${sidebarCollapsed ? ' sidebar-collapsed' : ''}`}
          style={sidebarCollapsed ? undefined : { width: 260 }}
        >
          <CustomTitleBar
            sidebarCollapsed={sidebarCollapsed}
            onToggleSidebar={() => setSidebarCollapsed(!sidebarCollapsed)}
          />
          <Sidebar
            activeTab={activeTab}
            onTabChange={setActiveTab}
            onOpenSettings={handleOpenSettings}
            collapsed={sidebarCollapsed}
            activeView={activeView}
            onNavigate={setActiveView}
          />
        </div>
        {!sidebarCollapsed && <div className="resizer-handle" onMouseDown={startLeftDrag} />}
        <SettingsModal />

        <main className="main-area">
          {activeView === "chat" && <CosmicBackground />}
          <AppHeader
            sessionTitle={sessionTitle}
            viewingSubagentPath={viewingSubagentPath}
            activeSessionId={activeSessionId}
            activeProjectId={activeProjectId}
            sidebarCollapsed={sidebarCollapsed}
            onExpandSidebar={() => setSidebarCollapsed(false)}
            rightSidebarExpanded={rightSidebarExpanded}
            onToggleRightSidebar={activeView === "chat" ? () => setRightSidebarExpanded(!rightSidebarExpanded) : undefined}
            hideTitle={activeView !== "chat"}
          />

          {activeView === "agents" ? (
            <AgentsPage />
          ) : activeView === "workflows" ? (
            <Suspense fallback={<div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', height: '100%', color: 'var(--text-muted)' }}>Loading workflow editor...</div>}>
              <WorkflowEditor onContinueInChat={continueWorkflowInChat} />
            </Suspense>
          ) : viewingSubagentPath.length > 0 && activeSubagent ? (
            <SubagentDetailPage
              subagent={activeSubagent}
              isProcessing={isProcessing}
              defaultModel={defaultModel}
            />
          ) : isResuming && entriesLength === 0 ? (
            <SessionLoader />
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
              onSend={handleSend}
            />
          )}

          {activeView === "chat" && (
            <div className="chat-input-wrapper-relative" style={{ position: 'relative', width: '100%' }}>
              <ChatInput
                isProcessing={isProcessing}
                onSend={handleSend}
                currentModel={defaultModel}
                onAbort={handleAbort}
                onSteer={handleSteer}
                onBtwQuery={handleBtwQuery}
                pendingSteerCount={pendingSteerCount}
                disabled={
                  viewingSubagentPath.length > 0 ||
                  isResuming ||
                  hasActivePendingApproval ||
                  hasActivePendingClarification
                }
                disabledMessage={
                  isResuming
                    ? "Resuming session..."
                    : hasActivePendingApproval
                    ? "Awaiting approval..."
                    : hasActivePendingClarification
                    ? "Awaiting your clarification..."
                    : "the input chat is disabled for the subagent"
                }
              />
              {activePendingApproval && (
                <div className="approval-overlay-container">
                  <ApprovalBlockUI block={activePendingApproval} isOverlay={true} />
                </div>
              )}
              {!activePendingApproval && activePendingClarification && (
                <div className="approval-overlay-container">
                  <ClarificationOverlay block={activePendingClarification} isOverlay={true} />
                </div>
              )}
            </div>
          )}
        </main>
        
        {activeView === "chat" && rightSidebarExpanded && <div className="resizer-handle" onMouseDown={startRightDrag} />}
        {activeView === "chat" && (
          <RightSidebar 
            sidebarRef={rightSidebarRef} 
            isExpanded={rightSidebarExpanded} 
            onToggle={() => setRightSidebarExpanded(false)} 
          />
        )}
      </div>
    </div>
  );
}

export default App;
