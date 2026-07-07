import type { ChatEntry, SubagentEntry, TurnBlock, SubagentBlock, DeltaPayload } from './types';
import type { FrontendMessage } from '../project/projectSlice';

// ── Block helpers (shared between main agent + subagent) ─────────────

export type AnyBlock = TurnBlock | SubagentBlock;

export function blockMessageId(b: AnyBlock): string | undefined {
  return 'message_id' in b ? (b as { message_id?: string }).message_id : undefined;
}

export function closeStreamingBlock(blocks: AnyBlock[] | undefined, messageId?: string): void {
  if (!blocks || blocks.length === 0) return;
  for (let i = blocks.length - 1; i >= 0; i--) {
    const block = blocks[i];
    if ('isStreaming' in block && block.isStreaming) {
      if (messageId !== undefined && blockMessageId(block) !== messageId) {
        continue;
      }
      block.isStreaming = false;
      if (block.type === 'thinking') {
        block.endTime = Date.now();
      }
    } else if (messageId === undefined) {
      break;
    }
  }
}

// ── Cross-chunk <think> tag reassembly (P0-4) ────────────────────────

const THINK_OPEN = '<think>';
const THINK_CLOSE = '</think>';

function getPartialThinkTagSuffix(text: string): string {
  let longest = '';
  for (const tag of [THINK_OPEN, THINK_CLOSE]) {
    for (let i = 1; i < tag.length; i++) {
      const prefix = tag.substring(0, i);
      if (text.endsWith(prefix) && prefix.length > longest.length) {
        longest = prefix;
      }
    }
  }
  return longest;
}

export function processThinkBuffer(
  buffers: Record<string, string>,
  key: string,
  delta: DeltaPayload
): DeltaPayload {
  if (typeof delta.Text !== 'string' || !delta.Text) return delta;
  const prev = buffers[key] ?? '';
  let text = prev + delta.Text;
  buffers[key] = '';
  const partial = getPartialThinkTagSuffix(text);
  if (partial) {
    buffers[key] = text.slice(text.length - partial.length);
    text = text.slice(0, text.length - partial.length);
  }
  return { ...delta, Text: text };
}

export function appendDeltaToBlocks(
  blocks: AnyBlock[],
  delta: DeltaPayload,
  messageId?: string
): void {
  const appendToType = (text: string, targetType: 'assistant' | 'thinking') => {
    let targetBlock: AnyBlock | null = null;
    for (let i = blocks.length - 1; i >= 0; i--) {
      const b = blocks[i];
      if ('isStreaming' in b && b.isStreaming && b.type === targetType && blockMessageId(b) === messageId) {
        targetBlock = b;
        break;
      }
    }

    if (!targetBlock) {
      if (targetType === 'thinking') {
        const lastBlock = blocks[blocks.length - 1];
        if (lastBlock && lastBlock.type === 'assistant' && 'isStreaming' in lastBlock && lastBlock.isStreaming && blockMessageId(lastBlock) === messageId) {
          blocks.splice(blocks.length - 1, 0, { type: 'thinking', text: '', isStreaming: true, message_id: messageId, startTime: Date.now() });
          targetBlock = blocks[blocks.length - 2];
        } else {
          blocks.push({ type: 'thinking', text: '', isStreaming: true, message_id: messageId, startTime: Date.now() });
          targetBlock = blocks[blocks.length - 1];
        }
      } else {
        blocks.push({ type: 'assistant', text: '', isStreaming: true, message_id: messageId });
        targetBlock = blocks[blocks.length - 1];
      }
    }

    if (targetBlock.type === 'assistant' || targetBlock.type === 'thinking') {
      targetBlock.text += text;
    }
  };

  if (typeof delta.Text === 'string') {
    let textChunk = delta.Text;

    while (textChunk.includes('<think>') || textChunk.includes('</think>')) {
      const thinkStartIdx = textChunk.indexOf('<think>');
      const thinkEndIdx = textChunk.indexOf('</think>');

      if (thinkStartIdx !== -1 && (thinkEndIdx === -1 || thinkStartIdx < thinkEndIdx)) {
        const before = textChunk.substring(0, thinkStartIdx);
        if (before) appendToType(before, 'assistant');
        textChunk = textChunk.substring(thinkStartIdx + 7);
      } else if (thinkEndIdx !== -1) {
        const before = textChunk.substring(0, thinkEndIdx);
        if (before) appendToType(before, 'thinking');
        textChunk = textChunk.substring(thinkEndIdx + 8);
      }
    }

    if (textChunk) {
      appendToType(textChunk, 'assistant');
    }
  }

  if (typeof delta.Thinking === 'string') {
    appendToType(delta.Thinking, 'thinking');
  }
}

