import { createSlice, PayloadAction } from '@reduxjs/toolkit';
import { resumeSession } from '../project/projectSlice';

export type TurnBlock =
  | { type: 'assistant'; text: string; isStreaming: boolean }
  | { type: 'thinking'; text: string; isStreaming: boolean; startTime?: number; endTime?: number }
  | { type: 'tool'; call_id: string; name: string; args?: any; result: string; active: boolean; is_error: boolean }
  | { type: 'approval'; prompt_id: string; tool_name: string; tool_input: any; danger_level: string; explanation: string; status: 'pending' | 'approved' | 'denied' }
  | { type: 'error'; text: string }
  | { type: 'subagent_ref'; subagent_id: string };

export interface SubagentBlock {
  type: 'assistant' | 'thinking' | 'tool' | 'approval' | 'error';
  text?: string;
  isStreaming?: boolean;
  startTime?: number;
  endTime?: number;
  call_id?: string;
  name?: string;
  args?: any;
  result?: string;
  active?: boolean;
  is_error?: boolean;
  prompt_id?: string;
  tool_name?: string;
  tool_input?: any;
  danger_level?: string;
  explanation?: string;
  status?: 'pending' | 'approved' | 'denied';
}

export interface SubagentEntry {
  id: string;
  role_name?: string;
  task: string;
  status: 'working' | 'done' | 'error';
  iterations_used?: number;
  blocks: SubagentBlock[];
  startTime: number;
  endTime?: number;
}

export interface ChatEntry {
  id: string;
  type: 'user' | 'turn';
  turnIndex?: number;
  text?: string;       // for user
  blocks?: TurnBlock[];// for turn
  startTime?: number;  // timestamp for TurnStart
  endTime?: number;    // timestamp for TurnEnd
  subagents?: Record<string, SubagentEntry>;
}

interface ChatState {
  entries: ChatEntry[];
  isProcessing: boolean;
  entriesBySession: Record<string, ChatEntry[]>;
  processingBySession: Record<string, boolean>;
  _resumedFromBackend: boolean;
}

const initialState: ChatState = {
  entries: [],
  isProcessing: false,
  entriesBySession: {},
  processingBySession: {},
  _resumedFromBackend: false,
};

function getActiveTurn(state: ChatState): ChatEntry | undefined {
  for (let i = state.entries.length - 1; i >= 0; i--) {
    const entry = state.entries[i];
    if (entry.type === 'turn' && !entry.endTime) {
      return entry;
    }
  }
  return undefined;
}

function getOrCreateSubagent(entry: ChatEntry, subagentId: string, roleName: string, task: string): SubagentEntry {
  if (!entry.subagents) entry.subagents = {};
  if (!entry.subagents[subagentId]) {
    entry.subagents[subagentId] = {
      id: subagentId,
      role_name: roleName,
      task,
      status: 'working',
      blocks: [],
      startTime: Date.now(),
    };
  }
  return entry.subagents[subagentId];
}

