import ArrowLeftIcon from 'lucide-react/dist/esm/icons/arrow-left.mjs';
import ArrowRightIcon from 'lucide-react/dist/esm/icons/arrow-right.mjs';
import RefreshCwIcon from 'lucide-react/dist/esm/icons/refresh-cw.mjs';
import ExternalLinkIcon from 'lucide-react/dist/esm/icons/external-link.mjs';
import MinusIcon from 'lucide-react/dist/esm/icons/minus.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import LockIcon from 'lucide-react/dist/esm/icons/lock.mjs';
import type { PreviewDescriptor, PreviewStatus } from '../../features/preview/previewApi';

interface PreviewBrowserChromeProps {
  descriptor: PreviewDescriptor;
  status: PreviewStatus | 'idle';
  onReload: () => void;
  onPopout: () => void;
  onHide: () => void;
  onStop: () => void;
}

function displayUrl(url: string, entrypoint?: string | null): string {
  if (entrypoint) {
    return entrypoint;
  }
  try {
    const parsed = new URL(url);
    return `localhost:${parsed.port}`;
  } catch {
    return 'localhost preview';
  }
}

export function PreviewBrowserChrome({
  descriptor,
  status,
  onReload,
  onPopout,
  onHide,
  onStop,
}: PreviewBrowserChromeProps) {
  const isLoading = status === 'starting';

  return (
    <div className="preview-browser-chrome">
      <div className="preview-chrome-nav">
        <button className="preview-chrome-btn" disabled title="后退（不可用）">
          <ArrowLeftIcon size={14} />
        </button>
        <button className="preview-chrome-btn" disabled title="前进（不可用）">
          <ArrowRightIcon size={14} />
        </button>
        <button
          className={`preview-chrome-btn${isLoading ? ' spinning' : ''}`}
          onClick={onReload}
          title="刷新"
          disabled={isLoading}
        >
          <RefreshCwIcon size={14} />
        </button>
      </div>

      <div className="preview-chrome-urlbar">
        <LockIcon size={12} className="preview-url-lock" />
        <span className="preview-url-text" title={descriptor.url}>
          {displayUrl(descriptor.url, descriptor.entrypoint)}
        </span>
        {descriptor.entrypoint && (
          <span className="preview-url-entry">live</span>
        )}
        <span className={`preview-chrome-status preview-chrome-status-${status}`}>
          {status}
        </span>
      </div>

      <div className="preview-chrome-actions">
        <button className="preview-chrome-btn" onClick={onPopout} title="独立窗口打开">
          <ExternalLinkIcon size={14} />
        </button>
        <button className="preview-chrome-btn" onClick={onHide} title="收起预览">
          <MinusIcon size={14} />
        </button>
        <button className="preview-chrome-btn preview-chrome-btn-stop" onClick={onStop} title="关闭预览">
          <XIcon size={14} />
        </button>
      </div>
    </div>
  );
}
