// Thin fetch wrappers for the `/studio/*` endpoints (not part of the OpenAPI client).

import { buildMartinUrl } from '@/lib/api';
import type {
  BrowseEntry,
  GenerateRequest,
  Job,
  RasterInfo,
  StudioConfig,
  ValidationReport,
} from './types';

async function getJson<T>(path: string): Promise<T> {
  const res = await fetch(buildMartinUrl(path));
  if (!res.ok) {
    throw new Error(`${path} failed: ${res.status} ${res.statusText}`);
  }
  return res.json() as Promise<T>;
}

async function postJson<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(buildMartinUrl(path), {
    body: JSON.stringify(body),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    const message =
      data && typeof data === 'object' && 'error' in data
        ? String((data as { error: unknown }).error)
        : `${path} failed: ${res.status}`;
    throw new Error(message);
  }
  return data as T;
}

export interface UploadResult {
  name: string;
  rel_path: string;
  size: number;
}

/**
 * Upload a single file as a raw octet-stream body, with its name in the query
 * string. Uses XHR so we can surface upload progress (fetch has no upload
 * progress events).
 */
function uploadFile(
  path: string,
  file: File,
  onProgress?: (pct: number) => void,
): Promise<UploadResult> {
  return new Promise((resolve, reject) => {
    const url = buildMartinUrl(`${path}?name=${encodeURIComponent(file.name)}`);
    const xhr = new XMLHttpRequest();
    xhr.open('POST', url);
    xhr.setRequestHeader('Content-Type', 'application/octet-stream');
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable && onProgress) onProgress(Math.round((e.loaded / e.total) * 100));
    };
    xhr.onload = () => {
      let data: unknown = {};
      try {
        data = JSON.parse(xhr.responseText);
      } catch {
        /* non-JSON body */
      }
      if (xhr.status >= 200 && xhr.status < 300) {
        resolve(data as UploadResult);
      } else {
        const message =
          data && typeof data === 'object' && 'error' in data
            ? String((data as { error: unknown }).error)
            : `${path} failed: ${xhr.status}`;
        reject(new Error(message));
      }
    };
    xhr.onerror = () => reject(new Error(`${path} failed: network error`));
    xhr.send(file);
  });
}

export const studioApi = {
  browse: () => getJson<BrowseEntry[]>('/studio/browse'),
  config: () => getJson<StudioConfig>('/studio/config'),
  generate: (req: GenerateRequest) =>
    postJson<{ job_id: string }>('/studio/generate', req),
  inspect: (inputs: string[]) => postJson<RasterInfo[]>('/studio/inspect', { inputs }),
  job: (id: string) => getJson<Job>(`/studio/jobs/${id}`),
  upload: (file: File, onProgress?: (pct: number) => void) =>
    uploadFile('/studio/upload', file, onProgress),
  validate: (path: string) => postJson<ValidationReport>('/studio/validate', { path }),
};
