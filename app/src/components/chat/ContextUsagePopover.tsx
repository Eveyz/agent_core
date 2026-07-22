import { useCallback, useEffect, useRef, useState } from 'react';
import { useSelector } from 'react-redux';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { RootState } from '../../store';
import { formatContextLabel, effectiveContextTokens } from '../../utils/modelCapabilities';
import '../../styles/context-usage.css';

export interface ContextSegmentUsage {
  key: string;
  label: string;
  tokens: number;
}

export interface ContextUsageSnapshot {
  used_tokens: number;
  max_context_tokens: number;
  segments: ContextSegmentUsage[];
  conversation_tokens: number;
}

const SEGMENT_COLORS: Record<string, string> = {
  system: 'var(--ctx-seg-system, #8b8b8b)',
  tools: 'var(--ctx-seg-tools, #a78bfa)',
  rules: 'var(--ctx-seg-rules, #4ade80)',
  skills: 'var(--ctx-seg-skills, #fbbf24)',
  plan: 'var(--ctx-seg-plan, #60a5fa)',
  environment: 'var(--ctx-seg-env, #67e8f9)',
  conversation: 'var(--ctx-seg-conversation, #f87171)',
};

const RING_SIZE = 18;
const RING_STROKE = 2.25;
const RING_RADIUS = (RING_SIZE - RING_STROKE) / 2;
const RING_CIRC = 2 * Math.PI * RING_RADIUS;

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) {
    const k = n / 1_000;
    return k >= 10 ? `${Math.round(k)}K` : `${k.toFixed(1)}K`;
  }
  return String(n);
}

function emptySnapshot(max: number): ContextUsageSnapshot {
  return {
    used_tokens: 0,
    max_context_tokens: max,
    segments: [],
    conversation_tokens: 0,
  };
}

function ringColor(pct: number): string {
  if (pct >= 90) return 'var(--danger)';
  if (pct >= 70) return 'var(--amber-500, var(--warning, #d97706))';
  // Empty / low: still use accent so the arc is visible in light mode
  return 'var(--accent)';
}

