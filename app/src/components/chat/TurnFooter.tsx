import { useState, useMemo, useCallback, useEffect, useRef, memo } from 'react';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import { useTranslation } from 'react-i18next';
import type { ChatEntry, TurnBlock } from '../../features/chat/chatSlice';
import { progressiveToolVerbKey } from './turnHelpers';
import type { WebSource } from './webSources';
import type { MapFeature } from './mapSources';

/** After this many ms with no new stream tokens, show a "waiting on model" hint. */
const MODEL_IDLE_MS = 4_000;
/** Rotate witty waiting lines every N ms while still idle. */
const WAITING_ROTATE_MS = 8_000;

const WAITING_KEYS = [
  'chat.footer.modelWaiting',
  'chat.footer.modelWaitingAlt',
  'chat.footer.modelWaitingQuiet',
] as const;

const TurnFooter = memo(function TurnFooter({
  entry,
  sources = [],
  mapFeatures = [],
  showProcessingIndicator = true,
}: {
  entry: ChatEntry;
  sources?: WebSource[];
  mapFeatures?: MapFeature[];
  showProcessingIndicator?: boolean;
}) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  // PERF-7: Skip expensive text concatenation while streaming (no endTime).
  // The footer renders null during streaming anyway, so the computation is wasted.
  const isStreaming = !entry.endTime;
  const sourceIcons = useMemo(() => {
    const seen = new Set<string>();
    const icons: { url: string; faviconUrl: string }[] = [];
    for (const s of sources) {
      const key = s.siteName || s.url;
      if (seen.has(key) || !s.faviconUrl) continue;
      seen.add(key);
      icons.push({ url: s.url, faviconUrl: s.faviconUrl });
      if (icons.length >= 3) break;
    }
    return icons;
  }, [sources]);

  const openSourcesOverview = useCallback(() => {
    window.dispatchEvent(
      new CustomEvent('open-right-sidebar', {
        detail: { tab: 'overview', section: 'web' },
      }),
    );
  }, []);

  const openMapsOverview = useCallback(() => {
    window.dispatchEvent(
      new CustomEvent('open-right-sidebar', {
        detail: { tab: 'overview', section: 'maps' },
      }),
    );
  }, []);
  const rawOutput = useMemo(() => {
    if (isStreaming || !entry.blocks) return '';
    return entry.blocks
      .filter((b): b is Extract<TurnBlock, { type: 'assistant' }> => b.type === 'assistant')
      .map((b) => b.text)
      .join('\n');
  }, [entry.blocks, isStreaming]);

  const endTimeText = useMemo(() => {
    if (!entry.endTime) return null;
    const d = new Date(entry.endTime);
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }, [entry.endTime]);

  const handleCopy = useCallback(async () => {
    if (!rawOutput) return;
    try {
      await navigator.clipboard.writeText(rawOutput);
      setCopied(true);
    } catch {
      // ignore
    }
  }, [rawOutput]);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);

  const isProcessing = !entry.endTime;

  // Fingerprint of in-flight model output — changes when thinking/text grows.
  const streamFingerprint = useMemo(() => {
    if (!isProcessing || !entry.blocks) return '';
    let thinkingLen = 0;
    let textLen = 0;
    let streaming = false;
    for (const b of entry.blocks) {
      if (b.type === 'thinking') {
        thinkingLen += b.text.length;
        if (b.isStreaming) streaming = true;
      } else if (b.type === 'assistant') {
        textLen += b.text.length;
        if (b.isStreaming) streaming = true;
      }
    }
    return streaming ? `${thinkingLen}:${textLen}` : '';
  }, [entry.blocks, isProcessing]);

  const lastActivityRef = useRef(Date.now());
  const [idleMs, setIdleMs] = useState(0);

  useEffect(() => {
    if (!isProcessing) {
      setIdleMs(0);
      return;
    }
    if (streamFingerprint) {
      lastActivityRef.current = Date.now();
      setIdleMs(0);
    }
  }, [streamFingerprint, isProcessing]);

  useEffect(() => {
    if (!isProcessing || !streamFingerprint) {
      setIdleMs(0);
      return;
    }
    const id = window.setInterval(() => {
      setIdleMs(Date.now() - lastActivityRef.current);
    }, 1000);
    return () => window.clearInterval(id);
  }, [isProcessing, streamFingerprint]);

  const statusText = useMemo(() => {
    if (!isProcessing || !entry.blocks) return t('chat.footer.working');

    const preparing = entry.blocks.filter(
      (b): b is Extract<TurnBlock, { type: 'tool' }> =>
        b.type === 'tool' && b.phase === 'preparing'
    );
    if (preparing.length > 0) {
      const names = [...new Set(preparing.map((b) => b.name).filter((n) => n && n !== 'tool'))];
      if (names.length === 1) {
        const verb = t(`chat.tools.verbs.${progressiveToolVerbKey(names[0])}`);
        // "Editing…" / "Writing…" — same progressive tense as the tool row.
        // Avoid "Generating write_file" which sounds like codegen.
        return t('chat.footer.callingTool', { verb });
      }
      return t('chat.footer.callingTools');
    }

    const hasActiveTool = entry.blocks.some(
      (b) => b.type === 'tool' && b.active && b.phase !== 'preparing'
    );
    if (hasActiveTool) {
      return t('chat.footer.working');
    }

    // Model still streaming (or mid-stream silence before tool_call).
    if (streamFingerprint) {
      if (idleMs >= MODEL_IDLE_MS) {
        const idx = Math.floor((idleMs - MODEL_IDLE_MS) / WAITING_ROTATE_MS) % WAITING_KEYS.length;
        return t(WAITING_KEYS[idx]);
      }
      return t('chat.footer.modelThinking');
    }

    return t('chat.footer.working');
  }, [entry.blocks, isProcessing, streamFingerprint, idleMs, t]);

  if (isProcessing) {
    if (!showProcessingIndicator) return null;
    return (
      <div className="turn-footer turn-footer-processing">
        <div className="black-hole-spinner" style={{ width: 12, height: 12 }} />
        <span className="turn-end-time">{statusText}</span>
      </div>
    );
  }

  if (!endTimeText && !rawOutput && sources.length === 0 && mapFeatures.length === 0) {
    return null;
  }

  const hasSources = sources.length > 0;
  const hasMaps = mapFeatures.length > 0;

  return (
    <div
      className={`turn-footer${hasSources || hasMaps ? ' turn-footer-has-sources' : ''}`}
    >
      {rawOutput && !isProcessing && (
        <button className="turn-copy-btn" onClick={handleCopy} title="Copy Raw Assistant Output">
          {copied ? <CheckIcon size={11} color="var(--success)" /> : <CopyIcon size={11} />}
        </button>
      )}
      {hasSources && (
        <button
          type="button"
          className="turn-sources-chip"
          onClick={openSourcesOverview}
          title={t('chat.turn.sourcesTitle', { count: sources.length })}
        >
          <span className="turn-sources-favicons" aria-hidden>
            {sourceIcons.map((icon) => (
              <img
                key={icon.url}
                className="turn-sources-favicon"
                src={icon.faviconUrl}
                alt=""
                loading="lazy"
                referrerPolicy="no-referrer"
              />
            ))}
          </span>
          <span>{t('chat.turn.sources')}</span>
        </button>
      )}
      {hasMaps && (
        <button
          type="button"
          className="turn-sources-chip"
          onClick={openMapsOverview}
          title={t('chat.turn.mapsTitle', { count: mapFeatures.length })}
        >
          <span>{t('chat.turn.maps')}</span>
        </button>
      )}
      {endTimeText && <span className="turn-end-time">{endTimeText}</span>}
    </div>
  );
});

export default TurnFooter;
