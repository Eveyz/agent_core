import { canonicalBlocks } from '../../features/chat/canonicalBlocks';
import type { ChatEntry, FrontendMessage, TurnBlock } from '../../features/chat/types';
import {
  appendDeltaToBlocks,
  closeStreamingThinking,
  type AnyBlock,
} from '../../features/chat/utils';
import type { AgentConversationMessage } from '../../features/agents/types';

export interface AgentMessageMetadata {
  direction?: 'outbound_request' | 'inbound' | 'inbound_reply';
  message_id?: string;
  from_agent_id?: string;
  from_display_name?: string;
  to_agent_id?: string;
  to_display_name?: string;
  display_content?: string;
  kind?: string;
  priority?: boolean;
}

export function messageMetadata(message: AgentConversationMessage): AgentMessageMetadata | null {
  const value = message.metadata?.agent_messaging;
  if (!value || typeof value !== 'object') return null;
  return value as AgentMessageMetadata;
}

export function isPeerInboundMessage(message: AgentConversationMessage): boolean {
  const direction = messageMetadata(message)?.direction;
  return direction === 'inbound' || direction === 'inbound_reply';
}

export type ConversationRenderItem =
  | { type: 'user'; key: string; message: AgentConversationMessage }
  | { type: 'peer'; key: string; message: AgentConversationMessage }
  | { type: 'turn'; key: string; entry: ChatEntry };

export interface OutboundReplyReceipt {
  message_id?: string;
  payload: { to?: string };
}

export interface PlacedOutboundReplies<T extends OutboundReplyReceipt> {
  byTurnKey: Map<string, T[]>;
  leftover: T[];
}

function toFrontendMessage(message: AgentConversationMessage): FrontendMessage {
  return {
    role: message.role,
    content: message.content,
    model: message.model,
    tool_calls: message.tool_calls,
    tool_call_id: message.tool_call_id,
    name: message.name,
    metadata: message.metadata,
  };
}

function turnEntry(id: string, messages: AgentConversationMessage[], streaming: boolean): ChatEntry {
  const now = Date.now();
  return {
    id,
    type: 'turn',
    blocks: canonicalBlocks(messages.map(toFrontendMessage)),
    startTime: streaming ? now : undefined,
    endTime: streaming ? undefined : now,
  };
}

/** Group a contact session into user bubbles, peer cards, and chat-style agent turns. */
export function groupConversationItems(
  messages: AgentConversationMessage[],
): ConversationRenderItem[] {
  const items: ConversationRenderItem[] = [];
  let turnMessages: AgentConversationMessage[] = [];
  let turnStart = -1;

  const flushTurn = () => {
    if (turnMessages.length === 0) return;
    const entry = turnEntry(`turn-${turnStart}`, turnMessages, false);
    if (entry.blocks && entry.blocks.length > 0) {
      items.push({ type: 'turn', key: `turn-${turnStart}`, entry });
    }
    turnMessages = [];
    turnStart = -1;
  };

  messages.forEach((message, index) => {
    if (isPeerInboundMessage(message)) {
      flushTurn();
      items.push({ type: 'peer', key: `peer-${index}`, message });
      return;
    }
    if (message.role === 'user') {
      flushTurn();
      items.push({ type: 'user', key: `user-${index}`, message });
      return;
    }
    if (message.role === 'assistant' || message.role === 'tool') {
      if (turnStart < 0) turnStart = index;
      turnMessages.push(message);
      return;
    }
    flushTurn();
  });
  flushTurn();
  return items;
}

function toolArgRecord(args: unknown): Record<string, unknown> | null {
  if (args && typeof args === 'object' && !Array.isArray(args)) {
    return args as Record<string, unknown>;
  }
  if (typeof args === 'string') {
    try {
      const parsed = JSON.parse(args) as unknown;
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        return parsed as Record<string, unknown>;
      }
    } catch {
      return null;
    }
  }
  return null;
}

