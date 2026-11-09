const enc = encodeURIComponent;

export const apiHealth = "/api/health";
export const apiPresence = "/api/presence";

export function publicFile(id) {
  return `/file/${enc(id)}`;
}

export function publicFileInfo(id) {
  return `/file/${enc(id)}/info`;
}

export function publicFileRename(id) {
  return `/file/${enc(id)}/rename`;
}

export function publicFileRenew(id) {
  return `/file/${enc(id)}/renew`;
}

export const apiFetch = "/api/fetch";

export function apiFetchJob(jobId) {
  return `/api/fetch/${jobId}`;
}

export function apiPairDevice(deviceId) {
  return `/api/device/${enc(deviceId)}`;
}

export const apiPairGenerate = "/api/pair/generate";
export const apiPairDevices = "/api/device";

export function tusSession(id) {
  return `/api/tus/${id}`;
}
