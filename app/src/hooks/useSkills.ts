import { useEffect, useCallback } from 'react';
import { useSelector } from 'react-redux';
import { useAppDispatch } from './useAppDispatch';
import { RootState } from '../store';
import { fetchSkills, invalidateSkillsCache } from '../features/chat/chatSlice';

export function useSkills() {
  const dispatch = useAppDispatch();
  const skillsCache = useSelector((state: RootState) => state.chat.skillsCache);

  useEffect(() => {
    if (!skillsCache || Date.now() - skillsCache.loadedAt > 25000) {
      dispatch(fetchSkills());
    }
  }, [dispatch, skillsCache]);

  return {
    skills: skillsCache?.skills ?? [],
    loadedAt: skillsCache?.loadedAt ?? null,
    loading: !skillsCache,
    refresh: useCallback(() => dispatch(fetchSkills()), [dispatch]),
    invalidate: useCallback(() => dispatch(invalidateSkillsCache()), [dispatch]),
  };
}