export const chatSlice = createSlice({
  name: 'chat',
  initialState,
  reducers: {
    cacheCurrentSession: (state, action: PayloadAction<string>) => {
      const sessionId = action.payload;
      state.entriesBySession[sessionId] = state.entries;
      state.processingBySession[sessionId] = state.isProcessing;
    },
    restoreOrClearSession: (state, action: PayloadAction<string>) => {
      const sessionId = action.payload;
      const cached = state.entriesBySession[sessionId];
      if (cached) {
        state.entries = cached;
        state.isProcessing = state.processingBySession[sessionId] ?? false;
      } else {
        state.entries = [];
        state.isProcessing = false;
      }
      state._resumedFromBackend = false;
    },
    userMessageSent: (state, action: PayloadAction<string>) => {
      state.entries.push({
        id: `user-${Date.now()}`,
        type: 'user',
        text: action.payload,
      });
      state.isProcessing = true;
      state._resumedFromBackend = false;
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
             args: args,
             result: '',
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
             block.result += typeof partial_result === 'string' ? partial_result : JSON.stringify(partial_result);
           }
        }
      } else if (event.ToolExecutionEnd) {
        const { tool_call_id, result, is_error } = event.ToolExecutionEnd;
        const lastEntry = state.entries[state.entries.length - 1];
         if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
            const block = lastEntry.blocks.find(b => b.type === 'tool' && b.call_id === tool_call_id);
            if (block && block.type === 'tool') {
              let finalResult = typeof result === 'string' ? result : JSON.stringify(result);
              if (finalResult.length > 5000) {
                 finalResult = finalResult.substring(0, 5000) + `\n\n... [Truncated ${finalResult.length - 5000} characters for performance]`;
              }
              block.result = finalResult;
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

      // ── Subagent events ──
      else if (event.SubagentStart) {
        const { subagent_id, role_name, task } = event.SubagentStart;
        if (typeof task !== 'string') {
          console.warn('[SubagentStart] task is not a string:', task, 'event:', event.SubagentStart);
        }
        const safeTask = typeof task === 'string' ? task : JSON.stringify(task);
        const safeRoleName = typeof role_name === 'string' ? role_name : String(subagent_id);
        const turn = getActiveTurn(state);
        if (turn) {
          getOrCreateSubagent(turn, subagent_id, safeRoleName, safeTask);
          if (turn.blocks) {
            turn.blocks.push({ type: 'subagent_ref', subagent_id });
          }
        }
      } else if (event.SubagentMessageUpdate) {
        const { subagent_id, delta } = event.SubagentMessageUpdate;
        const turn = getActiveTurn(state);
        if (turn && turn.subagents && turn.subagents[subagent_id]) {
          const sa = turn.subagents[subagent_id];
          if (typeof delta.Text === 'string') {
            let block = sa.blocks[sa.blocks.length - 1];
            if (!block || block.type !== 'assistant' || !block.isStreaming) {
              if (block && block.isStreaming) {
                block.isStreaming = false;
                if (block.type === 'thinking') block.endTime = Date.now();
              }
              sa.blocks.push({ type: 'assistant', text: '', isStreaming: true });
              block = sa.blocks[sa.blocks.length - 1];
            }
            if (block.type === 'assistant') block.text += delta.Text;
          } else if (typeof delta.Thinking === 'string') {
            let block = sa.blocks[sa.blocks.length - 1];
            if (!block || block.type !== 'thinking' || !block.isStreaming) {
              if (block && block.isStreaming) block.isStreaming = false;
              sa.blocks.push({ type: 'thinking', text: '', isStreaming: true, startTime: Date.now() });
              block = sa.blocks[sa.blocks.length - 1];
            }
            if (block.type === 'thinking') block.text += delta.Thinking;
          }
        }
      } else if (event.SubagentToolStart) {
        const { subagent_id, tool_call_id, tool_name, args } = event.SubagentToolStart;
        const turn = getActiveTurn(state);
        if (turn && turn.subagents && turn.subagents[subagent_id]) {
          const sa = turn.subagents[subagent_id];
          const lastBlock = sa.blocks[sa.blocks.length - 1];
          if (lastBlock && lastBlock.isStreaming) {
            lastBlock.isStreaming = false;
            if (lastBlock.type === 'thinking') lastBlock.endTime = Date.now();
          }
          sa.blocks.push({
            type: 'tool',
            call_id: tool_call_id,
            name: tool_name,
            args: args,
            result: '',
            active: true,
            is_error: false,
          });
        }
      } else if (event.SubagentToolEnd) {
        const { subagent_id, tool_call_id, result, is_error } = event.SubagentToolEnd;
        const turn = getActiveTurn(state);
        if (turn && turn.subagents && turn.subagents[subagent_id]) {
          const sa = turn.subagents[subagent_id];
          const block = sa.blocks.find(b => b.type === 'tool' && b.call_id === tool_call_id);
          if (block && block.type === 'tool') {
            let finalResult = typeof result === 'string' ? result : JSON.stringify(result);
            if (finalResult.length > 5000) {
               finalResult = finalResult.substring(0, 5000) + `\n\n... [Truncated ${finalResult.length - 5000} characters for performance]`;
            }
            block.result = finalResult;
            block.active = false;
            block.is_error = is_error;
          }
        }
      } else if (event.SubagentApprovalRequired) {
        const { subagent_id, prompt_id, tool_name, tool_input, danger_level, explanation } = event.SubagentApprovalRequired;
        const turn = getActiveTurn(state);
        if (turn && turn.subagents && turn.subagents[subagent_id]) {
          const sa = turn.subagents[subagent_id];
          const lastBlock = sa.blocks[sa.blocks.length - 1];
          if (lastBlock && lastBlock.isStreaming) {
            lastBlock.isStreaming = false;
            if (lastBlock.type === 'thinking') lastBlock.endTime = Date.now();
          }
          sa.blocks.push({
            type: 'approval',
            prompt_id,
            tool_name,
            tool_input,
            danger_level,
            explanation,
            status: 'pending'
          });
        }
      } else if (event.SubagentEnd) {
        const { subagent_id, success, iterations_used } = event.SubagentEnd;
        const turn = getActiveTurn(state);
        if (turn && turn.subagents && turn.subagents[subagent_id]) {
          const sa = turn.subagents[subagent_id];
          sa.status = success ? 'done' : 'error';
          sa.iterations_used = iterations_used;
          sa.endTime = Date.now();
          // Close any still-streaming blocks
          sa.blocks.forEach(b => {
            if (b.isStreaming) {
              b.isStreaming = false;
              if (b.type === 'thinking') b.endTime = Date.now();
            }
          });
        }
      }
    },
    toolApprovalResponded: (state, action: PayloadAction<{ promptId: string; approved: boolean }>) => {
      const lastEntry = state.entries[state.entries.length - 1];
      if (lastEntry && lastEntry.type === 'turn') {
         if (lastEntry.blocks) {
           const block = lastEntry.blocks.find(b => b.type === 'approval' && b.prompt_id === action.payload.promptId);
           if (block && block.type === 'approval') {
             block.status = action.payload.approved ? 'approved' : 'denied';
             return;
           }
         }
         if (lastEntry.subagents) {
           for (const sa of Object.values(lastEntry.subagents)) {
             if (sa.blocks) {
               const saBlock = sa.blocks.find(b => b.type === 'approval' && b.prompt_id === action.payload.promptId);
               if (saBlock && saBlock.type === 'approval') {
                 saBlock.status = action.payload.approved ? 'approved' : 'denied';
                 return;
               }
             }
           }
         }
      }
    },
    clearChat: (state) => {
      state.entries = [];
      state.isProcessing = false;
    },
    retryFromEntry: (state, action: PayloadAction<string>) => {
      // Remove the target user entry and everything after it, then re-add the user message
      const entryId = action.payload;
      const idx = state.entries.findIndex(e => e.id === entryId);
      if (idx === -1) return;
      const userText = state.entries[idx].text ?? '';
      state.entries = state.entries.slice(0, idx);
      state.entries.push({
        id: `user-${Date.now()}`,
        type: 'user',
        text: userText,
      });
      state.isProcessing = true;
      state._resumedFromBackend = false;
    },
  },
  extraReducers: (builder) => {
    builder.addCase(resumeSession.fulfilled, (state, action) => {
      // If we already restored from cache, don't overwrite
      if (state.entries.length > 0) return;
      const { messages, event_log } = action.payload;
      console.log('[resumeSession] messages:', messages.length, 'event_log:', event_log?.length ?? 0);
      state.entries = [];
      state.isProcessing = false;

      let assistantIdx = 0;
      for (const msg of messages) {
        if (msg.role === 'user') {
          state.entries.push({
            id: `user-${Date.now()}-${Math.random()}`,
            type: 'user',
            text: msg.content,
          });
        } else if (msg.role === 'assistant') {
          const turnIdx = assistantIdx;
          assistantIdx++;
          const blocks: TurnBlock[] = [];

          // Add blocks from event log in their original chronological order
          if (event_log && Array.isArray(event_log)) {
            const turnEvents = event_log.filter((e: any) => e.turn_index === turnIdx && (e.event_type === 'tool_call' || e.event_type === 'subagent' || e.event_type === 'thinking' || e.event_type === 'assistant'));
            for (const ev of turnEvents) {
              const payload = (ev.payload && typeof ev.payload === 'object' && !Array.isArray(ev.payload)) ? ev.payload : {};
              if (ev.event_type === 'tool_call') {
                blocks.push({
                  type: 'tool',
                  call_id: `restored-${Math.random()}`,
                  name: payload.name ?? 'unknown',
                  args: payload.args ?? undefined,
                  result: payload.args_summary ?? '',
                  active: false,
                  is_error: !!payload.is_error,
                });
              } else if (ev.event_type === 'subagent') {
                const subId = payload.id;
                if (subId) {
                  blocks.push({
                    type: 'subagent_ref',
                    subagent_id: subId,
                  });
                }
              } else if (ev.event_type === 'thinking') {
                blocks.push({
                  type: 'thinking',
                  text: payload.text ?? '',
                  isStreaming: false,
                  startTime: payload.startTime,
                  endTime: payload.endTime,
                });
              } else if (ev.event_type === 'assistant') {
                blocks.push({
                  type: 'assistant',
                  text: payload.text ?? '',
                  isStreaming: false,
                });
              }
            }
          }

          // Backward compatibility for old sessions:
          // If no 'assistant' block was restored from event_log, add a single assistant block with msg.content
          if (!blocks.some(b => b.type === 'assistant')) {
            blocks.push({ type: 'assistant', text: msg.content, isStreaming: false });
          }

          let startTime: number | undefined = undefined;
          let endTime: number | undefined = undefined;
          if (event_log && Array.isArray(event_log)) {
            const metaEvent = event_log.find((e: any) => e.turn_index === turnIdx && e.event_type === 'turn_meta');
            if (metaEvent && metaEvent.payload) {
              startTime = metaEvent.payload.startTime;
              endTime = metaEvent.payload.endTime;
            }
          }

          let subagents: Record<string, SubagentEntry> | undefined = undefined;
          if (event_log && Array.isArray(event_log)) {
            const subEvents = event_log.filter((e: any) => e.turn_index === turnIdx && e.event_type === 'subagent');
            if (subEvents.length > 0) {
              subagents = {};
              for (const ev of subEvents) {
                const payload = (ev.payload && typeof ev.payload === 'object' && !Array.isArray(ev.payload)) ? ev.payload : {};
                if (payload.id) {
                  subagents[payload.id] = payload as SubagentEntry;
                }
              }
            }
          }

          state.entries.push({
            id: `turn-${turnIdx}-${Date.now()}`,
            type: 'turn',
            turnIndex: turnIdx,
            blocks,
            subagents,
            startTime,
            endTime,
          });
        }
      }

      console.log('[resumeSession] entries built:', state.entries.length);
      // Mark as "already saved" so the AgentEnd save effect doesn't re-save
      state._resumedFromBackend = true;
    });
  },
});

