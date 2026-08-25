import type { AgentDef } from '../../features/agents/types';
import type { AgentMentionPayload } from './imageAttachments';

export type SelectedAgentMention = AgentMentionPayload & { token: string };

function tokenForAgent(agent: AgentDef): string {
  return `@${agent.name.trim().replace(/\s+/g, '_')}`;
}

export function insertAgentMentionToken(input: string, agent: AgentDef): string {
  const token = tokenForAgent(agent);
  if (/(?:^|\s)@[^\s]*$/u.test(input)) {
    return input.replace(/@[^\s]*$/u, `${token} `);
  }
  const separator = input.length > 0 && !/\s$/u.test(input) ? " " : "";
  return `${input}${separator}${token} `;
}

function containsMentionToken(input: string, token: string): boolean {
  const escaped = token.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(
    `(^|\\s)${escaped}(?=\\s|$|[.,!?，。！？:：;；])`,
    'u',
  ).test(input);
}

/**
 * Resolve composer state into the structured manifest sent to Tauri.
 *
 * Autocomplete selections remain authoritative. A manually typed token is
 * accepted only when it maps to exactly one saved agent, so a visible
 * `@Agent` never silently degrades into an ordinary text mention.
 */
export function resolveAgentMentions(
  input: string,
  selected: SelectedAgentMention[],
  agents: AgentDef[],
): AgentMentionPayload[] {
  const resolved = new Map<string, AgentMentionPayload>();

  for (const mention of selected) {
    if (containsMentionToken(input, mention.token)) {
      resolved.set(mention.agent_id, {
        agent_id: mention.agent_id,
        revision_id: mention.revision_id,
        optional: mention.optional,
      });
    }
  }

  const agentsByToken = new Map<string, AgentDef[]>();
  for (const agent of agents) {
    const token = tokenForAgent(agent);
    const matches = agentsByToken.get(token) ?? [];
    matches.push(agent);
    agentsByToken.set(token, matches);
  }
  for (const [token, matches] of agentsByToken) {
    if (matches.length !== 1 || !containsMentionToken(input, token)) continue;
    const agent = matches[0];
    resolved.set(agent.id, {
      agent_id: agent.id,
      revision_id: agent.updated_at,
    });
  }

  return [...resolved.values()];
}
