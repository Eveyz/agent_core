// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { extractMath, restoreMath } from './markdownMath';
import { parseMarkdown } from './MarkdownContent';

describe('markdownMath', () => {
  it('renders inline and display TeX', () => {
    const { text, htmls } = extractMath('Energy $E=mc^2$ and\n\n$$\\int_0^1 x\\,dx$$\n');
    expect(htmls).toHaveLength(2);
    expect(htmls[0]).toContain('katex');
    expect(htmls[1]).toContain('md-math-display');
    expect(text).not.toContain('E=mc^2');
    expect(restoreMath(text, htmls)).toContain('katex');
  });

  it('supports \\( \\) and \\[ \\] delimiters', () => {
    const { htmls } = extractMath('Inline \\(a+b\\) and display \\[c+d\\]');
    expect(htmls).toHaveLength(2);
    expect(htmls[0]).toContain('katex');
    expect(htmls[1]).toContain('md-math-display');
  });

  it('does not treat currency or code as math', () => {
    const { text, htmls } = extractMath('Price $12.99 and `cost $x$` and\n\n```js\nconst a = "$y$"\n```\n');
    expect(htmls).toHaveLength(0);
    expect(text).toContain('$12.99');
    expect(text).toContain('`cost $x$`');
    expect(text).toContain('"$y$"');
  });
});

describe('parseMarkdown math integration', () => {
  it('injects KaTeX HTML after sanitization', () => {
    const { __html } = parseMarkdown('The mean is $\\mu$ and variance $\\sigma^2$.');
    expect(__html).toContain('class="katex"');
    expect(__html).toContain('μ');
  });

  it('renders display math in tables-friendly markdown', () => {
    const { __html } = parseMarkdown('| Dist | PDF |\n| --- | --- |\n| Normal | $$\\frac{1}{\\sigma}$$ |');
    expect(__html).toContain('md-math-display');
    expect(__html).toContain('katex');
  });
});
