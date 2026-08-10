import katex from 'katex';

/** Private-use placeholders that survive marked + DOMPurify as plain text. */
const MATH_START = '\uE000';
const MATH_END = '\uE001';
const PROTECT_START = '\uE010';
const PROTECT_END = '\uE011';

const FENCED_CODE_RE = /(?:^|\n)(```|~~~)[^\n]*\n[\s\S]*?(?:\n\1[^\S\n]*(?:\n|$)|$)/g;
const INLINE_CODE_RE = /`+[^`]+`+/g;

export interface MathExtraction {
  text: string;
  htmls: string[];
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/** Shield fenced then inline code so `$` inside them is left alone. */
function protectCode(source: string): { text: string; parts: string[] } {
  const parts: string[] = [];
  const replace = (match: string) => {
    const index = parts.length;
    parts.push(match);
    return `${PROTECT_START}${index}${PROTECT_END}`;
  };
  const withoutFences = source.replace(FENCED_CODE_RE, replace);
  const text = withoutFences.replace(INLINE_CODE_RE, replace);
  return { text, parts };
}

function restoreProtected(text: string, parts: string[]): string {
  return text.replace(
    new RegExp(`${PROTECT_START}(\\d+)${PROTECT_END}`, 'g'),
    (_match, index: string) => parts[Number(index)] ?? '',
  );
}

function renderTex(tex: string, displayMode: boolean): string {
  try {
    const html = katex.renderToString(tex, {
      displayMode,
      throwOnError: false,
      strict: 'ignore',
      output: 'html',
      trust: false,
    });
    return displayMode
      ? `<span class="md-math-display">${html}</span>`
      : html;
  } catch {
    const cls = displayMode ? 'md-math-display md-math-error' : 'md-math-error';
    return `<span class="${cls}"><code>${escapeHtml(tex)}</code></span>`;
  }
}

function isEscaped(source: string, index: number): boolean {
  let slashes = 0;
  for (let i = index - 1; i >= 0 && source[i] === '\\'; i -= 1) slashes += 1;
  return slashes % 2 === 1;
}

function findClosing(
  source: string,
  start: number,
  closer: string,
  allowNewlines: boolean,
): number {
  for (let i = start; i < source.length; i += 1) {
    if (!allowNewlines && source[i] === '\n') return -1;
    if (source.startsWith(closer, i) && !isEscaped(source, i)) return i;
  }
  return -1;
}

/**
 * Replace TeX delimiters with placeholders and collect KaTeX HTML.
 * Skips fenced / inline code. Supports $$ $$ , \[ \], \( \), and $ $.
 */
export function extractMath(markdown: string): MathExtraction {
  const protectedCode = protectCode(markdown);
  const source = protectedCode.text;
  const htmls: string[] = [];
  let out = '';
  let i = 0;

  while (i < source.length) {
    if (source.startsWith('$$', i) && !isEscaped(source, i)) {
      const close = findClosing(source, i + 2, '$$', true);
      if (close !== -1) {
        const tex = source.slice(i + 2, close).trim();
        const index = htmls.length;
        htmls.push(renderTex(tex, true));
        out += `${MATH_START}${index}${MATH_END}`;
        i = close + 2;
        continue;
      }
    }

    if (source.startsWith('\\[', i) && !isEscaped(source, i)) {
      const close = findClosing(source, i + 2, '\\]', true);
      if (close !== -1) {
        const tex = source.slice(i + 2, close).trim();
        const index = htmls.length;
        htmls.push(renderTex(tex, true));
        out += `${MATH_START}${index}${MATH_END}`;
        i = close + 2;
        continue;
      }
    }

    if (source.startsWith('\\(', i) && !isEscaped(source, i)) {
      const close = findClosing(source, i + 2, '\\)', false);
      if (close !== -1) {
        const tex = source.slice(i + 2, close).trim();
        const index = htmls.length;
        htmls.push(renderTex(tex, false));
        out += `${MATH_START}${index}${MATH_END}`;
        i = close + 2;
        continue;
      }
    }

    if (
      source[i] === '$'
      && source[i + 1] !== '$'
      && !isEscaped(source, i)
      // Avoid currency like $5 or $12.99
      && source[i + 1] !== undefined
      && !/\d/.test(source[i + 1]!)
    ) {
      const close = findClosing(source, i + 1, '$', false);
      if (close !== -1 && close > i + 1) {
        const tex = source.slice(i + 1, close).trim();
        if (tex && !tex.includes('\n')) {
          const index = htmls.length;
          htmls.push(renderTex(tex, false));
          out += `${MATH_START}${index}${MATH_END}`;
          i = close + 1;
          continue;
        }
      }
    }

    out += source[i];
    i += 1;
  }

  return {
    text: restoreProtected(out, protectedCode.parts),
    htmls,
  };
}

/** Swap math placeholders for trusted KaTeX HTML after sanitization. */
export function restoreMath(html: string, htmls: string[]): string {
  if (htmls.length === 0) return html;
  return html.replace(
    new RegExp(`${MATH_START}(\\d+)${MATH_END}`, 'g'),
    (_match, index: string) => htmls[Number(index)] ?? '',
  );
}
