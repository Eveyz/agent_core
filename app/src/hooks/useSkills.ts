import { useEffect, useCallback } from 'react';
import { useSelector } from 'react-redux';
import { useAppDispatch } from './useAppDispatch';
import { RootState } from '../store';
import { fetchSkills, invalidateSkillsCache } from '../features/chat/chatSlice';
import { resolveSkillScope } from '../features/chat/skillScope';

export function useSkills() {
  const dispatch = useAppDispatch();
  const skillsCache = useSelector((state: RootState) => state.chat.skillsCache);
  const scopeKey = useSelector((state: RootState) => resolveSkillScope(state.project).scopeKey);
  const scopedCache = skillsCache?.scopeKey === scopeKey ? skillsCache : null;

  useEffect(() => {
    if (!scopedCache || Date.now() - scopedCache.loadedAt > 25000) {
      dispatch(fetchSkills());
    }
  }, [dispatch, scopedCache, scopeKey]);

  return {
    skills: scopedCache?.skills ?? [],
    loadedAt: scopedCache?.loadedAt ?? null,
    loading: !scopedCache,
    refresh: useCallback(
      (opts?: { force?: boolean }) => dispatch(fetchSkills(opts?.force ? { force: true } : undefined)),
      [dispatch],
    ),
    /** Drop caches and rescan disk — use when opening skill pickers / @ mentions. */
    refreshFromDisk: useCallback(async () => {
      await dispatch(invalidateSkillsCache());
      await dispatch(fetchSkills({ force: true }));
    }, [dispatch]),
    invalidate: useCallback(() => dispatch(invalidateSkillsCache()), [dispatch]),
  };
}
