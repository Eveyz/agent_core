import { createSelector } from '@reduxjs/toolkit';
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

export function useTokenCount(): number {
  return useAppSelector(selectTokenCount);
}

export function useTurnCount(): number {
  return useAppSelector((state) => state.chat.entries.filter((e) => e.type === 'turn').length);
}
