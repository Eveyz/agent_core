import { describe, it, expect, beforeEach } from 'vitest';
import {
  getSessionDraft,
  setSessionDraft,
  clearSessionDraft,
  _resetSessionDraftsForTests,
} from './sessionDraftStore';

describe('sessionDraftStore', () => {
  beforeEach(() => {
    _resetSessionDraftsForTests();
  });

  it('stores and restores drafts per session', () => {
    setSessionDraft('s1', 'hello from s1');
    setSessionDraft('s2', 'draft s2');

    expect(getSessionDraft('s1')).toBe('hello from s1');
    expect(getSessionDraft('s2')).toBe('draft s2');
    expect(getSessionDraft('missing')).toBe('');
  });

  it('deletes empty drafts instead of keeping blank strings', () => {
    setSessionDraft('s1', 'temp');
    setSessionDraft('s1', '');
    expect(getSessionDraft('s1')).toBe('');
  });

  it('clearSessionDraft removes a cached draft', () => {
    setSessionDraft('s1', 'to send');
    clearSessionDraft('s1');
    expect(getSessionDraft('s1')).toBe('');
  });
});
