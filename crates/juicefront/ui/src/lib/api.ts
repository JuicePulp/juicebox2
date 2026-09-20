/**
 * Shared API path builders for bundled client code. Bases (`UPLOAD_URL`
 * etc.) stay in `upload-config.ts`; these are path-only, resolved
 * same-origin through the Astro proxy in browsers. Admin pages still use
 * inline URLs in classic scripts; migrate them here when touched.
 */

const enc = encodeURIComponent;

export const apiHealth = "/api/health";
export const apiPresence = "/api/presence";

export function publicFile(id: string): string {
  return `/file/${id}`;
}

export function publicFileInfo(id: string): string {
  return `/file/${id}/info`;
}

export function publicFileRenew(id: string): string {
  return `/file/${id}/renew`;
}

export function apiOwnedFiles(): string {
  return "/api/owned-files";
}

export const apiFetch = "/api/fetch";

export function apiFetchJob(jobId: string): string {
  return `/api/fetch/${jobId}`;
}

export function apiPairDevice(deviceId: string): string {
  return `/api/device/${enc(deviceId)}`;
}

export const apiPairGenerate = "/api/pair/generate";
export const apiPairDevices = "/api/device";

export function tusSession(id: string): string {
  return `/api/tus/${id}`;
}