// ── Result truncation ────────────────────────────────────────────────

const MAX_RESULT_LEN = 5000;

export function truncateResult(result: string): string {
  if (result.length > MAX_RESULT_LEN) {
    return result.substring(0, MAX_RESULT_LEN) + `\n\n... [Truncated ${result.length - MAX_RESULT_LEN} characters for performance]`;
  }
  return result;
}

export function stringifyResult(result: unknown): string {
  if (result === undefined || result === null) return '';
  if (typeof result === 'string') return result;
  try {
    const s = JSON.stringify(result, null, 2);
    return typeof s === 'string' ? s : String(result);
  } catch {
    return String(result);
  }
}

// ── Serialization helpers (used by useSaveSession) ───────────────────

export function entriesToMessages(entries: ChatEntry[]): FrontendMessage[] {
  const msgs: FrontendMessage[] = [];
  for (const entry of entries) {
    if (entry.type === 'user' && entry.text) {
      msgs.push({ role: 'user', content: entry.text, model: entry.model });
    } else if (entry.type === 'turn' && entry.blocks) {
      let thinkingText = '';
      let assistantText = '';
      const toolBlocks: Extract<TurnBlock, { type: 'tool' }>[] = [];

      for (const block of entry.blocks) {
        if (block.type === 'thinking') {
          thinkingText += block.text;
        } else if (block.type === 'assistant') {
          assistantText += block.text;
        } else if (block.type === 'tool') {
          toolBlocks.push(block);
        }
      }

      let content = assistantText.trim();
      if (thinkingText.trim()) {
        content = `<think>${thinkingText.trim()}</think>${content ? '\n' + content : ''}`;
      }

      if (toolBlocks.length > 0) {
        const tool_calls = toolBlocks.map(tb => ({
          id: tb.call_id,
          type: 'function',
          function: {
            name: tb.name,
            arguments: typeof tb.args === 'string' ? tb.args : JSON.stringify(tb.args || {})
          }
        }));

        msgs.push({
          role: 'assistant',
          content: content || '',
          model: entry.model,
          tool_calls
        });

        for (const tb of toolBlocks) {
          msgs.push({
            role: 'tool',
            content: tb.result || '',
            tool_call_id: tb.call_id,
            name: tb.name,
            model: entry.model
          });
        }
      } else {
        if (content) {
          msgs.push({ role: 'assistant', content, model: entry.model });
        }
      }
    }
  }
  return msgs;
}

export function entriesToEventLog(
  entries: ChatEntry[],
  subagents: Record<string, SubagentEntry>
): {
  eventLog: unknown[];
  processTimeMs: number;
  thoughtTimeMs: number;
} {
  const eventLog: unknown[] = [];
  let processTimeMs = 0;
  let thoughtTimeMs = 0;
  let assistantIdx = 0;

  for (const entry of entries) {
    if (entry.type === 'turn' && entry.blocks) {
      let assistantText = '';
      for (const b of entry.blocks) {
        if (b.type === 'assistant') assistantText += b.text;
      }
      if (!assistantText.trim()) continue;

      if (entry.startTime && entry.endTime) {
        processTimeMs += entry.endTime - entry.startTime;
      }

      if (entry.startTime || entry.endTime || entry.cacheHitRate !== undefined || entry.turnIds) {
        eventLog.push({
          turn_index: assistantIdx,
          event_type: 'turn_meta',
          payload: { 
            startTime: entry.startTime, 
            endTime: entry.endTime, 
            cacheHitRate: entry.cacheHitRate,
            turnIds: entry.turnIds,
          },
        });
      }

      for (const b of entry.blocks) {
        if (b.type === 'thinking') {
          if (b.startTime && b.endTime) thoughtTimeMs += b.endTime - b.startTime;
          eventLog.push({
            turn_index: assistantIdx,
            event_type: 'thinking',
            payload: { text: b.text, startTime: b.startTime, endTime: b.endTime },
          });
        } else if (b.type === 'tool') {
          eventLog.push({
            turn_index: assistantIdx,
            event_type: 'tool_call',
            payload: { name: b.name, args: b.args, args_summary: b.result?.slice(0, 1000), is_error: b.is_error },
          });
        } else if (b.type === 'subagent_ref') {
          const sa = subagents[b.subagent_id];
          if (sa) {
            eventLog.push({
              turn_index: assistantIdx,
              event_type: 'subagent',
              payload: sa,
            });
          }
        } else if (b.type === 'assistant') {
          eventLog.push({
            turn_index: assistantIdx,
            event_type: 'assistant',
            payload: { text: b.text },
          });
        }
      }
      assistantIdx++;
    }
  }

  return { eventLog, processTimeMs, thoughtTimeMs };
}
