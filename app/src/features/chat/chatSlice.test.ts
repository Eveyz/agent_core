import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AnyAction, Reducer } from '@reduxjs/toolkit';
import type { ChatState } from './types';

async function loadModules() {
  vi.resetModules();
  vi.stubGlobal('localStorage', {
    getItem: vi.fn(() => null),
    setItem: vi.fn(),
    removeItem: vi.fn(),
  });
  const chat = await import('./chatSlice');
  const project = await import('../project/projectSlice');
  const utils = await import('./utils');
  return {
    reducer: chat.default as Reducer<ChatState, AnyAction>,
    agentEventsBatch: chat.agentEventsBatch,
    loadMorePrompts: chat.loadMorePrompts,
    retryFromEntry: chat.retryFromEntry,
    runIdSet: chat.runIdSet,
    userMessageSent: chat.userMessageSent,
    setActiveSession: project.setActiveSession,
    entriesToMessages: utils.entriesToMessages,
  };
}

beforeEach(() => {
  vi.unstubAllGlobals();
});

describe('chat reducer session routing', () => {
  it('routes run events to the session mapped by runIdSet', async () => {
    const { reducer, setActiveSession, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, setActiveSession('s1'));
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));

    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        { event: 'model_streaming', run_id: 'run-1', turn_id: 'turn-1', message_id: 'msg-1', delta: { Text: 'assistant text' } },
        { event: 'message_end', run_id: 'run-1', turn_id: 'turn-1', message_id: 'msg-1' },
      ]),
    );

    const turn = state.entries.s1.find((e) => e.type === 'turn');
    expect(turn?.blocks?.some((b) => b.type === 'assistant' && b.text === 'assistant text')).toBe(true);
    expect(state.isDirty.s1).toBe(true);
  });

  it('retryFromEntry truncates entries and prompts at the retried user message', async () => {
    const { reducer, setActiveSession, userMessageSent, runIdSet, retryFromEntry } = await loadModules();
    let state = reducer(undefined, setActiveSession('s1'));
    state = reducer(state, userMessageSent({ text: 'one', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));
    state = structuredClone(state);
    state.entries.s1.push({ id: 'turn-run-1', type: 'turn', promptId: 'run-1', blocks: [{ type: 'assistant', text: 'old answer', isStreaming: false }] });
    state = reducer(state, userMessageSent({ text: 'two', model: 'm2', sessionId: 's1' }));

    state = reducer(state, retryFromEntry({ id: 'user-run-1', text: 'edited one' }));

    expect(state.entries.s1.map((e) => e.text ?? e.type)).toEqual(['edited one']);
    expect(state.entries.s1[0].model).toBe('m1');
    expect(state.allPrompts.s1).toHaveLength(1);
    expect(state.allPrompts.s1[0].messages[0].content).toBe('edited one');
  });

  it('loadMorePrompts preserves live prompt blocks when rebuilding visible entries', async () => {
    const { reducer, setActiveSession, loadMorePrompts } = await loadModules();
    let state = reducer(undefined, setActiveSession('s1'));
    state = structuredClone(state);
    state.allPrompts.s1 = [
      {
        id: 'p-old',
        session_id: 's1',
        turn_index: 0,
        model: 'm0',
        status: 'completed',
        token_usage: {},
        started_at: null,
        ended_at: null,
        created_at: '',
        messages: [{ role: 'user', content: 'old' }, { role: 'assistant', content: 'old answer' }],
      },
    ];
    state.visiblePromptsCount.s1 = 1;
    state.entries.s1 = [
      { id: 'user-p-live', type: 'user', promptId: 'p-live', text: 'live', model: 'm1' },
      { id: 'turn-p-live', type: 'turn', promptId: 'p-live', blocks: [{ type: 'assistant', text: 'live answer', isStreaming: false }] },
    ];

    state = reducer(state, loadMorePrompts());

    expect(state.entries.s1.some((e) => e.promptId === 'p-live')).toBe(true);
    expect(state.entries.s1.some((e) => e.type === 'turn' && e.blocks?.some((b) => b.type === 'assistant' && b.text === 'live answer'))).toBe(true);
  });
});

describe('chat serialization', () => {
  it('serializes thinking blocks using parseable think tags', async () => {
    const { entriesToMessages } = await loadModules();
    const messages = entriesToMessages(
      [
        { id: 'user-1', type: 'user', promptId: 'p1', text: 'question' },
        {
          id: 'turn-1',
          type: 'turn',
          promptId: 'p1',
          blocks: [
            { type: 'thinking', text: 'reasoning', isStreaming: false },
            { type: 'assistant', text: 'answer', isStreaming: false },
          ],
        },
      ],
      {},
    );

    expect(messages[1].content).toBe('<think>reasoning</think>\nanswer');
  });
});
