import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAppDispatch } from './useAppDispatch';
import { agentAborted } from '../features/chat/chatSlice';

interface UseKeyboardShortcutsProps {
  isProcessing: boolean;
  runId: string | null;
  sessionId: string | null;
}

export function useKeyboardShortcuts({ isProcessing, runId, sessionId }: UseKeyboardShortcutsProps) {
  const dispatch = useAppDispatch();

  useEffect(() => {
    if (!isProcessing || !sessionId) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        dispatch(agentAborted({ sessionId }));
        invoke('abort_agent', { runId }).catch((err) => console.error('Failed to abort agent:', err));
      }
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [isProcessing, dispatch, runId, sessionId]);
}
