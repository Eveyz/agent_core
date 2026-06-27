import { useState, useEffect, useCallback } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import MinusIcon from 'lucide-react/dist/esm/icons/minus.mjs';
import Maximize2Icon from 'lucide-react/dist/esm/icons/maximize-2.mjs';
import Minimize2Icon from 'lucide-react/dist/esm/icons/minimize-2.mjs';
import ChevronLeftIcon from 'lucide-react/dist/esm/icons/chevron-left.mjs';

const TITLE_BAR_HEIGHT = 36;

export function CustomTitleBar({ sidebarCollapsed, onToggleSidebar }: {
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
}) {
  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    const checkMaximized = async () => {
      try {
        const win = getCurrentWindow();
        setIsMaximized(await win.isMaximized());
      } catch {
        // Not in Tauri environment
      }
    };
    checkMaximized();
  }, []);

  const handleMinimize = useCallback(async () => {
    try {
      const win = getCurrentWindow();
      await win.minimize();
    } catch {
      // Not in Tauri environment
    }
  }, []);

  const handleToggleMaximize = useCallback(async () => {
    try {
      const win = getCurrentWindow();
      if (isMaximized) {
        await win.unmaximize();
      } else {
        await win.maximize();
      }
      setIsMaximized(!isMaximized);
    } catch {
      // Not in Tauri environment
    }
  }, [isMaximized]);

  const handleClose = useCallback(async () => {
    try {
      const win = getCurrentWindow();
      await win.close();
    } catch {
      // Not in Tauri environment
    }
  }, []);

  const btnBase: React.CSSProperties = {
    width: 28,
    height: 28,
    borderRadius: 6,
    border: 'none',
    background: 'transparent',
    cursor: 'pointer',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    fontSize: 14,
    transition: 'background 0.15s',
  };

  return (
    <div
      style={{
        height: TITLE_BAR_HEIGHT,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        padding: '0 8px',
        backgroundColor: '#0a0a0f',
        borderBottom: '1px solid rgba(255,255,255,0.06)',
        userSelect: 'none',
        flexShrink: 0,
        zIndex: 1000,
        // @ts-ignore - webkit-app-region is not in React's CSSProperties type
        WebkitAppRegion: 'drag',
        // @ts-ignore
        appRegion: 'drag',
      } as React.CSSProperties}
    >
      {/* Left: window controls */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 6,
          // @ts-ignore
          WebkitAppRegion: 'no-drag',
          // @ts-ignore
          appRegion: 'no-drag',
        } as React.CSSProperties}
      >
        <button
          onClick={handleClose}
          title="关闭"
          style={{ ...btnBase, color: '#ff5f57' }}
          onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(255,95,87,0.15)')}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <XIcon size={14} />
        </button>
        <button
          onClick={handleMinimize}
          title="最小化"
          style={{ ...btnBase, color: '#febc2e' }}
          onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(254,188,46,0.15)')}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <MinusIcon size={14} />
        </button>
        <button
          onClick={handleToggleMaximize}
          title={isMaximized ? '还原' : '最大化'}
          style={{ ...btnBase, color: '#28c840' }}
          onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(40,200,64,0.15)')}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          {isMaximized ? <Minimize2Icon size={12} /> : <Maximize2Icon size={12} />}
        </button>
      </div>

      {/* Center: flex spacer (draggable) */}
      <div style={{ flex: 1 }} />

      {/* Right: sidebar toggle + status */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 4,
          // @ts-ignore
          WebkitAppRegion: 'no-drag',
          // @ts-ignore
          appRegion: 'no-drag',
        } as React.CSSProperties}
      >
        <button
          onClick={onToggleSidebar}
          title={sidebarCollapsed ? '展开侧边栏' : '收起侧边栏'}
          style={{
            ...btnBase,
            color: 'rgba(255,255,255,0.4)',
            width: 28,
            height: 28,
          }}
          onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,0.08)')}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <ChevronLeftIcon size={16} style={{ transform: sidebarCollapsed ? 'rotate(180deg)' : 'rotate(0deg)', transition: 'transform 0.2s' }} />
        </button>
      </div>
    </div>
  );
}
