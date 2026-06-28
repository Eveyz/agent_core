import { useEffect } from 'react';

export function useThemeEffect(appearance: 'dark' | 'light' | 'system') {
  useEffect(() => {
    const root = document.documentElement;
    const applyTheme = (theme: 'dark' | 'light') => {
      root.setAttribute('data-theme', theme);
    };

    if (appearance === 'system') {
      const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
      applyTheme(mediaQuery.matches ? 'dark' : 'light');
      const handler = (e: MediaQueryListEvent) => {
        applyTheme(e.matches ? 'dark' : 'light');
      };
      mediaQuery.addEventListener('change', handler);
      return () => mediaQuery.removeEventListener('change', handler);
    } else {
      applyTheme(appearance);
    }
  }, [appearance]);
}
