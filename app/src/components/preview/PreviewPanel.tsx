import { useCallback, useMemo } from 'react';
import { useSelector } from 'react-redux';
import { RootState } from '../../store';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import {
  hidePreviewPanel,
  openPreviewPopout,
  restartPreview,
  selectActivePreview,
  showPreviewPanel,
  stopPreview,
} from '../../features/preview/previewSlice';
import { PreviewBrowserChrome } from './PreviewBrowserChrome';
import { NativePreviewSurface } from './NativePreviewSurface';

export function PreviewPanel() {
  const dispatch = useAppDispatch();
  const descriptor = useSelector(selectActivePreview);
  const lastError = useSelector((state: RootState) => state.preview.lastError);
  const logs = useSelector((state: RootState) =>
    descriptor ? state.preview.logs[descriptor.id] ?? [] : [],
  );

  const status = descriptor?.status ?? 'idle';
  const isReady = status === 'ready';

  const handleReload = useCallback(() => {
    if (!descriptor) return;
    void dispatch(restartPreview(descriptor.id));
  }, [dispatch, descriptor]);

  const handlePopout = useCallback(() => {
    if (!descriptor) return;
    void dispatch(openPreviewPopout(descriptor.id));
  }, [dispatch, descriptor]);

  const handleReopen = useCallback(() => {
    if (!descriptor) return;
    void dispatch(showPreviewPanel(descriptor.id));
  }, [dispatch, descriptor]);

  const handleHide = useCallback(() => {
    if (!descriptor) return;
    void dispatch(hidePreviewPanel(descriptor.id));
  }, [dispatch, descriptor]);

  const handleStop = useCallback(() => {
    if (!descriptor) return;
    void dispatch(stopPreview(descriptor.id));
  }, [dispatch, descriptor]);

  const logText = useMemo(() => logs.slice(-4).join('\n'), [logs]);

  if (!descriptor) {
    return null;
  }

  if (descriptor.placement === 'hidden') {
    return (
      <div className="preview-sidebar-panel preview-sidebar-dormant">
        <p>预览已收起，后台仍在运行。</p>
        <div className="preview-dormant-actions">
          <button className="preview-btn" onClick={handleReopen}>
            重新打开
          </button>
          <button className="preview-btn secondary" onClick={handleStop}>
            关闭预览
          </button>
        </div>
      </div>
    );
  }

  if (descriptor.placement === 'popout') {
    return (
      <div className="preview-sidebar-panel preview-sidebar-panel-popout">
        <PreviewBrowserChrome
          descriptor={descriptor}
          status={status}
          onReload={handleReload}
          onPopout={handlePopout}
          onHide={handleHide}
          onStop={handleStop}
        />
        <div className="preview-popout-body">
          <span>预览已在独立窗口中打开。</span>
          <div className="preview-dormant-actions">
            <button className="preview-btn secondary" onClick={handleHide}>
              收起到侧栏
            </button>
            <button className="preview-btn secondary" onClick={handleStop}>
              关闭预览
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="preview-sidebar-panel">
      <PreviewBrowserChrome
        descriptor={descriptor}
        status={status}
        onReload={handleReload}
        onPopout={handlePopout}
        onHide={handleHide}
        onStop={handleStop}
      />

      {lastError && <div className="preview-error">{lastError}</div>}

      <div className="preview-sidebar-body">
        {descriptor.url && (
          <NativePreviewSurface
            previewId={descriptor.id}
            url={descriptor.url}
            visible={isReady}
          />
        )}
        {!isReady && (
          <div className="preview-loading-overlay">
            <div className="preview-loading-spinner" />
            <span>正在启动预览…</span>
          </div>
        )}
      </div>

      {logText && status !== 'ready' && <pre className="preview-logs">{logText}</pre>}
    </div>
  );
}
