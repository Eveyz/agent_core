import React, { useState, useEffect } from 'react';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import { createHighlighter, type Highlighter, type BundledLanguage } from 'shiki';

// Languages loaded eagerly on first use. These cover the vast majority of
// code blocks; any other language is loaded on demand the first time it
// appears. Loading every grammar up front was a major contributor to the
// first-open freeze — a smaller core set keeps cold-start short.
const CORE_LANGS = [
  'rust', 'typescript', 'javascript', 'python', 'json', 'bash',
  'toml', 'yaml', 'html', 'css', 'sql', 'markdown',
];

let highlighterPromise: ReturnType<typeof createHighlighter> | null = null;

function getShikiHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighter({
      themes: ['github-dark', 'github-light'],
      langs: CORE_LANGS,
    });
  }
  return highlighterPromise;
}

// Track in-flight language loads so concurrent code blocks for the same
// (rare) language don't trigger duplicate grammar fetches.
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

// ── Co-operative highlighting queue ──────────────────────────────────
// codeToHtml is synchronous and CPU-bound. Restoring a long session mounts
// dozens of CodeBlocks at once; if they all highlight back-to-back the main
// thread blocks and the UI freezes. Instead we process a small batch per
// idle frame and yield in between, so buttons stay responsive while the
// session is still being syntax-highlighted.
type HighlightJob = { code: string; lang: string; resolve: (html: string) => void };

const HIGHLIGHT_BATCH_SIZE = 3;
let highlightQueue: HighlightJob[] = [];
let drainScheduled = false;

function plainHtml(code: string): string {
  return `<pre><code>${escapeHtml(code)}</code></pre>`;
}

function runDrain(): void {
  // Reset the flag first so jobs enqueued while this batch runs can
  // schedule a fresh drain. Previously the flag was never cleared here,
  // which stalled the queue after the first batch — leaving every code
  // block past the first three stuck in the unstyled loading fallback.
  drainScheduled = false;
  const batch = highlightQueue.splice(0, HIGHLIGHT_BATCH_SIZE);
  if (batch.length === 0) {
    return;
  }
  getShikiHighlighter().then(async (highlighter) => {
    for (const job of batch) {
      try {
        const ok = await ensureLanguage(highlighter, job.lang);
        const html = ok
          ? highlighter.codeToHtml(job.code, {
              lang: (job.lang || 'plaintext') as BundledLanguage,
              themes: { light: 'github-light', dark: 'github-dark' },
              defaultColor: false,
              colorsRendering: 'css-vars',
            })
          : plainHtml(job.code);
        job.resolve(html);
      } catch (error) {
        console.error('Shiki highlighting error:', error);
        job.resolve(plainHtml(job.code));
      }
    }
    if (highlightQueue.length > 0) scheduleDrain();
  });
}

function scheduleDrain(): void {
  if (drainScheduled) return;
  drainScheduled = true;
  const ric = (window as unknown as {
    requestIdleCallback?: (cb: () => void, opts?: { timeout: number }) => number;
  }).requestIdleCallback;
  if (typeof ric === 'function') {
    ric(runDrain, { timeout: 200 });
  } else {
    setTimeout(runDrain, 16);
  }
}

function enqueueHighlight(code: string, lang: string): Promise<string> {
  return new Promise<string>((resolve) => {
    highlightQueue.push({ code, lang, resolve });
    scheduleDrain();
  });
}

// Language display name mapping
const LANGUAGE_NAMES: Record<string, string> = {
  rust: 'Rust',
  python: 'Python',
  javascript: 'JavaScript',
  typescript: 'TypeScript',
  json: 'JSON',
  toml: 'TOML',
  yaml: 'YAML',
  yml: 'YAML',
  html: 'HTML',
  css: 'CSS',
  sql: 'SQL',
  bash: 'Bash',
  sh: 'Shell',
  shell: 'Shell',
  c: 'C',
  cpp: 'C++',
  'c++': 'C++',
  java: 'Java',
  go: 'Go',
  golang: 'Go',
  ruby: 'Ruby',
  php: 'PHP',
  swift: 'Swift',
  kotlin: 'Kotlin',
  scala: 'Scala',
  r: 'R',
  lua: 'Lua',
  perl: 'Perl',
  elixir: 'Elixir',
  erlang: 'Erlang',
  haskell: 'Haskell',
  clojure: 'Clojure',
  dart: 'Dart',
  protobuf: 'Protocol Buffer',
  proto: 'Protocol Buffer',
  xml: 'XML',
  markdown: 'Markdown',
  md: 'Markdown',
  makefile: 'Makefile',
  make: 'Makefile',
  cmake: 'CMake',
  dockerfile: 'Dockerfile',
  docker: 'Dockerfile',
  diff: 'Diff',
  'git-commit': 'Git Commit',
  git: 'Git',
  ini: 'INI',
  cfg: 'INI',
  conf: 'INI',
};

interface CodeBlockProps {
  code: string;
  language: string;
  className?: string;
}

export const CodeBlock: React.FC<CodeBlockProps> = ({ code, language, className = '' }) => {
  const [highlightedCode, setHighlightedCode] = useState<string>('');
  const [copied, setCopied] = useState(false);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let mounted = true;
    setLoading(true);

    const lang = language.toLowerCase() || 'plaintext';
    enqueueHighlight(code, lang).then((html) => {
      if (mounted) {
        setHighlightedCode(html);
        setLoading(false);
      }
    });

    return () => {
      mounted = false;
    };
  }, [code, language]);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (error) {
      console.error('Failed to copy:', error);
    }
  };

  const displayName = LANGUAGE_NAMES[language.toLowerCase()] || language || 'Plain Text';

  if (loading) {
    return (
      <div className={`code-block-wrapper ${className}`}>
        <div className="code-block-header">
          <span className="code-block-language">{displayName}</span>
          <button className="code-block-copy-btn" onClick={handleCopy}>
            {copied ? <CheckIcon size={14} color="var(--success)" /> : <CopyIcon size={14} />}
          </button>
        </div>
        <div className="code-block-content">
          <pre><code>{code}</code></pre>
        </div>
      </div>
    );
  }

  return (
    <div className={`code-block-wrapper ${className}`}>
      <div className="code-block-header">
        <span className="code-block-language">{displayName}</span>
        <button className="code-block-copy-btn" onClick={handleCopy}>
          {copied ? <CheckIcon size={14} color="var(--success)" /> : <CopyIcon size={14} />}
        </button>
      </div>
      <div 
        className="code-block-content"
        dangerouslySetInnerHTML={{ __html: highlightedCode }}
      />
    </div>
  );
};

function escapeHtml(text: string): string {
  const div = document.createElement('div');
  div.textContent = text;
  return div.innerHTML;
}
