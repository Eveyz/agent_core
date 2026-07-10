import { invoke } from '@tauri-apps/api/core';

export type PreviewMode = 'static' | 'framework';
export type PreviewStatus = 'starting' | 'ready' | 'stopping' | 'stopped' | 'failed';
export type PreviewPlacement = 'hidden' | 'split' | 'popout';

export interface PreviewDescriptor {
  id: string;
  workspace_id: string;
  session_id?: string | null;
  mode: PreviewMode;
  url: string;
  status: PreviewStatus;
  revision: number;
  placement: PreviewPlacement;
  entrypoint?: string | null;
}

export interface FrameworkCommandRequest {
  program: string;
  args: string[];
}

export interface FrameworkDetection {
  package_manager?: string | null;
  dev_script?: string | null;
  suggested_program?: string | null;
  suggested_args: string[];
}

export interface PreviewStartRequest {
  workspace_id: string;
  session_id?: string | null;
  mode: PreviewMode;
  entrypoint?: string | null;
  approved_command?: FrameworkCommandRequest | null;
  placement?: PreviewPlacement | null;
}

export type PreviewEvent =
  | { v: 1; type: 'status'; preview_id: string; status: PreviewStatus }
  | { v: 1; type: 'reload'; preview_id: string; revision: number; paths: string[] }
  | { v: 1; type: 'log'; preview_id: string; stream: string; line: string }
  | { v: 1; type: 'error'; preview_id: string; code: string; message: string };

export async function previewStart(request: PreviewStartRequest): Promise<PreviewDescriptor> {
  return invoke<PreviewDescriptor>('preview_start', { request });
}

export async function previewStop(previewId: string): Promise<void> {
  return invoke('preview_stop', { previewId });
}

export async function previewRestart(previewId: string): Promise<PreviewDescriptor> {
  return invoke<PreviewDescriptor>('preview_restart', { previewId });
}

export async function previewGet(previewId: string): Promise<PreviewDescriptor | null> {
  return invoke<PreviewDescriptor | null>('preview_get', { previewId });
}

export async function previewList(workspaceId: string): Promise<PreviewDescriptor[]> {
  return invoke<PreviewDescriptor[]>('preview_list', { workspaceId });
}

export async function previewSetVisibility(
  previewId: string,
  placement: PreviewPlacement,
): Promise<PreviewDescriptor> {
  return invoke<PreviewDescriptor>('preview_set_visibility', {
    request: { preview_id: previewId, placement },
  });
}

export async function previewOpenPopout(previewId: string): Promise<void> {
  return invoke('preview_open_popout', { previewId });
}

export async function previewClosePopout(previewId: string): Promise<void> {
  return invoke('preview_close_popout', { previewId });
}

export async function previewDetectFramework(workspaceId: string): Promise<FrameworkDetection> {
  return invoke<FrameworkDetection>('preview_detect_framework', { workspaceId });
}
