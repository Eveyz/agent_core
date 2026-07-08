import { createSelector } from '@reduxjs/toolkit';
import { useState, useEffect, useRef } from 'react';
import { useAppSelector } from './useAppDispatch';
import { roughTokenCount } from '../utils/tokens';
import { selectActiveSessionEntries } from '../features/chat/selectors';

const selectTokenCount = createSelector(
  [selectActiveSessionEntries],
  (entries) => {
    return entries.reduce((sum, e) => {
      if (e.type === 'user' && e.text) return sum + roughTokenCount(e.text);
      if (e.type === 'turn' && e.blocks)
        return sum + e.blocks.reduce((s, b) => {
          if (b.type === 'assistant' || b.type === 'thinking') return s + roughTokenCount(b.text || '');
          if (b.type === 'tool') return s + roughTokenCount(b.result || '');
          return s;
        }, 0);
      return sum;
    }, 0);
  }
);

/**
 * PERF-2: Throttled token count.
 *
 * The selector itself is memoized, but Immer creates a new `entries` array on
 * every token delta, so the memoization misses. To avoid blocking the main
 * thread with O(total text) computation on every token, we sample the selector
 * at most once every 500ms via a timer-based throttle.
 */
export function useTokenCount(): number {
  const rawCount = useAppSelector(selectTokenCount);
  const [displayCount, setDisplayCount] = useState(rawCount);
  const lastUpdateRef = useRef(0);

  useEffect(() => {
    const now = Date.now();
    if (now - lastUpdateRef.current >= 500) {
      lastUpdateRef.current = now;
      setDisplayCount(rawCount);
    } else {
      const timer = setTimeout(() => {
        lastUpdateRef.current = Date.now();
        setDisplayCount(rawCount);
      }, 500 - (now - lastUpdateRef.current));
      return () => clearTimeout(timer);
    }
  }, [rawCount]);

  return displayCount;
}

function estimateModelCalls(blocks: any[]): number {
  if (!blocks || blocks.length === 0) return 1;
  let calls = 1;
  let lastWasTool = false;
  for (const b of blocks) {
    if (b.type === 'tool') {
      lastWasTool = true;
    } else if (b.type === 'assistant' || b.type === 'thinking' || b.type === 'error') {
      if (lastWasTool) {
        calls++;
        lastWasTool = false;
      }
    }
  }
  return calls;
}

const selectTurnCount = createSelector(
  [selectActiveSessionEntries],
  (entries) => {
    return entries.reduce((sum, e) => {
      if (e.type === 'turn') {
        if (e.turnIds && e.turnIds.length > 0) {
          return sum + e.turnIds.length;
        }
        return sum + estimateModelCalls(e.blocks || []);
      }
      return sum;
    }, 0);
  }
);

export function useTurnCount(): number {
  return useAppSelector(selectTurnCount);
}

const selectLatestCacheHitRate = createSelector(
  [selectActiveSessionEntries],
  (entries) => {
    for (let i = entries.length - 1; i >= 0; i--) {
      const entry = entries[i];
      if (entry.type === 'turn') {
        return entry.cacheHitRate !== undefined ? entry.cacheHitRate : null;
      }
    }
    return null;
  }
);

export function useCacheHitRate(): number | null {
  return useAppSelector(selectLatestCacheHitRate);
}

