import { describe, expect, it } from 'vitest';
import {
  applyConversationAgentEvent,
  createLiveTurn,
  groupConversationItems,
  placeOutboundReplyReceipts,
} from './conversationTurns';
import type { AgentConversationMessage } from '../../features/agents/types';

function msg(partial: Partial<AgentConversationMessage> & Pick<AgentConversationMessage, 'role' | 'content'>): AgentConversationMessage {
  return partial;
}

describe('groupConversationItems', () => {
  it('splits thinking and visible text, and folds tool calls into one turn', () => {
    const items = groupConversationItems([
      msg({ role: 'user', content: 'add 1+1' }),
      msg({
        role: 'assistant',
        content: '<think>write a tiny script</think>\nI will create demo.py',
        tool_calls: [{ id: 'call-1', function: { name: 'write_file', arguments: '{"path":"demo.py"}' } }],
      }),
      msg({ role: 'tool', content: 'wrote demo.py', tool_call_id: 'call-1', name: 'write_file' }),
      msg({ role: 'assistant', content: '<think>run it next</think>\nIt prints 2.' }),
    ]);

    expect(items.map((item) => item.type)).toEqual(['user', 'turn']);
    const turn = items[1];
    expect(turn.type).toBe('turn');
    if (turn.type !== 'turn') return;
    expect(turn.entry.blocks?.map((block) => block.type)).toEqual([
      'thinking',
      'assistant',
      'tool',
      'thinking',
      'assistant',
    ]);
    const tool = turn.entry.blocks?.find((block) => block.type === 'tool');
    expect(tool).toMatchObject({ type: 'tool', name: 'write_file', result: 'wrote demo.py' });
  });

  it('keeps inbound peer messages out of the agent turn', () => {
    const items = groupConversationItems([
      msg({
        role: 'user',
        content: 'envelope',
        metadata: {
          agent_messaging: {
            direction: 'inbound_reply',
            display_content: 'Looks correct',
            from_display_name: 'Debugger',
          },
        },
      }),
      msg({ role: 'assistant', content: 'Debugger confirmed the script.' }),
    ]);
    expect(items.map((item) => item.type)).toEqual(['peer', 'turn']);
  });
});

describe('placeOutboundReplyReceipts', () => {
  it('pins a reply receipt under the turn that called send_agent_message, not a later turn', () => {
    const items = groupConversationItems([
      msg({
        role: 'user',
        content: 'envelope',
        metadata: {
          agent_messaging: {
            direction: 'inbound',
            from_display_name: 'Coder',
            display_content: 'Please review demo.py',
          },
        },
      }),
      msg({
        role: 'assistant',
        content: 'Review looks good.',
        tool_calls: [{
          id: 'call-reply',
          function: { name: 'send_agent_message', arguments: '{"to":"coder","message":"Looks correct"}' },
        }],
      }),
      msg({ role: 'tool', content: 'sent', tool_call_id: 'call-reply', name: 'send_agent_message' }),
      msg({ role: 'user', content: 'keep going on tests' }),
      msg({ role: 'assistant', content: 'I will write more tests.' }),
    ]);

    const replyTurn = items.find((item) => item.type === 'turn');
    const laterTurn = items.filter((item) => item.type === 'turn').at(-1);
    expect(replyTurn?.type).toBe('turn');
    expect(laterTurn?.type).toBe('turn');
    if (replyTurn?.type !== 'turn' || laterTurn?.type !== 'turn') return;

    const placed = placeOutboundReplyReceipts(items, [
      { message_id: 'reply-1', payload: { to: 'coder' } },
    ]);

    expect(placed.byTurnKey.get(replyTurn.key)?.map((receipt) => receipt.message_id)).toEqual(['reply-1']);
    expect(placed.byTurnKey.get(laterTurn.key)).toBeUndefined();
    expect(placed.leftover).toEqual([]);
  });
});

describe('applyConversationAgentEvent', () => {
  it('streams thinking, then a tool call, into chat blocks', () => {
    let turn = createLiveTurn('turn-1', 1);
    turn = applyConversationAgentEvent(turn, {
      SubagentMessageUpdate: {
        subagent_id: 'coder',
        message_id: 'm1',
        delta: { Thinking: 'need a script' },
      },
    });
    turn = applyConversationAgentEvent(turn, {
      SubagentToolStart: {
        subagent_id: 'coder',
        tool_call_id: 'call-1',
        tool_name: 'write_file',
        args: { path: 'demo.py' },
      },
    });
    turn = applyConversationAgentEvent(turn, {
      SubagentToolEnd: {
        subagent_id: 'coder',
        tool_call_id: 'call-1',
        tool_name: 'write_file',
        result: 'ok',
        is_error: false,
      },
    });
    turn = applyConversationAgentEvent(turn, { SubagentEnd: { subagent_id: 'coder', success: true, iterations_used: 1 } });

    expect(turn.blocks[0]).toMatchObject({ type: 'thinking', text: 'need a script', isStreaming: false });
    expect(turn.blocks[1]).toMatchObject({
      type: 'tool',
      name: 'write_file',
      result: 'ok',
      active: false,
    });
    expect(turn.endTime).toBeDefined();
  });
});
