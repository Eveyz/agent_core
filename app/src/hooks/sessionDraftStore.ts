/** In-memory drafts keyed by session id. Survives ChatInput unmount (e.g. Agents view). */
const sessionDrafts = new Map<string, string>();

export function getSessionDraft(sessionId: string): string {
  return sessionDrafts.get(sessionId) ?? '';
}

export function setSessionDraft(sessionId: string, text: string): void {
  if (text) sessionDrafts.set(sessionId, text);
  else sessionDrafts.delete(sessionId);
}

export function clearSessionDraft(sessionId: string): void {
  sessionDrafts.delete(sessionId);
}

/** Test helper — clears the module cache between tests. */
export function _resetSessionDraftsForTests() {
  sessionDrafts.clear();
}
