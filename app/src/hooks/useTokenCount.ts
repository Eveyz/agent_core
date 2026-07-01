import { createSelector } from '@reduxjs/toolkit';
import { useState, useEffect, useRef } from 'react';
import type { RootState } from '../store';
import { useAppSelector } from './useAppDispatch';
import { roughTokenCount } from '../utils/tokens';

const selectEntries = (state: RootState) => state.chat.entries;

const selectTokenCount = createSelector(
  [selectEntries],
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

const selectTurnCount = createSelector(
  [selectEntries],
  (entries) => entries.filter((e) => e.type === 'turn').length
);

export function useTurnCount(): number {
  return useAppSelector(selectTurnCount);
}

const selectLatestCacheHitRate = createSelector(
  [selectEntries],
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

