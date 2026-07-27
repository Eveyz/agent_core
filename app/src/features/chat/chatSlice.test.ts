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
  return {
    reducer: chat.default as Reducer<ChatState, AnyAction>,
    agentEventsBatch: chat.agentEventsBatch,
    loadMorePrompts: chat.loadMorePrompts,
    runIdSet: chat.runIdSet,
    userMessageSent: chat.userMessageSent,
    toolApprovalResponded: chat.toolApprovalResponded,
    clarificationAnswered: chat.clarificationAnswered,
    btwAsked: chat.btwAsked,
    steerMessageQueued: chat.steerMessageQueued,
    cacheSkills: chat.cacheSkills,
    agentAborted: chat.agentAborted,
    plansHydrated: chat.plansHydrated,
  };
}

beforeEach(() => {
  vi.unstubAllGlobals();
});

describe('chat reducer session routing', () => {
  it('stores the workspace scope with the skill cache', async () => {
    const { reducer, cacheSkills } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(
      state,
      cacheSkills({
        scopeKey: 'session-a',
        skills: [{ name: 'review', description: 'Review code' }],
      }),
    );

    expect(state.skillsCache?.scopeKey).toBe('session-a');
    expect(state.skillsCache?.skills[0]?.name).toBe('review');
  });

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

  it('folds out-of-order replay exactly once in contiguous sequence order', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));
    state = reducer(state, agentEventsBatch([
      { event: 'turn_started', run_id: 'run-1', event_id: 'e1', seq: 1, turn_id: 't1', index: 0 },
      { event: 'model_streaming', run_id: 'run-1', event_id: 'e3', seq: 3, turn_id: 't1', message_id: 'm1', delta: { Text: 'B' } },
    ]));

    expect(state.pendingGapByRun['run-1']).toEqual({ fromSeq: 1, toSeq: 3 });
    state = reducer(state, agentEventsBatch([
      { event: 'model_streaming', run_id: 'run-1', event_id: 'e2', seq: 2, turn_id: 't1', message_id: 'm1', delta: { Text: 'A' } },
      { event: 'model_streaming', run_id: 'run-1', event_id: 'e3', seq: 3, turn_id: 't1', message_id: 'm1', delta: { Text: 'B' } },
    ]));

    const turn = state.entries.s1.find((entry) => entry.type === 'turn');
    const text = turn?.blocks?.filter((block) => block.type === 'assistant').map((block) => block.text).join('');
    expect(text).toBe('AB');
    expect(state.lastSeqByRun['run-1']).toBe(3);
    expect(state.pendingGapByRun['run-1']).toBeUndefined();
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
          tool_name: 'shell',
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

  it('tool_preparing upserts placeholders and tool_started upgrades without duplicating', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'scaffold', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));

    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        {
          event: 'tool_preparing',
          run_id: 'run-1',
          turn_id: 'turn-1',
          index: 0,
          call_id: 'c1',
          name: 'write_file',
        },
        {
          event: 'tool_preparing',
          run_id: 'run-1',
          turn_id: 'turn-1',
          index: 0,
          call_id: 'c1',
          name: 'write_file',
          hint_path: 'src/App.tsx',
        },
        {
          event: 'tool_preparing',
          run_id: 'run-1',
          turn_id: 'turn-1',
          index: 1,
          call_id: 'c2',
          name: 'write_file',
          hint_path: 'src/main.ts',
        },
      ]),
    );

    let turn = state.entries.s1.find((e) => e.type === 'turn');
    const preparing = turn?.blocks?.filter((b) => b.type === 'tool' && b.phase === 'preparing') ?? [];
    expect(preparing).toHaveLength(2);
    expect(preparing[0].type === 'tool' && preparing[0].hint_path).toBe('src/App.tsx');
    expect(preparing[1].type === 'tool' && preparing[1].hint_path).toBe('src/main.ts');

    state = reducer(
      state,
      agentEventsBatch([
        {
          event: 'tool_started',
          run_id: 'run-1',
          turn_id: 'turn-1',
          call_id: 'c1',
          name: 'write_file',
          args: { path: 'src/App.tsx', content: 'x' },
        },
      ]),
    );

    turn = state.entries.s1.find((e) => e.type === 'turn');
    const tools = turn?.blocks?.filter((b) => b.type === 'tool') ?? [];
    expect(tools).toHaveLength(2);
    const first = tools.find((b) => b.type === 'tool' && b.call_id === 'c1');
    expect(first && first.type === 'tool' && first.phase).toBe('running');
    expect(first && first.type === 'tool' && first.hint_path).toBeUndefined();
    const second = tools.find((b) => b.type === 'tool' && b.call_id === 'c2');
    expect(second && second.type === 'tool' && second.phase).toBe('preparing');
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

  it('retry appends a new user message without truncating history', async () => {
    const { reducer, userMessageSent, runIdSet } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'one', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1', promptId: 'prompt-1' }));
    state = structuredClone(state);
    state.entries.s1.push({ id: 'turn-prompt-1', type: 'turn', promptId: 'prompt-1', blocks: [{ type: 'assistant', text: 'old answer', isStreaming: false }] });
    state = reducer(state, userMessageSent({ text: 'two', model: 'm2', sessionId: 's1' }));

    // Retry is append-only: re-send the earlier prompt as a new user message.
    state = reducer(state, userMessageSent({ text: 'edited one', model: 'm1', sessionId: 's1' }));

    const userTexts = state.entries.s1.filter((e) => e.type === 'user').map((e) => e.text);
    expect(userTexts).toEqual(['one', 'two', 'edited one']);
    expect(state.allPrompts.s1).toHaveLength(3);
    expect(state.allPrompts.s1[state.allPrompts.s1.length - 1]?.messages[0].content).toBe('edited one');
  });

  it('runIdSet binds prompt identity from backend promptId, never from runId', async () => {
    const { reducer, userMessageSent, runIdSet } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(
      state,
      runIdSet({ runId: 'run-uuid', sessionId: 's1', promptId: 'prompt-uuid' }),
    );

    expect(state.runId.s1).toBe('run-uuid');
    const user = state.entries.s1.find((e) => e.type === 'user');
    const turn = state.entries.s1.find((e) => e.type === 'turn');
    expect(user?.promptId).toBe('prompt-uuid');
    expect(user?.id).toBe('user-prompt-uuid');
    expect(turn?.promptId).toBe('prompt-uuid');
    expect(turn?.id).toBe('turn-prompt-uuid');
    expect(state.allPrompts.s1[state.allPrompts.s1.length - 1]?.id).toBe('prompt-uuid');
    expect(user?.promptId).not.toBe(state.runId.s1);
  });

  it('runIdSet without promptId leaves placeholder prompt ids untouched', async () => {
    const { reducer, userMessageSent, runIdSet } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    const beforeUser = state.entries.s1.find((e) => e.type === 'user');
    const beforePromptId = beforeUser?.promptId;
    state = reducer(state, runIdSet({ runId: 'run-only', sessionId: 's1' }));

    expect(state.runId.s1).toBe('run-only');
    expect(state.entries.s1.find((e) => e.type === 'user')?.promptId).toBe(beforePromptId);
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

  it('restores every canonical model and tool iteration in prompt order', async () => {
    const { reducer } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, {
      type: 'project/resumeSession/fulfilled',
      payload: {
        meta: { id: 's1' },
        messages: [],
        prompts: [{
          id: 'p1', session_id: 's1', turn_index: 0, model: 'm', status: 'completed',
          token_usage: {}, started_at: null, ended_at: null, created_at: '',
          messages: [
            { role: 'user', content: 'task' },
            { role: 'assistant', content: 'checking', tool_calls: [{ id: 'c1', type: 'function', function: { name: 'read_file', arguments: '{"path":"a"}' } }] },
            { role: 'tool', content: 'A', tool_call_id: 'c1', name: 'read_file' },
            { role: 'assistant', content: 'searching', tool_calls: [{ id: 'c2', type: 'function', function: { name: 'grep', arguments: '{"pattern":"x"}' } }] },
            { role: 'tool', content: 'B', tool_call_id: 'c2', name: 'grep' },
            { role: 'assistant', content: 'done' },
          ],
        }],
      },
      meta: { arg: 's1' },
    });

    const turn = state.entries.s1.find((entry) => entry.type === 'turn');
    expect(turn?.blocks?.map((block) => block.type)).toEqual([
      'assistant', 'tool', 'assistant', 'tool', 'assistant',
    ]);
    const tools = turn?.blocks?.filter((block) => block.type === 'tool') ?? [];
    expect(tools[0].type === 'tool' && tools[0].result).toBe('A');
    expect(tools[1].type === 'tool' && tools[1].result).toBe('B');
  });

  it('hydrates Overview subagents from spawn tool args after resume', async () => {
    const { reducer } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, {
      type: 'project/resumeSession/fulfilled',
      payload: {
        meta: { id: 's1' },
        messages: [],
        prompts: [{
          id: 'p1', session_id: 's1', turn_index: 0, model: 'm', status: 'completed',
          token_usage: {}, started_at: null, ended_at: null, created_at: '',
          messages: [
            { role: 'user', content: 'check weather' },
            {
              role: 'assistant',
              content: '',
              tool_calls: [{
                id: 'call-sa',
                type: 'function',
                function: {
                  name: 'subagents',
                  arguments: JSON.stringify({
                    tasks: [
                      { id: 'weather-shanghai', task: 'SH' },
                      { id: 'weather-shenzhen', task: 'SZ' },
                    ],
                  }),
                },
              }],
            },
            {
              role: 'tool',
              content: `=== Sub-agent Batch Results (2 tasks) ===

[1] weather-shanghai — success
[subagent-handoff/v1]
runtime_id: aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa
status: succeeded
context_sufficient: true
iterations: 1
tools: 1

Shanghai ok

[2] weather-shenzhen — success
[subagent-handoff/v1]
runtime_id: bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb
status: succeeded
context_sufficient: true
iterations: 1
tools: 1

Shenzhen ok

=== End batch results ===`,
              tool_call_id: 'call-sa',
              name: 'subagents',
            },
            { role: 'assistant', content: 'done' },
          ],
        }],
      },
      meta: { arg: 's1' },
    });

    const turn = state.entries.s1.find((entry) => entry.type === 'turn');
    expect(turn?.blocks?.some((b) => b.type === 'tool' && b.name === 'subagents')).toBe(true);
    expect(turn?.blocks?.filter((b) => b.type === 'subagent_ref')).toHaveLength(2);
    expect(Object.keys(state.subagents.s1)).toHaveLength(2);
    expect(state.subagents.s1['aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa']?.role_name).toBe(
      'weather-shanghai',
    );
  });
});

