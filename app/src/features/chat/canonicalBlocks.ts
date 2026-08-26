import type { FrontendMessage, TurnBlock } from './types';

/** Rebuild thinking / assistant / tool blocks from a persisted message list. */
export function canonicalBlocks(messages: FrontendMessage[]): TurnBlock[] {
  const blocks: TurnBlock[] = [];
  const toolBlocks = new Map<string, Extract<TurnBlock, { type: 'tool' }>>();

  for (const message of messages) {
    if (message.role === 'assistant') {
      if (message.content) {
        const hasThinkTag = message.content.match(/<think>([\s\S]*?)<\/think>/);
        if (hasThinkTag) {
          blocks.push({ type: 'thinking', text: hasThinkTag[1], isStreaming: false });
          const visible = message.content.replace(/<think>[\s\S]*?<\/think>/, '').trim();
          if (visible) blocks.push({ type: 'assistant', text: visible, isStreaming: false });
        } else {
          blocks.push({ type: 'assistant', text: message.content, isStreaming: false });
        }
      }
      for (const tc of message.tool_calls ?? []) {
        let args: unknown = tc.function.arguments;
        try { args = JSON.parse(tc.function.arguments); } catch { /* retain raw args */ }
        const toolBlock: Extract<TurnBlock, { type: 'tool' }> = {
          type: 'tool',
          call_id: tc.id,
          name: tc.function.name,
          args,
          result: '',
          active: false,
          is_error: false,
        };
        toolBlocks.set(tc.id, toolBlock);
        blocks.push(toolBlock);
      }
    } else if (message.role === 'tool' && message.tool_call_id) {
      const toolBlock = toolBlocks.get(message.tool_call_id);
      if (toolBlock) {
        toolBlock.result = message.content ?? '';
        if (message.name) toolBlock.name = message.name;
      } else {
        blocks.push({
          type: 'tool',
          call_id: message.tool_call_id,
          name: message.name ?? 'tool',
          result: message.content ?? '',
          active: false,
          is_error: false,
        });
      }
    }
  }

  return blocks;
}
