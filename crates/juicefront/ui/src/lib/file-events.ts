type FileDeletedListener = (fileId: string) => void;

const listeners = new Set<FileDeletedListener>();

export function onFileDeleted(listener: FileDeletedListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function emitFileDeleted(fileId: string) {
  for (const listener of listeners) {
    try {
      listener(fileId);
    } catch {}
  }
}

export function removeLocalFile(fileId: string) {
  if (typeof localStorage === "undefined") return;
  try {
    const raw = localStorage.getItem("juicebox_uploads");
    if (!raw) return;
    const arr = JSON.parse(raw);
    if (!Array.isArray(arr)) return;
    const next = arr.filter((f) => f && f.id !== fileId);
    if (next.length !== arr.length) {
      localStorage.setItem("juicebox_uploads", JSON.stringify(next));
    }
  } catch {}
}
