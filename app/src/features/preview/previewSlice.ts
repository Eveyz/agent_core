import { createSlice, createAsyncThunk, PayloadAction } from '@reduxjs/toolkit';
import {
  previewStart,
  previewStop,
  previewRestart,
  previewList,
  previewSetVisibility,
  previewOpenPopout,
  previewDetectFramework,
  PreviewDescriptor,
  PreviewEvent,
  PreviewPlacement,
  PreviewStartRequest,
  FrameworkDetection,
} from './previewApi';

export interface PreviewState {
  byId: Record<string, PreviewDescriptor>;
  activePreviewId: string | null;
  activeWorkspaceId: string | null;
  placement: PreviewPlacement;
  panelOpen: boolean;
  panelHeight: number;
  logs: Record<string, string[]>;
  lastError: string | null;
  frameworkDetection: FrameworkDetection | null;
  detectingFramework: boolean;
}

const initialState: PreviewState = {
  byId: {},
  activePreviewId: null,
  activeWorkspaceId: null,
  placement: 'hidden',
  panelOpen: false,
  panelHeight: 360,
  logs: {},
  lastError: null,
  frameworkDetection: null,
  detectingFramework: false,
};

export const startPreview = createAsyncThunk(
  'preview/start',
  async (request: PreviewStartRequest) => previewStart(request),
);

export const stopPreview = createAsyncThunk(
  'preview/stop',
  async (previewId: string) => {
    await previewStop(previewId);
    return previewId;
  },
);

export const restartPreview = createAsyncThunk(
  'preview/restart',
  async (previewId: string) => previewRestart(previewId),
);

export const fetchPreviews = createAsyncThunk(
  'preview/list',
  async (workspaceId: string) => previewList(workspaceId),
);

export const setPreviewVisibility = createAsyncThunk(
  'preview/setVisibility',
  async ({ previewId, placement }: { previewId: string; placement: PreviewPlacement }) =>
    previewSetVisibility(previewId, placement),
);

export const hidePreviewPanel = createAsyncThunk(
  'preview/hide',
  async (previewId: string) => previewSetVisibility(previewId, 'hidden'),
);

export const showPreviewPanel = createAsyncThunk(
  'preview/show',
  async (previewId: string) => previewSetVisibility(previewId, 'split'),
);

export const openPreviewPopout = createAsyncThunk(
  'preview/openPopout',
  async (previewId: string) => {
    await previewOpenPopout(previewId);
    return previewId;
  },
);

export const detectFramework = createAsyncThunk(
  'preview/detectFramework',
  async (workspaceId: string) => previewDetectFramework(workspaceId),
);

