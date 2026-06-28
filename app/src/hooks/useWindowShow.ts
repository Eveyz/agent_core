import { useEffect } from 'react';

export function useWindowShow() {
  useEffect(() => {
    const showWindow = async () => {
      try {
        const { getCurrentWindow } = await import('@tauri-apps/api/window');
        const win = getCurrentWindow();
        await win.show();
        await win.setFocus();
      } catch (e) {
        // Not in Tauri environment (dev mode)
      }
    };
    showWindow();
  }, []);
}
