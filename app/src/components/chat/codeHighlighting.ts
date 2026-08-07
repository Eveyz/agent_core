import type { BundledLanguage, Highlighter, TokensResult } from 'shiki';

export const CORE_LANGS = [
  'rust', 'typescript', 'javascript', 'python', 'json', 'bash',
  'toml', 'yaml', 'html', 'css', 'sql', 'markdown',
];
const SHIKI_THEMES = { light: 'vitesse-light', dark: 'vitesse-dark' } as const;

function shikiRenderOptions(lang: string) {
  return {
    lang: lang as BundledLanguage,
    themes: SHIKI_THEMES,
    defaultColor: false as const,
    colorsRendering: 'css-vars' as const,
  };
}

let highlighterPromise: Promise<Highlighter> | null = null;

function getShikiHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    highlighterPromise = import('shiki').then(({ createHighlighter }) =>
      createHighlighter({
        themes: [SHIKI_THEMES.dark, SHIKI_THEMES.light],
        langs: CORE_LANGS,
      }),
    );
  }
  return highlighterPromise;
}

const loadingLangs = new Map<string, Promise<void>>();

async function ensureLanguage(highlighter: Highlighter, lang: string): Promise<boolean> {
  const normalized = (lang || 'plaintext').toLowerCase();
  if (normalized === 'plaintext' || normalized === 'text' || normalized === '') return true;
  if (highlighter.getLoadedLanguages().includes(normalized)) return true;
  let inflight = loadingLangs.get(normalized);
  if (!inflight) {
    inflight = highlighter
      .loadLanguage(normalized as BundledLanguage)
      .then(() => undefined)
      .catch(() => undefined);
    loadingLangs.set(normalized, inflight);
  }
  await inflight;
  return highlighter.getLoadedLanguages().includes(normalized);
}

type HighlightJob =
  | { format: 'html'; code: string; lang: string; resolve: (html: string) => void }
  | { format: 'tokens'; code: string; lang: string; resolve: (tokens: TokensResult) => void };

const HIGHLIGHT_BATCH_SIZE = 3;
let highlightQueue: HighlightJob[] = [];
let drainScheduled = false;

function escapeHtml(code: string): string {
  return code.replace(/[&<>"']/g, (character) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#039;',
  })[character] ?? character);
}

function plainHtml(code: string): string {
  return `<pre><code>${escapeHtml(code)}</code></pre>`;
}

function plainTokens(code: string): TokensResult {
  return {
    bg: 'transparent',
    fg: 'inherit',
    tokens: code.split('\n').map((line) => [{ content: line, offset: 0 }]),
  };
}

function runDrain(): void {
  drainScheduled = false;
  const batch = highlightQueue.splice(0, HIGHLIGHT_BATCH_SIZE);
  if (batch.length === 0) return;

  getShikiHighlighter().then(async (highlighter) => {
    for (const job of batch) {
      try {
        const normalized = (job.lang || 'plaintext').toLowerCase();
        const supported = await ensureLanguage(highlighter, normalized);
        if (job.format === 'html') {
          job.resolve(supported
            ? highlighter.codeToHtml(job.code, shikiRenderOptions(normalized))
            : plainHtml(job.code));
        } else {
          job.resolve(supported
            ? highlighter.codeToTokens(job.code, shikiRenderOptions(normalized))
            : plainTokens(job.code));
        }
      } catch (error) {
        console.error('Shiki highlighting error:', error);
        if (job.format === 'html') job.resolve(plainHtml(job.code));
        else job.resolve(plainTokens(job.code));
      }
    }
    if (highlightQueue.length > 0) scheduleDrain();
  });
}

function scheduleDrain(): void {
  if (drainScheduled) return;
  drainScheduled = true;
  const requestIdle = (window as unknown as {
    requestIdleCallback?: (callback: () => void, options?: { timeout: number }) => number;
  }).requestIdleCallback;
  if (typeof requestIdle === 'function') requestIdle(runDrain, { timeout: 200 });
  else setTimeout(runDrain, 16);
}

export function highlightCode(code: string, lang: string): Promise<string> {
  return new Promise<string>((resolve) => {
    highlightQueue.push({ format: 'html', code, lang, resolve });
    scheduleDrain();
  });
}

export function highlightCodeTokens(code: string, lang: string): Promise<TokensResult> {
  return new Promise<TokensResult>((resolve) => {
    highlightQueue.push({ format: 'tokens', code, lang, resolve });
    scheduleDrain();
  });
}
