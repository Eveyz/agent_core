const PROVIDER_UNAVAILABLE_RE =
  /AI provider is temporarily unavailable after repeated failures\. Try again in about (\d+)s/i;

/** Map runtime error text → locale key. Match stable English backend phrasing. */
export function translateErrorMessage(
  text: string,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string {
  const withSecs = text.match(PROVIDER_UNAVAILABLE_RE);
  if (withSecs) {
    return t('chat.errors.providerUnavailable', { seconds: withSecs[1] });
  }

  if (
    /AI provider is temporarily unavailable after repeated failures/i.test(text) ||
    /circuit breaker/i.test(text)
  ) {
    return t('chat.errors.providerUnavailableGeneric');
  }

  return text;
}
