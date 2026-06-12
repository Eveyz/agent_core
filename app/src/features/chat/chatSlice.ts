import { createSlice, PayloadAction } from '@reduxjs/toolkit';

export type TurnBlock =
  | { type: 'assistant'; text: string; isStreaming: boolean }
  | { type: 'thinking'; text: string; isStreaming: boolean; startTime?: number; endTime?: number }
  | { type: 'tool'; call_id: string; name: string; result: string; active: boolean; is_error: boolean }
  | { type: 'approval'; prompt_id: string; tool_name: string; tool_input: any; danger_level: string; explanation: string; status: 'pending' | 'approved' | 'denied' }
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
        const last = state.entries[state.entries.length - 1];
        // If the last entry is already an active turn, we merge it!
        if (last && last.type === 'turn' && !last.endTime) {
          last.turnIndex = event.TurnStart.turn_index; // Update index, but keep blocks
        } else {
          state.entries.push({
            id: `turn-${event.TurnStart.turn_index}-${Date.now()}`,
            type: 'turn',
            turnIndex: event.TurnStart.turn_index,
            blocks: [],
            startTime: Date.now(),
          });
        }
      } else if (event.TurnEnd) {
        // We no longer close the turn here, because another turn might follow.
        // We wait for AgentEnd to close it.
      } else if (event.MessageUpdate) {
        const delta = event.MessageUpdate.delta;
        const lastEntry = state.entries[state.entries.length - 1];
        if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
          
          if (typeof delta.Text === 'string') {
            // Find or create active 'assistant' block
            let block = lastEntry.blocks[lastEntry.blocks.length - 1];
            if (!block || block.type !== 'assistant' || !block.isStreaming) {
               if (block && ('isStreaming' in block) && block.isStreaming) {
                 block.isStreaming = false;
                 if (block.type === 'thinking') {
                   block.endTime = Date.now();
                 }
               }
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
               if (block && ('isStreaming' in block) && block.isStreaming) {
                 block.isStreaming = false;
                 // Should never happen that thinking follows thinking, but just in case
               }
               lastEntry.blocks.push({ type: 'thinking', text: '', isStreaming: true, startTime: Date.now() });
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
              if (block.type === 'thinking') {
                block.endTime = Date.now();
              }
           }
        }
      } else if (event.ToolExecutionStart) {
        const { tool_call_id, tool_name, args } = event.ToolExecutionStart;
        const lastEntry = state.entries[state.entries.length - 1];
        if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
           const lastBlock = lastEntry.blocks[lastEntry.blocks.length - 1];
           if (lastBlock && 'isStreaming' in lastBlock && lastBlock.isStreaming) {
              lastBlock.isStreaming = false;
              if (lastBlock.type === 'thinking') {
                lastBlock.endTime = Date.now();
              }
           }
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
      } else if (event.ApprovalRequired) {
        const { prompt_id, tool_name, tool_input, danger_level, explanation } = event.ApprovalRequired;
        const lastEntry = state.entries[state.entries.length - 1];
        if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
           const lastBlock = lastEntry.blocks[lastEntry.blocks.length - 1];
           if (lastBlock && 'isStreaming' in lastBlock && lastBlock.isStreaming) {
              lastBlock.isStreaming = false;
              if (lastBlock.type === 'thinking') {
                lastBlock.endTime = Date.now();
              }
           }
           lastEntry.blocks.push({
             type: 'approval',
             prompt_id,
             tool_name,
             tool_input,
             danger_level,
             explanation,
             status: 'pending'
           });
        }
      } else if (event.AgentEnd) {
        state.isProcessing = false;
        const last = state.entries[state.entries.length - 1];
        if (last && last.type === 'turn' && !last.endTime) {
          last.endTime = Date.now();
        }
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
    toolApprovalResponded: (state, action: PayloadAction<{ promptId: string; approved: boolean }>) => {
      const lastEntry = state.entries[state.entries.length - 1];
      if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
         const block = lastEntry.blocks.find(b => b.type === 'approval' && b.prompt_id === action.payload.promptId);
         if (block && block.type === 'approval') {
           block.status = action.payload.approved ? 'approved' : 'denied';
         }
      }
    },
    clearChat: (state) => {
      state.entries = [];
      state.isProcessing = false;
    }
  },
});

export const { userMessageSent, agentEventReceived, toolApprovalResponded, clearChat } = chatSlice.actions;
export default chatSlice.reducer;
