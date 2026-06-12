import { createSlice, PayloadAction } from '@reduxjs/toolkit';

export type TurnBlock =
  | { type: 'assistant'; text: string; isStreaming: boolean }
  | { type: 'thinking'; text: string; isStreaming: boolean }
  | { type: 'tool'; call_id: string; name: string; result: string; active: boolean; is_error: boolean }
  | { type: 'error'; text: string };

export interface ChatEntry {
  id: string;
  type: 'user' | 'turn';
  turnIndex?: number;
  text?: string;       // for user
  blocks?: TurnBlock[];// for turn
  startTime?: number;  // timestamp for TurnStart
  endTime?: number;    // timestamp for TurnEnd
}

interface ChatState {
  entries: ChatEntry[];
  isProcessing: boolean;
}

const initialState: ChatState = {
  entries: [],
  isProcessing: false,
};

export const chatSlice = createSlice({
  name: 'chat',
  initialState,
  reducers: {
    userMessageSent: (state, action: PayloadAction<string>) => {
      state.entries.push({
        id: `user-${Date.now()}`,
        type: 'user',
        text: action.payload,
      });
      state.isProcessing = true;
    },
    agentEventReceived: (state, action: PayloadAction<any>) => {
      let event = action.payload;
      if (typeof event === 'string') {
        if (event === 'AgentStart') {
           return;
        }
        try {
          event = JSON.parse(event);
        } catch (e) {}
      }

      if (event.TurnStart) {
        state.entries.push({
          id: `turn-${event.TurnStart.turn_index}-${Date.now()}`,
          type: 'turn',
          turnIndex: event.TurnStart.turn_index,
          blocks: [],
          startTime: Date.now(),
        });
      } else if (event.TurnEnd) {
        const last = state.entries[state.entries.length - 1];
        if (last && last.type === 'turn') {
          last.endTime = Date.now();
        }
      } else if (event.MessageUpdate) {
        const delta = event.MessageUpdate.delta;
        const lastEntry = state.entries[state.entries.length - 1];
        if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
          
          if (typeof delta.Text === 'string') {
            // Find or create active 'assistant' block
            let block = lastEntry.blocks[lastEntry.blocks.length - 1];
            if (!block || block.type !== 'assistant' || !block.isStreaming) {
               lastEntry.blocks.push({ type: 'assistant', text: '', isStreaming: true });
               block = lastEntry.blocks[lastEntry.blocks.length - 1];
            }
            if (block.type === 'assistant') {
               block.text += delta.Text;
            }
          } else if (typeof delta.Thinking === 'string') {
            // Find or create active 'thinking' block
            let block = lastEntry.blocks[lastEntry.blocks.length - 1];
            if (!block || block.type !== 'thinking' || !block.isStreaming) {
               lastEntry.blocks.push({ type: 'thinking', text: '', isStreaming: true });
               block = lastEntry.blocks[lastEntry.blocks.length - 1];
            }
            if (block.type === 'thinking') {
               block.text += delta.Thinking;
            }
          }
        }
      } else if (event.MessageEnd) {
        const lastEntry = state.entries[state.entries.length - 1];
        if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
           const block = lastEntry.blocks[lastEntry.blocks.length - 1];
           if (block && (block.type === 'assistant' || block.type === 'thinking')) {
              block.isStreaming = false;
           }
        }
      } else if (event.ToolExecutionStart) {
        const { tool_call_id, tool_name, args } = event.ToolExecutionStart;
        const lastEntry = state.entries[state.entries.length - 1];
        if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
           lastEntry.blocks.push({
             type: 'tool',
             call_id: tool_call_id,
             name: tool_name,
             result: `Executing ${tool_name} with ${JSON.stringify(args)}...\n`,
             active: true,
             is_error: false
           });
        }
      } else if (event.ToolExecutionUpdate) {
        const { tool_call_id, partial_result } = event.ToolExecutionUpdate;
        const lastEntry = state.entries[state.entries.length - 1];
        if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
           const block = lastEntry.blocks.find(b => b.type === 'tool' && b.call_id === tool_call_id);
           if (block && block.type === 'tool') {
             block.result += partial_result;
           }
        }
      } else if (event.ToolExecutionEnd) {
        const { tool_call_id, result, is_error } = event.ToolExecutionEnd;
        const lastEntry = state.entries[state.entries.length - 1];
        if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
           const block = lastEntry.blocks.find(b => b.type === 'tool' && b.call_id === tool_call_id);
           if (block && block.type === 'tool') {
             block.result = result;
             block.active = false;
             block.is_error = is_error;
           }
        }
      } else if (event.AgentEnd) {
        state.isProcessing = false;
      } else if (event.Error) {
        state.isProcessing = false;
        const lastEntry = state.entries[state.entries.length - 1];
        if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
          lastEntry.blocks.push({ type: 'error', text: event.Error });
        } else {
          // If the agent errored before starting a turn (e.g. invalid API key)
          state.entries.push({
            id: `error-${Date.now()}`,
            type: 'turn',
            turnIndex: 0,
            blocks: [{ type: 'error', text: event.Error }],
            startTime: Date.now(),
            endTime: Date.now()
          });
        }
      }
    },
    clearChat: (state) => {
      state.entries = [];
      state.isProcessing = false;
    }
  },
});

export const { userMessageSent, agentEventReceived, clearChat } = chatSlice.actions;
export default chatSlice.reducer;
