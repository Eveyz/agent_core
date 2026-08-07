export const STREAMDOWN_ASSISTANT_STORAGE_KEY = 'agent_core_streamdown_assistant';

function parseBooleanFlag(value: string | null | undefined): boolean | undefined {
  if (value === 'true') return true;
  if (value === 'false') return false;
  return undefined;
}

export function resolveStreamdownAssistantFlag(
  buildValue: string | undefined,
  localValue: string | null,
): boolean {
  return parseBooleanFlag(localValue) ?? parseBooleanFlag(buildValue) ?? false;
}

export function isStreamdownAssistantEnabled(): boolean {
  const localValue = typeof window === 'undefined'
    ? null
    : window.localStorage.getItem(STREAMDOWN_ASSISTANT_STORAGE_KEY);
  return resolveStreamdownAssistantFlag(
    import.meta.env.VITE_STREAMDOWN_ASSISTANT,
    localValue,
  );
}
