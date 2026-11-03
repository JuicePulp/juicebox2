const STORAGE_KEY = "juicebox_uploads";

export function readStoredFiles() {
  if (typeof localStorage === "undefined")
    return [];
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw)
      return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

/// Insert or update one file entry. Doing this by id stops the upload card and
/// files card from appending duplicate rows for the same server file.
export function upsertLocalFile(entry) {
  if (!entry || !entry.id || typeof localStorage === "undefined")
    return;
  try {
    const arr = readStoredFiles();
    let idx = arr.findIndex((f) => f && f.id === entry.id);
    if (idx < 0 && entry.delete_token)
      idx = arr.findIndex((f) => f && f.delete_token === entry.delete_token);
    if (idx >= 0) {
      const merged = { ...arr[idx] };
      for (const [key, value] of Object.entries(entry)) {
        if (value !== undefined)
          merged[key] = value;
      }
      arr[idx] = merged;
    } else {
      arr.push(entry);
    }
    localStorage.setItem(STORAGE_KEY, JSON.stringify(arr));
  } catch {}
}
