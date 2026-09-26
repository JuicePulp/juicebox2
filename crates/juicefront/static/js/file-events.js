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
    const raw = localStorage.getItem("juicebox_uploads");
    if (!raw)
      return;
    const arr = JSON.parse(raw);
    if (!Array.isArray(arr))
      return;
    const next = arr.filter((f) => f && f.id !== fileId);
    if (next.length !== arr.length) {
      localStorage.setItem("juicebox_uploads", JSON.stringify(next));
    }
  } catch {}
}