export function ContextUsagePopover() {
  const { t } = useTranslation();
  const activeSessionId = useSelector(
    (state: RootState) => state.project.activeSessionId
  );
  const activeRunId = useSelector((state: RootState) => {
    const sid = state.project.activeSessionId;
    return sid ? state.chat.runId[sid] ?? undefined : undefined;
  });
  const isProcessing = useSelector((state: RootState) => {
    const sid = state.project.activeSessionId;
    return sid ? !!state.chat.processing[sid] : false;
  });
  const defaultModel = useSelector(
    (state: RootState) => state.settings.config?.default_model
  );
  const config = useSelector((state: RootState) => state.settings.config);

  const [open, setOpen] = useState(false);
  const [snapshot, setSnapshot] = useState<ContextUsageSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);

  const resolvedMax = (() => {
    if (!config || !defaultModel) return 128_000;
    const slash = defaultModel.indexOf('/');
    if (slash < 0) return 128_000;
    const provider = config.providers[defaultModel.slice(0, slash)];
    const entry = provider?.models[defaultModel.slice(slash + 1)];
    if (!entry || !provider) return provider?.max_context_tokens ?? 128_000;
    return effectiveContextTokens(
      entry.model_id,
      entry.max_context_tokens,
      provider.max_context_tokens
    );
  })();

  const fetchUsage = useCallback(async () => {
    setLoading(true);
    try {
      const snap = await invoke<ContextUsageSnapshot>('get_context_usage', {
        sessionId: activeSessionId ?? null,
        runId: activeRunId ?? null,
      });
      setSnapshot(snap);
    } catch (e) {
      console.warn('get_context_usage failed:', e);
      setSnapshot(emptySnapshot(resolvedMax));
    } finally {
      setLoading(false);
    }
  }, [activeSessionId, activeRunId, resolvedMax]);

  useEffect(() => {
    void fetchUsage();
  }, [fetchUsage]);

  // Track prior processing so we can force a final refresh when a run ends
  // (interval alone can miss the post-completion snapshot by up to ~4s, and
  // relying only on runId→null is easy to miss if that signal is delayed).
  const wasProcessingRef = useRef(false);
  useEffect(() => {
    if (isProcessing) {
      wasProcessingRef.current = true;
      const id = setInterval(() => {
        void fetchUsage();
      }, 4000);
      return () => clearInterval(id);
    }
    if (wasProcessingRef.current) {
      wasProcessingRef.current = false;
      void fetchUsage();
    }
  }, [isProcessing, fetchUsage]);

  useEffect(() => {
    if (!open) return;
    void fetchUsage();
  }, [open, fetchUsage]);

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open]);

  const segmentLabel = (key: string, fallback: string) =>
    t(`chat.contextUsage.segments.${key}`, { defaultValue: fallback });

  // Used tokens come from the live/estimated snapshot; the denominator always
  // follows the currently selected model so switching models updates the %.
  const data = snapshot ?? emptySnapshot(resolvedMax);
  const max = resolvedMax || data.max_context_tokens || 1;
  const pct = Math.min(100, Math.round((data.used_tokens / max) * 100));
  const dashOffset = RING_CIRC * (1 - pct / 100);
  const color = ringColor(pct);
  const usedLabel = formatTokens(data.used_tokens);
  const maxLabel = formatContextLabel(max);
  const title = t('chat.contextUsage.titleHint', {
    pct,
    used: usedLabel,
    max: maxLabel,
  });

  return (
    <div className="context-usage-wrap" ref={wrapRef}>
      <button
        type="button"
        className="context-usage-ring-btn"
        onClick={() => setOpen((v) => !v)}
        title={title}
        aria-label={t('chat.contextUsage.ariaLabel', { pct })}
      >
        <svg
          className="context-usage-ring"
          width={RING_SIZE}
          height={RING_SIZE}
          viewBox={`0 0 ${RING_SIZE} ${RING_SIZE}`}
          aria-hidden
        >
          <circle
            className="context-usage-ring-track"
            cx={RING_SIZE / 2}
            cy={RING_SIZE / 2}
            r={RING_RADIUS}
            fill="none"
            strokeWidth={RING_STROKE}
          />
          {pct > 0 && (
            <circle
              className="context-usage-ring-fill"
              cx={RING_SIZE / 2}
              cy={RING_SIZE / 2}
              r={RING_RADIUS}
              fill="none"
              strokeWidth={RING_STROKE}
              stroke={color}
              strokeDasharray={RING_CIRC}
              strokeDashoffset={dashOffset}
              strokeLinecap="round"
              transform={`rotate(-90 ${RING_SIZE / 2} ${RING_SIZE / 2})`}
            />
          )}
        </svg>
      </button>

      {open && (
        <div className="context-usage-popover context-usage-popover-right">
          <div className="context-usage-header">
            <span>{t('chat.contextUsage.title')}</span>
            <button
              type="button"
              className="context-usage-close"
              onClick={() => setOpen(false)}
              aria-label={t('chat.contextUsage.close')}
            >
              ×
            </button>
          </div>

          <div className="context-usage-summary">
            <span className="context-usage-pct">
              {t('chat.contextUsage.percentFull', { pct })}
            </span>
            <span className="context-usage-counts">
              {t('chat.contextUsage.tokenCounts', {
                used: usedLabel,
                max: maxLabel,
              })}
            </span>
          </div>

          <div className="context-usage-bar" aria-hidden>
            {data.segments.length === 0 ? (
              <div className="context-usage-bar-empty" />
            ) : (
              data.segments.map((seg) => {
                const width = Math.max(0.5, (seg.tokens / max) * 100);
                const label = segmentLabel(seg.key, seg.label);
                return (
                  <div
                    key={seg.key}
                    className="context-usage-bar-seg"
                    style={{
                      width: `${width}%`,
                      background: SEGMENT_COLORS[seg.key] ?? '#888',
                    }}
                    title={`${label}: ${formatTokens(seg.tokens)}`}
                  />
                );
              })
            )}
          </div>

          <div className="context-usage-legend">
            {loading && data.segments.length === 0 && (
              <div className="context-usage-legend-empty">
                {t('chat.contextUsage.loading')}
              </div>
            )}
            {!loading && data.segments.length === 0 && (
              <div className="context-usage-legend-empty">
                {t('chat.contextUsage.empty')}
              </div>
            )}
            {data.segments.map((seg) => (
              <div key={seg.key} className="context-usage-legend-row">
                <span
                  className="context-usage-swatch"
                  style={{ background: SEGMENT_COLORS[seg.key] ?? '#888' }}
                />
                <span className="context-usage-legend-label">
                  {segmentLabel(seg.key, seg.label)}
                </span>
                <span className="context-usage-legend-tokens">
                  {formatTokens(seg.tokens)}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
