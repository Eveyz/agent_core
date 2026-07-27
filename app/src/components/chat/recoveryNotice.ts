const REMOTE_RETRY_RE =
  /Failed to connect to remote model \(([^)]+)\), retrying in (\d+)s \(attempt (\d+)\/(\d+)\)/i;

/** Codes that all mean "can't reach the model right now — retrying". */
export const CONNECTION_RETRY_CODES = new Set([
  'model_retry',
  'model_stream_retry',
]);

export function isRemoteConnectionRetry(text: string, code?: string): boolean {
  if (code && CONNECTION_RETRY_CODES.has(code)) return true;
  return REMOTE_RETRY_RE.test(text) || /Failed to connect to remote model/i.test(text);
}

/**
 * Map runtime notice text/code → locale key. Prefer `code` so display does not
 * depend on the backend's English phrasing.
 */
export function translateRecoveryMessage(
  text: string,
  t: (key: string, opts?: Record<string, unknown>) => string,
  code?: string,
  details?: { tokens_before?: number; tokens_after?: number },
): string {
  if (code === 'context_compacted') {
    const legacyCounts = text.match(/(\d+)\s*→\s*(\d+)\s*tokens/i);
    const before = details?.tokens_before ?? (legacyCounts ? Number(legacyCounts[1]) : undefined);
    const after = details?.tokens_after ?? (legacyCounts ? Number(legacyCounts[2]) : undefined);
    if (before !== undefined && after !== undefined) {
      return t('chat.recovery.compacted', {
        before: before.toLocaleString('en-US'),
        after: after.toLocaleString('en-US'),
      });
    }
    return t('chat.recovery.compactedGeneric');
  }

  // Connection retries: calm line, with attempt/delay when the backend includes them.
  if (isRemoteConnectionRetry(text, code)) {
    const detail = text.match(REMOTE_RETRY_RE);
    if (detail) {
      return t('chat.recovery.unreachableRetry', {
        seconds: detail[2],
        attempt: detail[3],
        total: detail[4],
      });
    }
    return t('chat.recovery.unreachable');
  }

  if (code === 'context_compaction_retry') {
    const compactMatch = text.match(/compacting to\s*(\d+)%/i);
    if (compactMatch) {
      return t('chat.recovery.compacting', { percentage: compactMatch[1] });
    }
    return t('chat.recovery.compactingGeneric');
  }

  if (code === 'max_tokens_escalation') {
    const escalateMatch = text.match(/escalating max_tokens to\s*(\d+)/i);
    if (escalateMatch) {
      return t('chat.recovery.escalating', { maxTokens: escalateMatch[1] });
    }
    return t('chat.recovery.escalatingGeneric');
  }

  if (code === 'fallback_model') {
    const switchMatch = text.match(/switching to fallback model:\s*(.*)/i);
    if (switchMatch) {
      return t('chat.recovery.switchingModel', { model: switchMatch[1].trim() });
    }
    return t('chat.recovery.switchingModelGeneric');
  }

  // Legacy / uncoded English messages (older persisted turns).
  const compactMatch = text.match(/context too long;\s*compacting to\s*(\d+)%\s*before retry/i);
  if (compactMatch) {
    return t('chat.recovery.compacting', { percentage: compactMatch[1] });
  }

  const escalateMatch = text.match(/escalating max_tokens to\s*(\d+)/i);
  if (escalateMatch) {
    return t('chat.recovery.escalating', { maxTokens: escalateMatch[1] });
  }

  const delayMatch = text.match(/retrying model call after\s*(\d+)ms/i);
  if (delayMatch) {
    return t('chat.recovery.retryingDelay', { delay: delayMatch[1] });
  }

  const switchMatch = text.match(/switching to fallback model:\s*(.*)/i);
  if (switchMatch) {
    return t('chat.recovery.switchingModel', { model: switchMatch[1].trim() });
  }

  const retryingInMatch = text.match(/retrying in\s*(.*)/i);
  if (retryingInMatch) {
    return t('chat.recovery.retryingIn', { time: retryingInMatch[1] });
  }

  if (text.toLowerCase().includes('retrying model call')) {
    return t('chat.recovery.retrying');
  }

  // Never leak raw English backend copy into a non-English UI when we can
  // tell this is a recovery banner.
  if (code || /retry|compact|fallback|escalat/i.test(text)) {
    return t('chat.recovery.retrying');
  }

  return text;
}

export function isActiveRecoveryNotice(text: string, code?: string): boolean {
  return (
    isRemoteConnectionRetry(text, code) ||
    code === 'context_compaction_retry' ||
    code === 'max_tokens_escalation' ||
    code === 'fallback_model' ||
    /retrying|compacting|escalating|switching to fallback/i.test(text)
  );
}
