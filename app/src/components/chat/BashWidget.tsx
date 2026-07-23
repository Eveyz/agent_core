import React, { useState, useEffect, memo, useCallback, useMemo } from 'react';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import WrapTextIcon from 'lucide-react/dist/esm/icons/wrap-text.mjs';
import { getToolIcon } from './toolIcons';
import { highlightCode } from './CodeBlock';

function cleanTerminalOutput(text: string): string {
  if (!text) return '';

  // 1. Strip ANSI escape codes
  const ansiRegex = /[\u001b\u009b][[()#;?]*(?:[0-9]{1,4}(?:;[0-9]{0,4})*)?[0-9A-ORZcf-nqry=><]/g;
  let cleaned = text.replace(ansiRegex, '');

  // 2. Normalize \r\n to \n. After this, any remaining \r is a standalone carriage return.
  cleaned = cleaned.replace(/\r\n/g, '\n');

  // 3. Process carriage returns (\r) and backspaces (\x08) line by line
  const lines = cleaned.split('\n');
  const processedLines = lines.map(line => {
    if (!line.includes('\r') && !line.includes('\x08')) {
      return line;
    }

    const chars: string[] = [];
    let cursor = 0;

    for (let i = 0; i < line.length; i++) {
      const char = line[i];
      if (char === '\r') {
        cursor = 0;
      } else if (char === '\x08') {
        if (cursor > 0) {
          cursor--;
        }
      } else {
        chars[cursor] = char;
        cursor++;
      }
    }

    for (let i = 0; i < chars.length; i++) {
      if (chars[i] === undefined) {
        chars[i] = ' ';
      }
    }

    return chars.join('');
  });

  return processedLines.join('\n');
}

/** Heuristic: PowerShell vs bash for syntax highlighting. */
function detectShellLanguage(command: string): 'bash' | 'powershell' {
  const c = command.trim();
  if (!c) return 'bash';
  if (/^\s*(pwsh|powershell)(\.exe)?\b/i.test(c)) return 'powershell';
  if (/\.(ps1|psm1)\b/i.test(c)) return 'powershell';
  // Verb-Noun cmdlets and common PS idioms
  if (/\b(Get|Set|New|Remove|Add|Clear|Write|Import|Export|Select|Where|ForEach|Invoke|Start|Stop|Out|ConvertTo|ConvertFrom)-[A-Za-z]/.test(c)) {
    return 'powershell';
  }
  if (/\$_\b|\$PSVersionTable\b|\$env:[A-Za-z]|-eq\b|-ne\b|-gt\b|-lt\b|-like\b|-match\b|Out-Null|Out-File|ForEach-Object|Where-Object/.test(c)) {
    return 'powershell';
  }
  return 'bash';
}

/** Parse trailing `[exit code: N]` from shell tool output. */
function parseExitCode(result: string): number | null {
  const match = result.match(/\[exit code:\s*(-?\d+)\]\s*$/m);
  if (!match) return null;
  const code = Number(match[1]);
  return Number.isFinite(code) ? code : null;
}

type ShellStream = 'stdout' | 'stderr';

interface ShellOutputSection {
  stream: ShellStream;
  text: string;
}

/** Split tool output into stdout / stderr; drop the exit-code trailer (shown in status). */
function parseShellOutputSections(text: string): ShellOutputSection[] {
  if (!text) return [];

  let body = text.replace(/\n?\[exit code:\s*-?\d+\]\s*$/m, '');
  // Normalize occasional "--- stderr--" typos from older formatters
  body = body.replace(/\n--- stderr-+\n/g, '\n--- stderr ---\n');

  const parts = body.split(/\n--- stderr ---\n/);
  const sections: ShellOutputSection[] = [];

  const stdout = (parts[0] ?? '').replace(/^\n+/, '').replace(/\n+$/, '');
  if (stdout) sections.push({ stream: 'stdout', text: stdout });

  if (parts.length > 1) {
    const stderr = parts.slice(1).join('\n--- stderr ---\n').replace(/^\n+/, '').replace(/\n+$/, '');
    if (stderr) sections.push({ stream: 'stderr', text: stderr });
  }

  return sections;
}

type LineTone = 'default' | 'error' | 'warn' | 'meta' | 'sep';

function classifyOutputLine(line: string, stream: ShellStream): LineTone {
  const t = line.trim();
  if (!t) return 'default';
  if (/^-{2,}\s*\w/.test(t) || /^-{3,}$/.test(t) || /^={3,}/.test(t)) return 'sep';
  if (
    /^(Traceback|Exception|Error|Fatal|panic:|FAILED|FAIL:)/i.test(t) ||
    /^\w*(Error|Exception|Panic):/.test(t) ||
    (stream === 'stderr' && /error|failed|fatal/i.test(t) && t.length < 200)
  ) {
    return 'error';
  }
  if (/^(warning|warn)\b/i.test(t) || /\bWARN\b/.test(t)) return 'warn';
  if (/^\s*File ".+", line \d+/.test(line) || /^\s+at\s+\S+/.test(line)) return 'meta';
  return 'default';
}

function ShellOutputBody({
  sections,
  wrapLines,
}: {
  sections: ShellOutputSection[];
  wrapLines: boolean;
}) {
  if (sections.length === 0) {
    return (
      <pre className={`bash-details-output ${wrapLines ? 'bash-details-output-wrap' : 'bash-details-output-scroll'}`} />
    );
  }

  return (
    <>
      {sections.map((section, idx) => (
        <div key={`${section.stream}-${idx}`} className={`bash-stream bash-stream-${section.stream}`}>
          {sections.length > 1 && (
            <div className="bash-stream-label">{section.stream}</div>
          )}
          <pre
            className={`bash-details-output ${wrapLines ? 'bash-details-output-wrap' : 'bash-details-output-scroll'}`}
          >
            {section.text.split('\n').map((line, lineIdx) => {
              const tone = classifyOutputLine(line, section.stream);
              return (
                <span key={lineIdx} className={`bash-line bash-line-${tone}`}>
                  {line}
                  {'\n'}
                </span>
              );
            })}
          </pre>
        </div>
      ))}
    </>
  );
}

const BashWidget = memo(function BashWidget({
  args,
  result,
  active,
  is_error,
  name,
}: {
  args?: unknown;
  result?: string;
  active?: boolean;
  is_error?: boolean;
  name?: string;
}) {
  const [collapsed, setCollapsed] = useState(!active);
  const [copied, setCopied] = useState(false);
  const [wrapLines, setWrapLines] = useState(false);
  const [commandHtml, setCommandHtml] = useState('');

  const cleanedResult = useMemo(() => {
    return cleanTerminalOutput(result || '');
  }, [result]);

  const exitCode = useMemo(() => parseExitCode(cleanedResult), [cleanedResult]);
  const failed = Boolean(is_error) || (exitCode !== null && exitCode !== 0);
  const outputSections = useMemo(() => parseShellOutputSections(cleanedResult), [cleanedResult]);

  const toolName = name || 'shell';
  let command = (args as Record<string, unknown> | undefined)?.command as string || '';

  let customLabel: React.ReactNode = null;

  if (!command) {
    if (toolName === 'grep_search' || toolName === 'grep') {
      const q = ((args as Record<string, unknown> | undefined)?.Query || (args as Record<string, unknown> | undefined)?.query || (args as Record<string, unknown> | undefined)?.pattern || '') as string;
      const p = ((args as Record<string, unknown> | undefined)?.SearchPath || (args as Record<string, unknown> | undefined)?.search_path || (args as Record<string, unknown> | undefined)?.path || '') as string;
      command = `grep "${q}" ${p}`;
      const basename = p ? p.split('/').pop() : 'files';
      customLabel = <>{active ? 'Searching' : 'Searched'} <span className="bash-command">{basename}</span></>;
    } else if (toolName.includes('glob') || toolName.includes('find')) {
      const pat = ((args as Record<string, unknown> | undefined)?.Pattern || (args as Record<string, unknown> | undefined)?.pattern || '') as string;
      const p = ((args as Record<string, unknown> | undefined)?.SearchPath || (args as Record<string, unknown> | undefined)?.search_path || (args as Record<string, unknown> | undefined)?.path || (args as Record<string, unknown> | undefined)?.dir || '') as string;
      command = `find ${p} -name "${pat}"`;
      const basename = p ? p.split('/').pop() : 'directory';
      customLabel = <>{active ? 'Exploring' : 'Explored'} <span className="bash-command">{basename}</span></>;
    } else {
      command = typeof args === 'string' ? args : JSON.stringify(args);
    }
  }

  const shellLang = useMemo(() => detectShellLanguage(command), [command]);
  const shellLabel = shellLang === 'powershell' ? 'PowerShell' : 'Bash';
  const shellPrompt = shellLang === 'powershell' ? 'PS> ' : '$ ';

  useEffect(() => {
    let mounted = true;
    if (!command) {
      setCommandHtml('');
      return;
    }
    highlightCode(command, shellLang).then((html) => {
      if (mounted) setCommandHtml(html);
    });
    return () => {
      mounted = false;
    };
  }, [command, shellLang]);

  const displayCommand = command.length > 50 ? command.substring(0, 50) + '...' : command;

  const labelPrefix = active ? 'Running' : 'Ran';
  const ToolIcon = getToolIcon(toolName);

  const handleCopy = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(cleanedResult || '');
      setCopied(true);
    } catch (error) {
      console.error('Failed to copy:', error);
    }
  }, [cleanedResult]);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(timer);
  }, [copied]);

  return (
    <div className="step-block bash-block">
      <div
        className={`step-row ${active ? 'step-row-active' : ''} ${failed ? 'step-row-error' : ''} step-row-pointer`}
        onClick={() => setCollapsed(!collapsed)}
      >
        <ToolIcon size={13} className="step-icon tool-icon-margin" color={failed ? 'var(--danger)' : 'var(--text-muted)'} />
        <span className="step-label bash-label">
          {customLabel ? (
            customLabel
          ) : (
            <>{labelPrefix} <span className="bash-command">{displayCommand}</span></>
          )}
        </span>
        <div className="tool-controls-flex">
          {collapsed ? <ChevronRightIcon size={12} className="step-chevron" /> : <ChevronDownIcon size={12} className="step-chevron" />}
        </div>
      </div>
      {!collapsed && (
        <div className={`bash-details-wrapper ${failed ? 'bash-details-wrapper-fail' : ''}`}>
          <div className="bash-details-header">
            <span className="bash-details-header-label">{shellLabel}</span>
            <div className="bash-details-header-actions">
              {!active && (
                <span
                  className={`bash-details-status-chip ${failed ? 'bash-details-status-fail' : 'bash-details-status-ok'}`}
                >
                  {failed ? (
                    exitCode !== null ? `Fail · ${exitCode}` : 'Fail'
                  ) : (
                    <>
                      <CheckIcon size={11} strokeWidth={2.5} />
                      Success
                    </>
                  )}
                </span>
              )}
              {result && (
                <>
                  <button
                    className="code-block-copy-btn bash-output-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      setWrapLines((w) => !w);
                    }}
                    title={wrapLines ? 'Unwrap lines' : 'Wrap lines'}
                    type="button"
                    data-active={wrapLines || undefined}
                  >
                    <WrapTextIcon size={13} />
                  </button>
                  <button
                    className="code-block-copy-btn bash-output-btn"
                    onClick={handleCopy}
                    title="Copy output"
                    type="button"
                  >
                    {copied ? <CheckIcon size={13} color="var(--success)" /> : <CopyIcon size={13} />}
                  </button>
                </>
              )}
            </div>
          </div>
          <div className="bash-details-body">
            <div className="bash-details-command">
              <span className="bash-prompt">{shellPrompt}</span>
              {commandHtml ? (
                <div
                  className="bash-details-command-code"
                  dangerouslySetInnerHTML={{ __html: commandHtml }}
                />
              ) : (
                <span className="bash-details-command-fallback">{command}</span>
              )}
            </div>
            {result && outputSections.length > 0 && (
              <div className="bash-details-output-wrapper">
                <div className={`bash-output-scroll-area ${wrapLines ? 'is-wrapping' : ''}`}>
                  <ShellOutputBody sections={outputSections} wrapLines={wrapLines} />
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
});

export default BashWidget;
