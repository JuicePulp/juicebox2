import { readStoredFiles } from "./upload-storage.js";
export { upsertLocalFile } from "./upload-storage.js";
const STORAGE_KEY = "juicebox_uploads";
const listeners = new Set;

export function onFileDeleted(listener) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
export function emitFileDeleted(fileId) {
  for (const listener of listeners) {
    try {
      listener(fileId);
    } catch {}
  }
}
export function removeLocalFile(fileId) {
  if (typeof localStorage === "undefined")
    return;
  try {
    const arr = readStoredFiles();
    const next = arr.filter((f) => f && f.id !== fileId);
    if (next.length !== arr.length) {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
    }
  } catch {}
}
