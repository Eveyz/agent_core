import type { TurnBlock, SubagentBlock, DeltaPayload } from './types';

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

/** Close thinking only — keep assistant text open so late content deltas
 *  after tool_call markers still append to the same narration block
 *  (avoids splitting "near-zero" across a tool row). */
export function closeStreamingThinking(blocks: AnyBlock[] | undefined): void {
  if (!blocks || blocks.length === 0) return;
  for (let i = blocks.length - 1; i >= 0; i--) {
    const block = blocks[i];
    if (block.type === 'thinking' && 'isStreaming' in block && block.isStreaming) {
      block.isStreaming = false;
      block.endTime = Date.now();
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

    // Same message may keep streaming text after tool_call markers. Reuse the
    // existing assistant block (even if closed) so narration stays one piece
    // before tools instead of splitting mid-sentence.
    if (!targetBlock && targetType === 'assistant' && messageId) {
      for (let i = blocks.length - 1; i >= 0; i--) {
        const b = blocks[i];
        if (b.type === 'assistant' && blockMessageId(b) === messageId) {
          targetBlock = b;
          if ('isStreaming' in b) b.isStreaming = true;
          break;
        }
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
        // Prefer inserting assistant before any trailing tool placeholders for
        // this turn, so text renders above tools even if tools arrived first.
        let insertAt = blocks.length;
        for (let i = blocks.length - 1; i >= 0; i--) {
          const b = blocks[i];
          if (b.type === 'tool' && (b.phase === 'preparing' || b.active)) {
            insertAt = i;
            continue;
          }
          break;
        }
        const newBlock: AnyBlock = { type: 'assistant', text: '', isStreaming: true, message_id: messageId };
        blocks.splice(insertAt, 0, newBlock);
        targetBlock = newBlock;
      }
    }

    if (targetBlock.type === 'assistant' || targetBlock.type === 'thinking') {
      targetBlock.text += text;
    }
  };

  if (typeof delta.Text === 'string') {
    let textChunk = delta.Text;

    while (textChunk.includes(THINK_OPEN) || textChunk.includes(THINK_CLOSE)) {
      const thinkStartIdx = textChunk.indexOf(THINK_OPEN);
      const thinkEndIdx = textChunk.indexOf(THINK_CLOSE);

      if (thinkStartIdx !== -1 && (thinkEndIdx === -1 || thinkStartIdx < thinkEndIdx)) {
        const before = textChunk.substring(0, thinkStartIdx);
        if (before) appendToType(before, 'assistant');
        textChunk = textChunk.substring(thinkStartIdx + THINK_OPEN.length);
      } else if (thinkEndIdx !== -1) {
        const before = textChunk.substring(0, thinkEndIdx);
        if (before) appendToType(before, 'thinking');
        textChunk = textChunk.substring(thinkEndIdx + THINK_CLOSE.length);
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
