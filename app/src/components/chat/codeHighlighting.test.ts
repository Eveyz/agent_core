import { describe, expect, it } from 'vitest';
import { normalizeHighlightSource } from './codeHighlighting';

describe('normalizeHighlightSource', () => {
  it('strips leading and trailing blank lines from a one-line result', () => {
    expect(normalizeHighlightSource('2\n\n\n')).toBe('2');
    expect(normalizeHighlightSource('\n\n2\n')).toBe('2');
  });

  it('keeps internal blank lines', () => {
    expect(normalizeHighlightSource('a\n\nb\n')).toBe('a\n\nb');
  });
});
