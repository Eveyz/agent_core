import { useEffect, useRef } from 'react';
import { useAppSelector } from './useAppDispatch';
import { useSaveSession } from './useSaveSession';

interface AutoSaveParams {
  activeSessionId: string | null;
  activeProjectPath: string | null;
  defaultModel: string;
}

export function useAutoSaveSession({
  activeSessionId,
  activeProjectPath,
  defaultModel,
}: AutoSaveParams): void {
  const saveSession = useSaveSession();

  const isProcessing = useAppSelector((state) => state.chat.isProcessing);
  const resumedFromBackend = useAppSelector((state) => state.chat._resumedFromBackend);

  const lastAgentEndRef = useRef(false);

  useEffect(() => {
    if (isProcessing) {
      lastAgentEndRef.current = false;
      return;
    }

    if (resumedFromBackend) return;

    if (!lastAgentEndRef.current) {
      lastAgentEndRef.current = true;
      saveSession({ activeSessionId, activeProjectPath, defaultModel, skipIfResumed: true });
    }
  }, [isProcessing, resumedFromBackend, activeSessionId, activeProjectPath, defaultModel, saveSession]);
}