const previewSlice = createSlice({
  name: 'preview',
  initialState,
  reducers: {
    previewEventReceived(state, action: PayloadAction<PreviewEvent>) {
      const event = action.payload;
      switch (event.type) {
        case 'status': {
          const existing = state.byId[event.preview_id];
          if (existing) {
            existing.status = event.status;
          }
          break;
        }
        case 'reload': {
          const existing = state.byId[event.preview_id];
          if (existing) {
            existing.revision = event.revision;
          }
          break;
        }
        case 'log': {
          const key = event.preview_id;
          const lines = state.logs[key] ?? [];
          lines.push(`[${event.stream}] ${event.line}`);
          if (lines.length > 200) {
            lines.splice(0, lines.length - 200);
          }
          state.logs[key] = lines;
          break;
        }
        case 'error':
          state.lastError = event.message;
          break;
        default:
          break;
      }
    },
    setActivePreview(state, action: PayloadAction<string | null>) {
      state.activePreviewId = action.payload;
    },
    clearPreviewError(state) {
      state.lastError = null;
    },
    previewOpenedFromTool(state, action: PayloadAction<PreviewDescriptor>) {
      const descriptor = action.payload;
      state.byId[descriptor.id] = descriptor;
      state.activePreviewId = descriptor.id;
      state.activeWorkspaceId = descriptor.workspace_id;
      state.placement = descriptor.placement;
      state.panelOpen = true;
      state.lastError = null;
    },
    openPreviewPanel(state) {
      state.panelOpen = true;
    },
    closePreviewPanel(state) {
      state.panelOpen = false;
    },
    setPreviewPanelHeight(state, action: PayloadAction<number>) {
      state.panelHeight = action.payload;
    },
  },
  extraReducers: (builder) => {
    builder
      .addCase(startPreview.fulfilled, (state, action) => {
        const descriptor = action.payload;
        state.byId[descriptor.id] = descriptor;
        state.activePreviewId = descriptor.id;
        state.activeWorkspaceId = descriptor.workspace_id;
        state.placement = descriptor.placement;
        state.panelOpen = true;
        state.lastError = null;
      })
      .addCase(startPreview.rejected, (state, action) => {
        state.lastError = action.error.message ?? 'Failed to start preview';
      })
      .addCase(stopPreview.fulfilled, (state, action) => {
        delete state.byId[action.payload];
        if (state.activePreviewId === action.payload) {
          state.activePreviewId = null;
          state.placement = 'hidden';
          state.panelOpen = false;
        }
      })
      .addCase(restartPreview.fulfilled, (state, action) => {
        const descriptor = action.payload;
        state.byId[descriptor.id] = descriptor;
        state.activePreviewId = descriptor.id;
      })
      .addCase(fetchPreviews.fulfilled, (state, action) => {
        for (const descriptor of action.payload) {
          state.byId[descriptor.id] = descriptor;
        }
      })
      .addCase(setPreviewVisibility.fulfilled, (state, action) => {
        const descriptor = action.payload;
        state.byId[descriptor.id] = descriptor;
        state.placement = descriptor.placement;
      })
      .addCase(hidePreviewPanel.fulfilled, (state, action) => {
        const descriptor = action.payload;
        state.byId[descriptor.id] = descriptor;
        state.placement = descriptor.placement;
        state.panelOpen = false;
      })
      .addCase(showPreviewPanel.fulfilled, (state, action) => {
        const descriptor = action.payload;
        state.byId[descriptor.id] = descriptor;
        state.activePreviewId = descriptor.id;
        state.placement = descriptor.placement;
        state.panelOpen = true;
        state.lastError = null;
      })
      .addCase(openPreviewPopout.fulfilled, (state, action) => {
        const id = action.payload;
        const existing = state.byId[id];
        if (existing) {
          existing.placement = 'popout';
          state.placement = 'popout';
        }
      })
      .addCase(detectFramework.pending, (state) => {
        state.detectingFramework = true;
      })
      .addCase(detectFramework.fulfilled, (state, action) => {
        state.detectingFramework = false;
        state.frameworkDetection = action.payload;
      })
      .addCase(detectFramework.rejected, (state) => {
        state.detectingFramework = false;
        state.frameworkDetection = null;
      });
  },
});

export const {
  previewEventReceived,
  setActivePreview,
  clearPreviewError,
  previewOpenedFromTool,
  openPreviewPanel,
  closePreviewPanel,
  setPreviewPanelHeight,
} = previewSlice.actions;
export default previewSlice.reducer;

export const selectActivePreview = (state: { preview: PreviewState }) =>
  state.preview.activePreviewId ? state.preview.byId[state.preview.activePreviewId] ?? null : null;

export const selectHasPreviewSession = (state: { preview: PreviewState }) => {
  const id = state.preview.activePreviewId;
  return id ? state.preview.byId[id] != null : false;
};

export const selectPreviewPanelOpen = (state: { preview: PreviewState }) => {
  const descriptor = selectActivePreview(state);
  if (!descriptor) return false;
  return state.preview.panelOpen && descriptor.placement !== 'hidden';
};

export const selectDormantPreview = (state: { preview: PreviewState }) => {
  const descriptor = selectActivePreview(state);
  if (!descriptor) return null;
  if (state.preview.panelOpen && descriptor.placement !== 'hidden') return null;
  return descriptor;
};
