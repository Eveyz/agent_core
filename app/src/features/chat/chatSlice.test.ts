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
  const utils = await import('./utils');
  return {
    reducer: chat.default as Reducer<ChatState, AnyAction>,
    agentEventsBatch: chat.agentEventsBatch,
    loadMorePrompts: chat.loadMorePrompts,
    retryFromEntry: chat.retryFromEntry,
    runIdSet: chat.runIdSet,
    userMessageSent: chat.userMessageSent,
    toolApprovalResponded: chat.toolApprovalResponded,
    clarificationAnswered: chat.clarificationAnswered,
    btwAsked: chat.btwAsked,
    entriesToMessages: utils.entriesToMessages,
  };
}

beforeEach(() => {
  vi.unstubAllGlobals();
});

describe('chat reducer session routing', () => {
  it('routes run events to the session mapped by runIdSet', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
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

  it('does not mix events across concurrent sessions in one batch', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'a', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));
    state = reducer(state, userMessageSent({ text: 'b', model: 'm1', sessionId: 's2' }));
    state = reducer(state, runIdSet({ runId: 'run-2', sessionId: 's2' }));

    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 't1', index: 0 },
        { event: 'turn_started', run_id: 'run-2', turn_id: 't2', index: 0 },
        { event: 'model_streaming', run_id: 'run-1', turn_id: 't1', message_id: 'm1', delta: { Text: 'from-s1' } },
        { event: 'model_streaming', run_id: 'run-2', turn_id: 't2', message_id: 'm2', delta: { Text: 'from-s2' } },
      ]),
    );

    const turn1 = state.entries.s1.find((e) => e.type === 'turn');
    const turn2 = state.entries.s2.find((e) => e.type === 'turn');
    expect(turn1?.blocks?.some((b) => b.type === 'assistant' && b.text === 'from-s1')).toBe(true);
    expect(turn2?.blocks?.some((b) => b.type === 'assistant' && b.text === 'from-s2')).toBe(true);
    expect(turn1?.blocks?.some((b) => b.type === 'assistant' && b.text === 'from-s2')).toBe(false);
  });

  it('toolApprovalResponded updates the targeted session even when another is active', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch, toolApprovalResponded } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));
    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        {
          event: 'approval_required',
          run_id: 'run-1',
          turn_id: 'turn-1',
          prompt_id: 'ap-1',
          tool_name: 'bash',
          tool_input: {},
          danger_level: 'high',
          explanation: 'run cmd',
        },
      ]),
    );

    // Simulate user viewing s2 while approving s1's prompt
    state = reducer(state, userMessageSent({ text: 'other', model: 'm1', sessionId: 's2' }));
    state = reducer(state, toolApprovalResponded({ sessionId: 's1', promptId: 'ap-1', approved: true }));

    const turn = state.entries.s1.find((e) => e.type === 'turn');
    const approval = turn?.blocks?.find((b) => b.type === 'approval');
    expect(approval && approval.type === 'approval' && approval.status).toBe('approved');
  });

  it('input_requested creates a clarification block and clarificationAnswered resolves it', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch, clarificationAnswered } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'vague goal', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));
    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        {
          event: 'input_requested',
          run_id: 'run-1',
          turn_id: 'turn-1',
          prompt_id: 'cl-1',
          title: 'Clarify goal',
          questions: [
            {
              id: 'scope',
              prompt: 'What scope?',
              allow_multiple: false,
              options: [
                { id: 'mvp', label: 'MVP' },
                { id: 'full', label: 'Full' },
              ],
            },
          ],
        },
      ]),
    );

    const turn = state.entries.s1.find((e) => e.type === 'turn');
    const pending = turn?.blocks?.find((b) => b.type === 'clarification');
    expect(pending && pending.type === 'clarification' && pending.status).toBe('pending');
    expect(pending && pending.type === 'clarification' && pending.questions).toHaveLength(1);

    state = reducer(
      state,
      clarificationAnswered({
        sessionId: 's1',
        promptId: 'cl-1',
        answers: { scope: ['mvp'] },
      }),
    );
    const answered = state.entries.s1
      .find((e) => e.type === 'turn')
      ?.blocks?.find((b) => b.type === 'clarification');
    expect(answered && answered.type === 'clarification' && answered.status).toBe('answered');
    expect(answered && answered.type === 'clarification' && answered.answers).toEqual({ scope: ['mvp'] });
  });

  it('scopes btw entries per session', async () => {
    const { reducer, btwAsked } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, btwAsked({ sessionId: 's1', id: 'b1', question: 'q1' }));
    state = reducer(state, btwAsked({ sessionId: 's2', id: 'b2', question: 'q2' }));
    expect(state.btwEntries.s1).toHaveLength(1);
    expect(state.btwEntries.s1[0].question).toBe('q1');
    expect(state.btwEntries.s2).toHaveLength(1);
    expect(state.btwEntries.s2[0].question).toBe('q2');
  });

  it('retryFromEntry truncates entries and prompts at the retried user message', async () => {
    const { reducer, userMessageSent, runIdSet, retryFromEntry } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'one', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));
    state = structuredClone(state);
    state.entries.s1.push({ id: 'turn-run-1', type: 'turn', promptId: 'run-1', blocks: [{ type: 'assistant', text: 'old answer', isStreaming: false }] });
    state = reducer(state, userMessageSent({ text: 'two', model: 'm2', sessionId: 's1' }));

    state = reducer(state, retryFromEntry({ sessionId: 's1', id: 'user-run-1', text: 'edited one' }));

    expect(state.entries.s1.map((e) => e.text ?? e.type)).toEqual(['edited one']);
    expect(state.entries.s1[0].model).toBe('m1');
    expect(state.allPrompts.s1).toHaveLength(1);
    expect(state.allPrompts.s1[0].messages[0].content).toBe('edited one');
  });

  it('loadMorePrompts preserves live prompt blocks when rebuilding visible entries', async () => {
    const { reducer, loadMorePrompts } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
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

    state = reducer(state, loadMorePrompts({ sessionId: 's1' }));

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

describe('recovery error events', () => {
  it('keeps processing true when recovery Error events arrive mid-run', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));
    expect(state.processing.s1).toBe(true);

    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        {
          event: 'error',
          run_id: 'run-1',
          turn_id: 'turn-1',
          message: 'retrying model call after 500ms',
        },
      ]),
    );

    expect(state.processing.s1).toBe(true);
    const turn = state.entries.s1.find((e) => e.type === 'turn');
    expect(turn?.endTime).toBeUndefined();
    expect(turn?.blocks?.some((b) => b.type === 'error' && b.text.includes('retrying'))).toBe(true);
  });

  it('clears processing on terminal Error events', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));

    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        {
          event: 'error',
          run_id: 'run-1',
          turn_id: 'turn-1',
          message: 'provider returned 500 Internal Server Error',
        },
      ]),
    );

    expect(state.processing.s1).toBe(false);
  });
});
