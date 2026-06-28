import type { RootState } from '../store';
import type { SubagentBlock, TurnBlock } from '../features/chat/chatSlice';

export function getActiveSessionTitle(projectState: RootState['project']): string {
  if (!projectState.activeSessionId || !projectState.activeProjectId) return '';
  const list = projectState.sessions[projectState.activeProjectId] ?? [];
  const s = list.find((s) => s.id === projectState.activeSessionId);
  return s?.title ?? '';
}

export function convertSubagentBlocks(blocks: SubagentBlock[]): TurnBlock[] {
  return blocks.map((b): TurnBlock => {
    switch (b.type) {
      case 'assistant':
        return {
          type: 'assistant',
          text: b.text ?? '',
          isStreaming: b.isStreaming ?? false,
          message_id: b.message_id,
        };
      case 'thinking':
        return {
          type: 'thinking',
          text: b.text ?? '',
          isStreaming: b.isStreaming ?? false,
          message_id: b.message_id,
          startTime: b.startTime,
          endTime: b.endTime,
        };
      case 'tool':
        return {
          type: 'tool',
          call_id: b.call_id ?? '',
          name: b.name ?? '',
          args: b.args,
          result: b.result ?? '',
          active: b.active ?? false,
          is_error: b.is_error ?? false,
          startTime: b.startTime,
          endTime: b.endTime,
        };
      case 'approval':
        return {
          type: 'approval',
          prompt_id: b.prompt_id ?? '',
          tool_name: b.tool_name ?? '',
          tool_input: b.tool_input,
          danger_level: b.danger_level ?? '',
          explanation: b.explanation ?? '',
          status: b.status ?? 'pending',
        };
      case 'error':
        return { type: 'error', text: b.text ?? '' };
      default:
        return { type: 'error', text: 'unknown block type' };
    }
  });
}
