import { describe, expect, it } from 'vitest';
import previewReducer, {
  previewEventReceived,
  previewOpenedFromTool,
  setActivePreview,
} from './previewSlice';

describe('previewSlice', () => {
  it('tracks reload revisions from preview events', () => {
    const initial = previewReducer(undefined, { type: 'init' });
    const withPreview = previewReducer(
      {
        ...initial,
        byId: {
          'abc': {
            id: 'abc',
            workspace_id: 'ws1',
            mode: 'static',
            url: 'http://127.0.0.1:1/p/token/',
            status: 'ready',
            revision: 0,
            placement: 'split',
          },
        },
        activePreviewId: 'abc',
      },
      previewEventReceived({
        v: 1,
        type: 'reload',
        preview_id: 'abc',
        revision: 3,
        paths: ['index.html'],
      }),
    );
    expect(withPreview.byId.abc?.revision).toBe(3);
  });

  it('clears active preview id', () => {
    const state = previewReducer(
      { ...previewReducer(undefined, { type: 'init' }), activePreviewId: 'abc' },
      setActivePreview(null),
    );
    expect(state.activePreviewId).toBeNull();
  });

  it('opens panel when agent preview tool returns a descriptor', () => {
    const state = previewReducer(
      previewReducer(undefined, { type: 'init' }),
      previewOpenedFromTool({
        id: 'abc',
        workspace_id: 'ws1',
        mode: 'static',
        url: 'http://127.0.0.1:1/p/token/',
        status: 'starting',
        revision: 0,
        placement: 'split',
        entrypoint: 'index.html',
      }),
    );
    expect(state.panelOpen).toBe(true);
    expect(state.activePreviewId).toBe('abc');
    expect(state.byId.abc?.entrypoint).toBe('index.html');
  });
});
