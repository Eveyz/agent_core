import { useState, useEffect, useRef, useCallback } from 'react';
import {
  getSessionDraft,
  setSessionDraft,
  clearSessionDraft,
} from './sessionDraftStore';

/**
 * Per-session composer draft. Switching sessions saves the current text and
 * restores the target session's draft (or empty). A brief null session id
 * (new-session creation) keeps the textarea contents until a real id arrives.
 */
export function useSessionDraft(sessionId: string | null) {
  const [input, setInputState] = useState(() =>
    sessionId ? getSessionDraft(sessionId) : '',
  );
  const sessionIdRef = useRef(sessionId);
  const inputRef = useRef(input);
  inputRef.current = input;

  useEffect(() => {
    const prev = sessionIdRef.current;
    const next = sessionId;
    if (prev === next) return;

    if (prev) {
      setSessionDraft(prev, inputRef.current);
    }

    sessionIdRef.current = next;

    // Brief null while creating a session — leave the textarea alone.
    if (!next) return;

    setInputState(getSessionDraft(next));
  }, [sessionId]);

  const setInput = useCallback((value: string | ((prev: string) => string)) => {
    setInputState((prev) => {
      const next = typeof value === 'function' ? value(prev) : value;
      const sid = sessionIdRef.current;
      if (sid) setSessionDraft(sid, next);
      return next;
    });
  }, []);

  const clearDraft = useCallback(() => {
    const sid = sessionIdRef.current;
    if (sid) clearSessionDraft(sid);
    setInputState('');
  }, []);

  return { input, setInput, clearDraft };
}