describe('runtime notice and error events', () => {
  it('keeps processing true when recoverable Notice events arrive mid-run', async () => {
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
          event: 'notice',
          run_id: 'run-1',
          turn_id: 'turn-1',
          code: 'model_retry',
          severity: 'warning',
          recoverable: true,
          message: 'retrying model call after 500ms',
        },
      ]),
    );

    expect(state.processing.s1).toBe(true);
    const turn = state.entries.s1.find((e) => e.type === 'turn');
    expect(turn?.endTime).toBeUndefined();
    expect(turn?.blocks?.some((b) => b.type === 'notice' && b.text.includes('retrying'))).toBe(true);
  });

  it('replaces prior notice with the same code instead of stacking', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));

    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        {
          event: 'notice',
          run_id: 'run-1',
          turn_id: 'turn-1',
          code: 'model_retry',
          severity: 'warning',
          recoverable: true,
          message: 'Failed to connect to remote model (stream failed), retrying in 1s (attempt 1/5)',
        },
        {
          event: 'notice',
          run_id: 'run-1',
          turn_id: 'turn-1',
          code: 'model_retry',
          severity: 'warning',
          recoverable: true,
          message: 'Failed to connect to remote model (stream failed), retrying in 2s (attempt 2/5)',
        },
        {
          event: 'notice',
          run_id: 'run-1',
          turn_id: 'turn-1',
          code: 'model_retry',
          severity: 'warning',
          recoverable: true,
          message: 'Failed to connect to remote model (stream failed), retrying in 4s (attempt 3/5)',
        },
      ]),
    );

    const turn = state.entries.s1.find((e) => e.type === 'turn');
    const notices = turn?.blocks?.filter((b) => b.type === 'notice') ?? [];
    expect(notices).toHaveLength(1);
    expect(notices[0]).toMatchObject({
      type: 'notice',
      code: 'model_retry',
      text: 'Failed to connect to remote model (stream failed), retrying in 4s (attempt 3/5)',
      recoverable: true,
    });
  });

  it('collapses stream-retry and recovery-retry notices into one banner', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));

    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        {
          event: 'notice',
          run_id: 'run-1',
          turn_id: 'turn-1',
          code: 'model_stream_retry',
          severity: 'warning',
          recoverable: true,
          message:
            'Failed to connect to remote model (stream failed), retrying in 1s (attempt 2/5)',
        },
        {
          event: 'notice',
          run_id: 'run-1',
          turn_id: 'turn-1',
          code: 'model_retry',
          severity: 'warning',
          recoverable: true,
          message:
            'Failed to connect to remote model (rate limit), retrying in 2s (attempt 2/3)',
        },
      ]),
    );

    const turn = state.entries.s1.find((e) => e.type === 'turn');
    const notices = turn?.blocks?.filter((b) => b.type === 'notice') ?? [];
    expect(notices).toHaveLength(1);
    expect(notices[0]).toMatchObject({
      type: 'notice',
      code: 'model_retry',
      recoverable: true,
    });
    expect(notices[0]?.type === 'notice' && notices[0].text).toContain('rate limit');
  });

  it('clears recoverable model_retry notice once the stream resumes', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));

    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        {
          event: 'notice',
          run_id: 'run-1',
          turn_id: 'turn-1',
          code: 'model_retry',
          severity: 'warning',
          recoverable: true,
          message: 'Failed to connect to remote model (stream failed), retrying in 2s (attempt 2/5)',
        },
        {
          event: 'message_start',
          run_id: 'run-1',
          turn_id: 'turn-1',
          message_id: 'msg-1',
        },
      ]),
    );

    const turn = state.entries.s1.find((e) => e.type === 'turn');
    expect(turn?.blocks?.some((b) => b.type === 'notice')).toBe(false);
    expect(turn?.blocks?.some((b) => b.type === 'thinking')).toBe(true);
  });

  it('clears recoverable model_stream_retry notice when model_streaming resumes', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));

    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        {
          event: 'model_streaming',
          run_id: 'run-1',
          turn_id: 'turn-1',
          message_id: 'msg-1',
          delta: { Text: 'partial before drop' },
        },
        {
          event: 'notice',
          run_id: 'run-1',
          turn_id: 'turn-1',
          code: 'model_stream_retry',
          severity: 'warning',
          recoverable: true,
          message:
            'Failed to connect to remote model (stream failed), retrying in 1s (attempt 1/5)',
        },
        {
          event: 'model_streaming',
          run_id: 'run-1',
          turn_id: 'turn-1',
          message_id: 'msg-2',
          delta: { Text: 'resumed after retry' },
        },
      ]),
    );

    const turn = state.entries.s1.find((e) => e.type === 'turn');
    expect(turn?.blocks?.some((b) => b.type === 'notice')).toBe(false);
    expect(
      turn?.blocks?.some(
        (b) => b.type === 'assistant' && b.text.includes('resumed after retry'),
      ),
    ).toBe(true);
  });

  it('clears recoverable retry notice on model_call_started before any tokens', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));

    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        {
          event: 'notice',
          run_id: 'run-1',
          turn_id: 'turn-1',
          code: 'model_stream_retry',
          severity: 'warning',
          recoverable: true,
          message:
            'Failed to connect to remote model (stream failed), retrying in 1s (attempt 1/5)',
        },
        // Reconnected, but model may think silently for a long time — no deltas yet.
        { event: 'model_call_started', run_id: 'run-1', turn_id: 'turn-1' },
      ]),
    );

    const turn = state.entries.s1.find((e) => e.type === 'turn');
    expect(turn?.blocks?.some((b) => b.type === 'notice')).toBe(false);
  });

  it('surfaces context compaction and requests an immediate usage refresh', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));

    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        {
          event: 'context_compacted',
          run_id: 'run-1',
          turn_id: 'turn-1',
          summary: 'chunked_drop: 299142 → 30429 tokens (model window only)',
          strategy: 'chunked_drop',
          tokens_before: 299142,
          tokens_after: 30429,
        },
      ]),
    );

    const turn = state.entries.s1.find((entry) => entry.type === 'turn');
    expect(turn?.blocks?.find((block) => block.type === 'notice')).toMatchObject({
      type: 'notice',
      code: 'context_compacted',
      text: 'chunked_drop: 299142 → 30429 tokens (model window only)',
      recoverable: false,
      strategy: 'chunked_drop',
      tokens_before: 299142,
      tokens_after: 30429,
    });
    expect(state.contextUsageRevision.s1).toBe(1);
    expect(state.lastRunId.s1).toBe('run-1');
  });

  it('clears recoverable retry notice when the user aborts', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch, agentAborted } =
      await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hello', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));

    state = reducer(
      state,
      agentEventsBatch([
        { event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 },
        {
          event: 'notice',
          run_id: 'run-1',
          turn_id: 'turn-1',
          code: 'model_retry',
          severity: 'warning',
          recoverable: true,
          message:
            'Failed to connect to remote model (stream failed), retrying in 1s (attempt 1/5)',
        },
      ]),
    );

    state = reducer(state, agentAborted({ sessionId: 's1' }));

    const turn = state.entries.s1.find((e) => e.type === 'turn');
    expect(turn?.blocks?.some((b) => b.type === 'notice')).toBe(false);
    expect(
      turn?.blocks?.some((b) => b.type === 'error' && b.text.includes('Interrupted')),
    ).toBe(true);
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

