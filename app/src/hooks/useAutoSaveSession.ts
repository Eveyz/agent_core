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
  const { saveSession } = useSaveSession();

  const isProcessing = useAppSelector((state) => (
    activeSessionId ? !!state.chat.processing[activeSessionId] : false
  ));

  const prevSessionIdRef = useRef(activeSessionId);
  const prevIsProcessingRef = useRef(isProcessing);

  useEffect(() => {
    // Only auto-save if the session has not changed, and the agent's run
    // transitioned from processing (true) to idle (false) (i.e. run finished).
    if (
      prevSessionIdRef.current === activeSessionId &&
      prevIsProcessingRef.current &&
      !isProcessing
    ) {
      saveSession({ activeSessionId, activeProjectPath, defaultModel });
    }
    prevSessionIdRef.current = activeSessionId;
    prevIsProcessingRef.current = isProcessing;
  }, [isProcessing, activeSessionId, activeProjectPath, defaultModel, saveSession]);
}
