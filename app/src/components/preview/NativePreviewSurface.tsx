import { useEffect, useRef, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Webview } from '@tauri-apps/api/webview';
import { PhysicalPosition, PhysicalSize } from '@tauri-apps/api/dpi';

interface NativePreviewSurfaceProps {
  previewId: string;
  url: string;
  visible: boolean;
}

export function NativePreviewSurface({ previewId, url, visible }: NativePreviewSurfaceProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [useIframeFallback, setUseIframeFallback] = useState(false);
  const webviewLabel = `preview-${previewId}`;

  useEffect(() => {
    if (!visible || useIframeFallback) return;

    let disposed = false;
    let webview: Webview | null = null;

    const syncBounds = async () => {
      const el = containerRef.current;
      if (!el) return null;
      const rect = el.getBoundingClientRect();
      const scale = window.devicePixelRatio || 1;
      return {
        x: Math.round(rect.left * scale),
        y: Math.round(rect.top * scale),
        width: Math.max(1, Math.round(rect.width * scale)),
        height: Math.max(1, Math.round(rect.height * scale)),
      };
    };

    const mount = async () => {
      try {
        const appWindow = getCurrentWindow();
        const bounds = await syncBounds();
        if (!bounds || disposed) return;

        webview = new Webview(appWindow, webviewLabel, {
          url,
          x: bounds.x,
          y: bounds.y,
          width: bounds.width,
          height: bounds.height,
        });

        await new Promise<void>((resolve, reject) => {
          const timeout = window.setTimeout(() => reject(new Error('webview timeout')), 5000);
          webview?.once('tauri://created', () => {
            window.clearTimeout(timeout);
            resolve();
          });
          webview?.once('tauri://error', (e) => {
            window.clearTimeout(timeout);
            reject(e);
          });
        });
      } catch {
        if (!disposed) {
          setUseIframeFallback(true);
        }
      }
    };

    void mount();

    const resizeObserver = new ResizeObserver(() => {
      void (async () => {
        if (!webview) return;
        const bounds = await syncBounds();
        if (!bounds) return;
        try {
          await webview.setPosition(new PhysicalPosition(bounds.x, bounds.y));
          await webview.setSize(new PhysicalSize(bounds.width, bounds.height));
        } catch {
          // ignore transient geometry errors
        }
      })();
    });

    const el = containerRef.current;
    if (el) resizeObserver.observe(el);

    return () => {
      disposed = true;
      resizeObserver.disconnect();
      void webview?.close().catch(() => undefined);
    };
  }, [previewId, url, visible, useIframeFallback, webviewLabel]);

  return (
    <div ref={containerRef} className="preview-surface">
      {useIframeFallback && visible ? (
        <iframe
          key={url}
          src={url}
          title="Live preview"
          className="preview-iframe"
          sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-downloads"
          referrerPolicy="no-referrer"
          allow="clipboard-read 'none'; clipboard-write 'none'; camera 'none'; microphone 'none'; geolocation 'none'"
        />
      ) : (
        <div className="preview-surface-placeholder">Loading preview…</div>
      )}
    </div>
  );
}
