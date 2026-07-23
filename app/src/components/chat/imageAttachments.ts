export interface PendingImage {
  id: string;
  mimeType: string;
  previewUrl: string;
  dataBase64: string;
}

export interface ChatImageRef {
  id: string;
  previewUrl: string;
  mimeType: string;
  path?: string;
}

export interface SendPayload {
  text: string;
  images?: PendingImage[];
  agentMentions?: AgentMentionPayload[];
}

export interface AgentMentionPayload {
  agent_id: string;
  revision_id: string;
  optional?: boolean;
}

export function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result;
      if (typeof result !== 'string') {
        reject(new Error('Failed to read image'));
        return;
      }
      const comma = result.indexOf(',');
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.onerror = () => reject(reader.error ?? new Error('Failed to read image'));
    reader.readAsDataURL(blob);
  });
}

export async function fileToPendingImage(file: File | Blob, mimeHint?: string): Promise<PendingImage> {
  const mimeType = (file instanceof File ? file.type : mimeHint) || mimeHint || 'image/png';
  if (!mimeType.startsWith('image/')) {
    throw new Error('Not an image');
  }
  const dataBase64 = await blobToBase64(file);
  const previewUrl = URL.createObjectURL(file);
  return {
    id: crypto.randomUUID(),
    mimeType,
    previewUrl,
    dataBase64,
  };
}

export function revokePendingImages(images: PendingImage[]) {
  for (const img of images) {
    if (img.previewUrl.startsWith('blob:')) {
      URL.revokeObjectURL(img.previewUrl);
    }
  }
}
