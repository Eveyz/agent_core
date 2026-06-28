import React, { useState, useEffect } from 'react';
import { Check, Copy } from 'lucide-react';
import { createHighlighter } from 'shiki';

// Initialize shiki highlighter
let highlighterPromise: ReturnType<typeof createHighlighter> | null = null;

function getShikiHighlighter() {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighter({
      themes: ['github-dark'],
      langs: [
        'rust',
        'python',
        'javascript',
        'typescript',
        'json',
        'toml',
        'yaml',
        'html',
        'css',
        'sql',
        'bash',
        'c',
        'cpp',
        'java',
        'go',
        'ruby',
        'php',
        'swift',
        'kotlin',
        'scala',
        'r',
        'lua',
        'perl',
        'elixir',
        'erlang',
        'haskell',
        'clojure',
        'dart',
        'protobuf',
        'xml',
        'markdown',
        'makefile',
        'cmake',
        'dockerfile',
        'diff',
        'git-commit',
        'ini',
      ],
    });
  }
  return highlighterPromise;
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

    async function highlight() {
      try {
        const highlighter = await getShikiHighlighter();
        if (!mounted) return;

        const lang = language.toLowerCase() || 'plaintext';
        const html = highlighter.codeToHtml(code, {
          lang,
          theme: 'github-dark',
        });
        
        if (mounted) {
          setHighlightedCode(html);
          setLoading(false);
        }
      } catch (error) {
        console.error('Shiki highlighting error:', error);
        if (mounted) {
          // Fallback to plain text if highlighting fails
          setHighlightedCode(`<pre><code>${escapeHtml(code)}</code></pre>`);
          setLoading(false);
        }
      }
    }

    highlight();

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
            <Copy size={14} />
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
          {copied ? <Check size={14} /> : <Copy size={14} />}
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