describe('steer two-segment timeline', () => {
  it('merges turn_started into the open turn when a pending steer card is last', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch, steerMessageQueued } =
      await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'research world cup', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));
    state = reducer(
      state,
      agentEventsBatch([{ event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 }]),
    );

    const openTurnId = state.entries.s1.find((e) => e.type === 'turn')?.id;
    expect(openTurnId).toBeTruthy();

    // Optimistic steer card lands after the open turn (would previously split Worked).
    state = reducer(
      state,
      steerMessageQueued({ sessionId: 's1', steerId: 'steer-1', text: '阿根廷怎么样' }),
    );
    expect(state.entries.s1[state.entries.s1.length - 1]?.isSteer).toBe(true);

    state = reducer(
      state,
      agentEventsBatch([{ event: 'turn_started', run_id: 'run-1', turn_id: 'turn-2', index: 1 }]),
    );

    const turns = state.entries.s1.filter((e) => e.type === 'turn');
    expect(turns).toHaveLength(1);
    expect(turns[0].id).toBe(openTurnId);
    expect(turns[0].endTime).toBeUndefined();
    expect(turns[0].turnIds).toEqual(expect.arrayContaining(['turn-1', 'turn-2']));
  });

  it('closes the open turn on steer_injected and starts a new segment on next turn_started', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch, steerMessageQueued } =
      await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'research world cup', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));
    state = reducer(
      state,
      agentEventsBatch([{ event: 'turn_started', run_id: 'run-1', turn_id: 'turn-1', index: 0 }]),
    );
    state = reducer(
      state,
      steerMessageQueued({ sessionId: 's1', steerId: 'steer-1', text: '阿根廷怎么样' }),
    );
    // Pre-inject work continues in the same segment.
    state = reducer(
      state,
      agentEventsBatch([{ event: 'turn_started', run_id: 'run-1', turn_id: 'turn-2', index: 1 }]),
    );

    state = reducer(
      state,
      agentEventsBatch([
        {
          event: 'steer_injected',
          run_id: 'run-1',
          steer_id: 'steer-1',
          message: '阿根廷怎么样',
        },
      ]),
    );

    const preInjectTurn = state.entries.s1.find((e) => e.type === 'turn');
    expect(preInjectTurn?.endTime).toBeDefined();
    const steer = state.entries.s1.find((e) => e.isSteer);
    expect(steer?.steerStatus).toBe('injected');

    state = reducer(
      state,
      agentEventsBatch([{ event: 'turn_started', run_id: 'run-1', turn_id: 'turn-3', index: 2 }]),
    );

    const turns = state.entries.s1.filter((e) => e.type === 'turn');
    expect(turns).toHaveLength(2);
    expect(turns[0].endTime).toBeDefined();
    expect(turns[1].endTime).toBeUndefined();
    expect(turns[1].turnId).toBe('turn-3');
  });
});