export const { userMessageSent, agentEventReceived, toolApprovalResponded, clearChat, cacheCurrentSession, restoreOrClearSession, retryFromEntry } = chatSlice.actions;
export default chatSlice.reducer;

// ── Helpers ──────────────────────────────────────────────────────────

export function entriesToMessages(entries: ChatEntry[]): import('../project/projectSlice').FrontendMessage[] {
  const msgs: import('../project/projectSlice').FrontendMessage[] = [];
  for (const entry of entries) {
    if (entry.type === 'user' && entry.text) {
      msgs.push({ role: 'user', content: entry.text });
    } else if (entry.type === 'turn' && entry.blocks) {
      let assistantText = '';
      for (const block of entry.blocks) {
        if (block.type === 'assistant') {
          assistantText += block.text;
        }
      }
      if (assistantText.trim()) {
        msgs.push({ role: 'assistant', content: assistantText.trim() });
      }
    }
  }
  return msgs;
}

export function entriesToEventLog(entries: ChatEntry[]): { eventLog: any[], processTimeMs: number, thoughtTimeMs: number } {
  const eventLog: any[] = [];
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

      if (entry.startTime || entry.endTime) {
        eventLog.push({
          turn_index: assistantIdx,
          event_type: 'turn_meta',
          payload: { startTime: entry.startTime, endTime: entry.endTime },
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
          const sa = entry.subagents?.[b.subagent_id];
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
