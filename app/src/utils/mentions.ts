/**
 * Parse @mentions from text. Returns array of { type, value }.
 */
export function parseMentions(text: string): Array<{ type: 'text' | 'mention'; value: string }> {
  const tokens: Array<{ type: 'text' | 'mention'; value: string }> = [];
  let lastIndex = 0;
  const regex = /@[^\s]+/g;
  let match: RegExpExecArray | null;
  while ((match = regex.exec(text)) !== null) {
    if (match.index > lastIndex) {
      tokens.push({ type: 'text', value: text.slice(lastIndex, match.index) });
    }
    tokens.push({ type: 'mention', value: match[0] });
    lastIndex = match.index + match[0].length;
  }
  if (lastIndex < text.length) {
    tokens.push({ type: 'text', value: text.slice(lastIndex) });
  }
  if (tokens.length === 0 && text) {
    tokens.push({ type: 'text', value: text });
  }
  return tokens;
}

/**
 * Find the mention token that contains or is adjacent to position.
 * Returns [start, end] of the mention, or null.
 */
export function findMentionBoundaries(text: string, pos: number): [number, number] | null {
  const regex = /@[^\s]+/g;
  let match: RegExpExecArray | null;
  while ((match = regex.exec(text)) !== null) {
    const start = match.index;
    const end = start + match[0].length;
    if (pos > start && pos <= end) {
      return [start, end];
    }
  }
  return null;
}
