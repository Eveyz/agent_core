import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAppDispatch } from './useAppDispatch';
import { agentAborted } from '../features/chat/chatSlice';

interface UseKeyboardShortcutsProps {
  isProcessing: boolean;
  runId: string | null;
}

export function useKeyboardShortcuts({ isProcessing, runId }: UseKeyboardShortcutsProps) {
  const dispatch = useAppDispatch();

  useEffect(() => {
    if (!isProcessing) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        dispatch(agentAborted());
        invoke('abort_agent', { runId }).catch((e) => console.error('Failed to abort agent:', e));
      }
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [isProcessing, dispatch, runId]);
}