export function sendAgentMessageTargets(entry: ChatEntry): string[] {
  return (entry.blocks ?? []).flatMap((block) => {
    if (block.type !== 'tool' || block.name !== 'send_agent_message') return [];
    const to = toolArgRecord(block.args)?.to;
    return typeof to === 'string' && to.trim() ? [to.trim()] : [];
  });
}

function sameContact(left?: string, right?: string): boolean {
  if (!left || !right) return false;
  return left.trim().toLowerCase() === right.trim().toLowerCase();
}

/** Pin each outbound reply receipt under the turn that sent it, not the thread footer. */
export function placeOutboundReplyReceipts<T extends OutboundReplyReceipt>(
  items: ConversationRenderItem[],
  receipts: T[],
): PlacedOutboundReplies<T> {
  const unused = [...receipts];
  const byTurnKey = new Map<string, T[]>();

  const assign = (key: string, receipt: T) => {
    const current = byTurnKey.get(key) ?? [];
    current.push(receipt);
    byTurnKey.set(key, current);
  };

  items.forEach((item, index) => {
    if (item.type !== 'turn') return;
    const targets = sendAgentMessageTargets(item.entry);
    let placed = false;
    for (const target of targets) {
      const matchIndex = unused.findIndex((receipt) => sameContact(receipt.payload.to, target));
      if (matchIndex < 0) continue;
      assign(item.key, unused.splice(matchIndex, 1)[0]);
      placed = true;
    }
    if (placed) return;
    const previous = items[index - 1];
    if (previous?.type === 'peer' && unused.length > 0) {
      assign(item.key, unused.shift()!);
    }
  });

  return { byTurnKey, leftover: unused };
}

export interface LiveConversationTurn {
  turnId: string;
  blocks: TurnBlock[];
  startTime: number;
  endTime?: number;
}

export function createLiveTurn(turnId: string, startTime = Date.now()): LiveConversationTurn {
  return { turnId, blocks: [], startTime };
}

export function liveTurnEntry(turn: LiveConversationTurn): ChatEntry {
  return {
    id: `live-${turn.turnId}`,
    type: 'turn',
    blocks: turn.blocks,
    startTime: turn.startTime,
    endTime: turn.endTime,
  };
}

type UnwrappedEvent = { type: string; payload: Record<string, unknown> };

function unwrapEvent(event: unknown): UnwrappedEvent | null {
  if (typeof event === 'string') return { type: event, payload: {} };
  if (!event || typeof event !== 'object') return null;
  const record = event as Record<string, unknown>;
  const keys = Object.keys(record);
  if (keys.length === 1) {
    const type = keys[0];
    const payload = record[type];
    if (payload && typeof payload === 'object' && !Array.isArray(payload)) {
      return { type, payload: payload as Record<string, unknown> };
    }
    return { type, payload: {} };
  }
  if (typeof record.event === 'string') {
    return { type: record.event, payload: record };
  }
  return null;
}

function deltaPayload(delta: unknown): { Text?: string; Thinking?: string } | null {
  if (!delta || typeof delta !== 'object') return null;
  const record = delta as Record<string, unknown>;
  if (typeof record.Text === 'string') return { Text: record.Text };
  if (typeof record.Thinking === 'string') return { Thinking: record.Thinking };
  if (typeof record.text === 'string') return { Text: record.text };
  if (typeof record.thinking === 'string') return { Thinking: record.thinking };
  return null;
}

function cloneBlocks(blocks: TurnBlock[]): TurnBlock[] {
  return blocks.map((block) => ({ ...block }));
}

function pushToolStart(blocks: AnyBlock[], callId: string, name: string, args?: unknown): void {
  closeStreamingThinking(blocks);
  const existing = blocks.find((block) => block.type === 'tool' && block.call_id === callId);
  if (existing && existing.type === 'tool') {
    existing.name = name;
    existing.args = args ?? existing.args;
    existing.active = true;
    existing.is_error = false;
    existing.phase = 'running';
    existing.startTime ??= Date.now();
    return;
  }
  blocks.push({
    type: 'tool',
    call_id: callId,
    name,
    args,
    result: '',
    active: true,
    is_error: false,
    startTime: Date.now(),
    phase: 'running',
  });
}

