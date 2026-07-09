import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionMeta } from './projectSlice';

async function loadModules() {
  vi.resetModules();
  vi.stubGlobal('localStorage', {
    getItem: vi.fn(() => null),
    setItem: vi.fn(),
    removeItem: vi.fn(),
  });
  return import('./projectSlice');
}

function makeSession(id: string, updatedAt: string): SessionMeta {
  return {
    id,
    title: id,
    summary: '',
    start_time: updatedAt,
    end_time: null,
    message_count: 0,
    cwd: '/tmp',
    model_used: 'gpt',
    tags: [],
    archived: false,
    parent_session_id: null,
    session_type: 'main',
    process_time_ms: 0,
    thought_time_ms: 0,
    created_at: updatedAt,
    updated_at: updatedAt,
  };
}

beforeEach(() => {
  vi.unstubAllGlobals();
});

describe('projectSlice session activity', () => {
  it('sortSessionsByActivity orders by updated_at with id tie-breaker', async () => {
    const { sortSessionsByActivity } = await loadModules();
    const sessions = [
      makeSession('b', '2026-01-01T12:00:00Z'),
      makeSession('a', '2026-01-01T12:00:00Z'),
      makeSession('c', '2026-01-02T12:00:00Z'),
    ];
    const sorted = sortSessionsByActivity(sessions);
    expect(sorted.map((s) => s.id)).toEqual(['c', 'b', 'a']);
  });

  it('touchSessionActivity bumps updated_at and reorders within project', async () => {
    const { default: reducer, touchSessionActivity } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = {
      ...state,
      sessions: {
        proj: [
          makeSession('old', '2026-01-01T10:00:00Z'),
          makeSession('active', '2026-01-01T09:00:00Z'),
        ],
      },
    };
    state = reducer(state, touchSessionActivity({
      sessionId: 'active',
      updatedAt: '2026-01-03T10:00:00Z',
    }));
    expect(state.sessions.proj[0].id).toBe('active');
    expect(state.sessions.proj[0].updated_at).toBe('2026-01-03T10:00:00Z');
  });

  it('saveSessionMessages.fulfilled patches updated_at and ignores stale generation', async () => {
    const mod = await loadModules();
    const { default: reducer, saveSessionMessages, __testSetSaveGeneration } = mod;
    let state = reducer(undefined, { type: '@@INIT' });
    state = {
      ...state,
      sessions: {
        proj: [makeSession('s1', '2026-01-01T10:00:00Z')],
      },
    };

    state = reducer(state, {
      type: saveSessionMessages.fulfilled.type,
      payload: {
        sessionId: 's1',
        messageCount: 3,
        updated_at: '2026-01-02T10:00:00Z',
        generation: 2,
      },
    });
    expect(state.sessions.proj[0].message_count).toBe(3);
    expect(state.sessions.proj[0].updated_at).toBe('2026-01-02T10:00:00Z');

    __testSetSaveGeneration('s1', 3);
    state = reducer(state, {
      type: saveSessionMessages.fulfilled.type,
      payload: {
        sessionId: 's1',
        messageCount: 1,
        updated_at: '2026-01-01T08:00:00Z',
        generation: 2,
      },
    });
    expect(state.sessions.proj[0].message_count).toBe(3);
    expect(state.sessions.proj[0].updated_at).toBe('2026-01-02T10:00:00Z');
  });

  it('findProjectIdForSession resolves owner project', async () => {
    const { findProjectIdForSession } = await loadModules();
    const sessions = {
      projA: [makeSession('s1', '2026-01-01T10:00:00Z')],
      projB: [makeSession('s2', '2026-01-01T10:00:00Z')],
    };
    expect(findProjectIdForSession(sessions, 's2')).toBe('projB');
    expect(findProjectIdForSession(sessions, 'missing')).toBeNull();
  });
});
