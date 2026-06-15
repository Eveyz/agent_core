import { useState, useEffect, useRef, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useDispatch, useSelector } from 'react-redux';
import BoxIcon from 'lucide-react/dist/esm/icons/box.mjs';
import MessageSquareIcon from 'lucide-react/dist/esm/icons/message-square.mjs';
import TerminalSquareIcon from 'lucide-react/dist/esm/icons/terminal-square.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import Maximize2Icon from 'lucide-react/dist/esm/icons/maximize-2.mjs';
import { RootState } from './store';
import { agentEventReceived, userMessageSent } from './features/chat/chatSlice';
import { openSettings, fetchConfig } from './features/settings/settingsSlice';
import { Sidebar } from './components/layout/Sidebar';
import { CosmicBackground } from './components/layout/CosmicBackground';
import { EmptyState } from './components/chat/EmptyState';
import { UserRow } from './components/chat/UserRow';
import { AgentRow } from './components/chat/AgentRow';
import { ChatInput } from './components/chat/ChatInput';
import SettingsModal from './components/settings/SettingsModal';
import './App.css';

function App() {
  const dispatch = useDispatch();
  const entries = useSelector((state: RootState) => state.chat.entries);
  const isProcessing = useSelector((state: RootState) => state.chat.isProcessing);
  const defaultModel = useSelector((state: RootState) => state.settings.config?.default_model || '');
  const [activeTab, setActiveTab] = useState<'code' | 'write'>('code');
  const messagesEndRef = useRef<HTMLDivElement>(null);

  const handleOpenSettings = useCallback(() => {
    dispatch(openSettings());
  }, [dispatch]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [entries.length, isProcessing]);

  useEffect(() => {
    dispatch(fetchConfig() as any);
  }, [dispatch]);

  useEffect(() => {
    let isMounted = true;
    let unlistenFn: (() => void) | undefined;
    const setupListener = async () => {
      const fn = await listen<any>('agent-event', (event) => {
        dispatch(agentEventReceived(event.payload));
      });
      if (!isMounted) {
        fn();
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
      <Sidebar activeTab={activeTab} onTabChange={setActiveTab} onOpenSettings={handleOpenSettings} />
      <SettingsModal />

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