function applyToolEnd(
  blocks: AnyBlock[],
  callId: string,
  name: string,
  result: string,
  isError: boolean,
): void {
  const existing = [...blocks].reverse().find(
    (block) => block.type === 'tool' && (callId ? block.call_id === callId : block.active),
  );
  if (existing && existing.type === 'tool') {
    existing.result = result;
    existing.active = false;
    existing.is_error = isError;
    existing.endTime = Date.now();
    existing.phase = undefined;
    if (name) existing.name = name;
    return;
  }
  blocks.push({
    type: 'tool',
    call_id: callId || `tool-${blocks.length}`,
    name: name || 'tool',
    result,
    active: false,
    is_error: isError,
    endTime: Date.now(),
  });
}

function finalizeBlocks(blocks: AnyBlock[]): void {
  for (const block of blocks) {
    if ('isStreaming' in block && block.isStreaming) {
      block.isStreaming = false;
      if (block.type === 'thinking') block.endTime = Date.now();
    }
    if (block.type === 'tool' && block.active) {
      block.active = false;
      block.endTime ??= Date.now();
    }
  }
}

/** Apply a custom-agent / subagent stream event onto the in-progress contact turn. */
export function applyConversationAgentEvent(
  turn: LiveConversationTurn,
  event: unknown,
): LiveConversationTurn {
  const unwrapped = unwrapEvent(event);
  if (!unwrapped) return turn;

  const next: LiveConversationTurn = {
    ...turn,
    blocks: cloneBlocks(turn.blocks),
  };
  const { type, payload } = unwrapped;
  const blocks = next.blocks as AnyBlock[];

  switch (type) {
    case 'SubagentStart':
    case 'AgentStart':
      return next;
    case 'SubagentMessageUpdate':
    case 'MessageUpdate': {
      const messageId = typeof payload.message_id === 'string' ? payload.message_id : undefined;
      const delta = deltaPayload(payload.delta);
      if (delta) appendDeltaToBlocks(blocks, delta, messageId);
      break;
    }
    case 'SubagentToolStart':
    case 'ToolExecutionStart': {
      const callId = String(payload.tool_call_id ?? payload.call_id ?? '');
      const name = String(payload.tool_name ?? payload.name ?? 'tool');
      pushToolStart(blocks, callId, name, payload.args);
      break;
    }
    case 'ToolExecutionUpdate':
    case 'SubagentToolUpdate': {
      const callId = String(payload.tool_call_id ?? payload.call_id ?? '');
      const partial = String(payload.partial_result ?? payload.partial ?? '');
      const tool = [...blocks].reverse().find(
        (block) => block.type === 'tool' && (callId ? block.call_id === callId : block.active),
      );
      if (tool && tool.type === 'tool') tool.result = partial;
      break;
    }
    case 'SubagentToolEnd':
    case 'ToolExecutionEnd': {
      applyToolEnd(
        blocks,
        String(payload.tool_call_id ?? payload.call_id ?? ''),
        String(payload.tool_name ?? payload.name ?? 'tool'),
        String(payload.result ?? ''),
        Boolean(payload.is_error),
      );
      break;
    }
    case 'SubagentEnd':
    case 'AgentEnd':
    case 'Aborted':
      finalizeBlocks(blocks);
      next.endTime = Date.now();
      break;
    default:
      break;
  }

  return next;
}

export function conversationEventIsTerminal(event: unknown): boolean {
  const unwrapped = unwrapEvent(event);
  return unwrapped?.type === 'SubagentEnd'
    || unwrapped?.type === 'AgentEnd'
    || unwrapped?.type === 'Aborted';
}
