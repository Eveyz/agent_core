import type { FrontendPrompt, PlanDetail, TodoItem } from '../../features/chat/types';
import { truncateText } from './overviewSubagents';

export type PromptTodoGroup = {
  promptId: string | null;
  turnIndex: number;
  userPreview: string;
  plans: PlanDetail[];
};

export function planProgress(items: TodoItem[]): { completed: number; total: number } {
  const total = items.length;
  const completed = items.filter((i) => i.status === 'completed').length;
  return { completed, total };
}

export function countPlanItems(plans: PlanDetail[]): number {
  return plans.reduce((n, p) => n + (p.items?.length ?? 0), 0);
}

function userPreviewFromPrompt(prompt: FrontendPrompt): string {
  const userMsg = prompt.messages.find((m) => m.role === 'user');
  const text = typeof userMsg?.content === 'string' ? userMsg.content : '';
  return truncateText(text || 'Untitled prompt', 60);
}

/**
 * Group session plans under the prompt that created them (`source_prompt_id`).
 * Plans without a prompt id go under a "Current" bucket (promptId null).
 * Prompt groups are ordered by turn_index ascending; Current last.
 */
export function groupPlansByPrompt(
  plans: PlanDetail[],
  prompts: FrontendPrompt[],
): PromptTodoGroup[] {
  if (!plans.length) return [];

  const promptById = new Map(prompts.map((p) => [p.id, p]));
  const byPrompt = new Map<string | null, PlanDetail[]>();

  for (const plan of plans) {
    const key = plan.source_prompt_id?.trim() ? plan.source_prompt_id : null;
    const list = byPrompt.get(key) ?? [];
    list.push(plan);
    byPrompt.set(key, list);
  }

  const groups: PromptTodoGroup[] = [];

  // Known prompts in turn order
  const orderedPromptIds = [...prompts]
    .sort((a, b) => a.turn_index - b.turn_index)
    .map((p) => p.id);

  for (const promptId of orderedPromptIds) {
    const list = byPrompt.get(promptId);
    if (!list?.length) continue;
    const prompt = promptById.get(promptId)!;
    groups.push({
      promptId,
      turnIndex: prompt.turn_index,
      userPreview: userPreviewFromPrompt(prompt),
      plans: list,
    });
    byPrompt.delete(promptId);
  }

  // Orphan prompt ids (plan references a prompt not in allPrompts)
  for (const [promptId, list] of [...byPrompt.entries()]) {
    if (promptId === null) continue;
    groups.push({
      promptId,
      turnIndex: Number.MAX_SAFE_INTEGER - 1,
      userPreview: truncateText(promptId, 60),
      plans: list,
    });
    byPrompt.delete(promptId);
  }

  // Current / unscoped
  const current = byPrompt.get(null);
  if (current?.length) {
    groups.push({
      promptId: null,
      turnIndex: Number.MAX_SAFE_INTEGER,
      userPreview: 'Current',
      plans: current,
    });
  }

  return groups;
}
