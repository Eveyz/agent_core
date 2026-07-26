import type { WorkflowMentionPayload } from './imageAttachments';

export type SelectedWorkflowMention = WorkflowMentionPayload & { token: string };

function containsMentionToken(input: string, token: string): boolean {
  const escaped = token.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(
    `(^|\\s)${escaped}(?=\\s|$|[.,!?，。！？:：;；])`,
    'u',
  ).test(input);
}

export function resolveWorkflowMentions(
  input: string,
  selected: SelectedWorkflowMention[],
): WorkflowMentionPayload[] {
  const resolved = new Map<string, WorkflowMentionPayload>();
  for (const mention of selected) {
    if (!containsMentionToken(input, mention.token)) continue;
    resolved.set(mention.workflow_id, {
      workflow_id: mention.workflow_id,
      revision_id: mention.revision_id,
      scope: mention.scope,
      display_token: mention.display_token,
      optional: mention.optional,
    });
  }
  return [...resolved.values()];
}