describe('durable multi-plan todos', () => {
  it('keeps todos across userMessageSent', async () => {
    const { reducer, userMessageSent, plansHydrated } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(
      state,
      plansHydrated({
        sessionId: 's1',
        items: [{ id: '1', description: 'Step A', status: 'in_progress' }],
        parked: [{ id: 'p1', title: 'Old', completed: 1, total: 3, updated_at: '2026-01-01' }],
        activePlanId: 'ap1',
        activePlanTitle: 'Auth',
      }),
    );
    state = reducer(state, userMessageSent({ text: 'keep going', model: 'm1', sessionId: 's1' }));
    expect(state.todo.s1).toHaveLength(1);
    expect(state.parkedPlans.s1).toHaveLength(1);
    expect(state.activePlanTitle.s1).toBe('Auth');
  });

  it('applies todo_updated with parked stack', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch } = await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(state, userMessageSent({ text: 'hi', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));
    state = reducer(
      state,
      agentEventsBatch([
        {
          event: 'todo_updated',
          run_id: 'run-1',
          items: [{ id: '1', description: 'Do thing', status: 'pending' }],
          parked: [
            { id: 'park-1', title: 'Earlier', completed: 0, total: 2, updated_at: 't' },
          ],
          plans: [
            {
              id: 'active-1',
              title: 'Now',
              status: 'active',
              updated_at: 't',
              items: [{ id: '1', description: 'Do thing', status: 'pending' }],
            },
          ],
          active_plan_id: 'active-1',
          active_plan_title: 'Now',
        },
      ]),
    );
    expect(state.todo.s1[0].description).toBe('Do thing');
    expect(state.parkedPlans.s1[0].id).toBe('park-1');
    expect(state.plans.s1).toHaveLength(1);
    expect(state.plans.s1[0].title).toBe('Now');
    expect(state.activePlanId.s1).toBe('active-1');
    expect(state.activePlanTitle.s1).toBe('Now');
  });

  it('does not clear todos on run_started', async () => {
    const { reducer, userMessageSent, runIdSet, agentEventsBatch, plansHydrated } =
      await loadModules();
    let state = reducer(undefined, { type: '@@INIT' });
    state = reducer(
      state,
      plansHydrated({
        sessionId: 's1',
        items: [{ id: '1', description: 'Keep', status: 'pending' }],
        parked: [],
      }),
    );
    state = reducer(state, userMessageSent({ text: 'hi', model: 'm1', sessionId: 's1' }));
    state = reducer(state, runIdSet({ runId: 'run-1', sessionId: 's1' }));
    state = reducer(
      state,
      agentEventsBatch([{ event: 'run_started', run_id: 'run-1' }]),
    );
    expect(state.todo.s1).toHaveLength(1);
  });
});
