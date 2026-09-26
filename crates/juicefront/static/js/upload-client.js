import {
  startUpload,
  generateId,
  extractServerId
} from "./upload-engine.js";
import { UPLOAD_URL, TUS_THRESHOLD } from "./upload-config.js";
import { deleteTus } from "./tus.js";
const listeners = new Set;
let items = [];
const persistedIds = new Set;
const removedIds = new Set;
export function getUploads() {
  return items;
}
export function subscribe(listener) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
function publish() {
  const snapshot = items;
  for (const listener of listeners) {
    try {
      listener(snapshot);
    } catch {}
  }
}
function upsertItem(item) {
  const idx = items.findIndex((i) => i.id === item.id);
  items = idx >= 0 ? items.map((i, n) => n === idx ? item : i) : [...items, item];
  maybePersist(item);
  publish();
}
function removeItem(id) {
  items = items.filter((i) => i.id !== id);
  removedIds.add(id);
  publish();
}
export function wasRemoved(id) {
  return removedIds.has(id);
}
function reset(snapshot) {
  items = snapshot;
  for (const it of snapshot)
    maybePersist(it);
  publish();
}
let port = null;
function onWorkerMessage(e) {
  const msg = e.data || {};
  if (msg.type === "snapshot" && Array.isArray(msg.items)) {
    reset(msg.items);
  } else if (msg.type === "item" && msg.item) {
    upsertItem(msg.item);
  } else if (msg.type === "removed" && msg.id) {
    removeItem(msg.id);
  }
}
function ensurePort() {
  if (typeof window === "undefined") return null;
  if (port) return port;
  try {
    if (typeof SharedWorker === "undefined") return null;
    var sw = new SharedWorker(new URL("./upload-worker.js", import.meta.url), {
      name: "juicebox-uploads",
      type: "module",
    });
    sw.port.onmessage = onWorkerMessage;
    sw.port.start();
    port = sw.port;
  } catch {
    port = null;
  }
  return port;
}
const localItems = new Map;
const localHandles = new Map;
function localUpsert(patch) {
  let it = localItems.get(patch.id);
  if (!it) {
    it = { ...patch };
    localItems.set(patch.id, it);
  }
  Object.assign(it, patch);
  upsertItem({ ...it });
}
function startLocal(item, input) {
  localItems.set(item.id, { ...item });
  const handle = startUpload(item, input.file, {
    update: localUpsert,
    onTusCreate: () => {},
    onTusDelete: (tusId) => {
      deleteTus(UPLOAD_URL, tusId);
    }
  });
  localHandles.set(item.id, handle);
}
export function enqueueUpload(input) {
  const id = generateId();
  const item = {
    id,
    filename: input.file.name,
    mimeType: input.file.type || "application/octet-stream",
    size: input.file.size,
    method: input.file.size > TUS_THRESHOLD ? "tus" : "direct",
    state: "queued",
    progress: 0,
    ttlHours: input.ttlHours,
    quickLink: input.quickLink,
    customHost: input.customHost,
    uploadMode: input.uploadMode,
    createdAt: Date.now()
  };
  upsertItem({ ...item });
  const p = ensurePort();
  if (p) {
    try {
      const options = {
        ttlHours: input.ttlHours,
        customHost: input.customHost,
        uploadMode: input.uploadMode,
        quickLink: input.quickLink
      };
      p.postMessage({
        type: "enqueue",
        id,
        filename: item.filename,
        mimeType: item.mimeType,
        size: item.size,
        ttlHours: input.ttlHours,
        options,
        file: input.file
      });
    } catch {
      startLocal(item, input);
    }
  } else {
    startLocal(item, input);
  }
  return id;
}
export function connectUploads() {
  ensurePort();
}
export function cancelUpload(id) {
  const p = ensurePort();
  if (p) {
    try {
      p.postMessage({ type: "cancel", id });
    } catch {}
    return;
  }
  localHandles.get(id)?.abort();
}
export function removeUpload(id) {
  const p = ensurePort();
  if (p) {
    try {
      p.postMessage({ type: "remove", id });
    } catch {}
  }
  removeItem(id);
}
export function updateUpload(id, patch) {
  const p = ensurePort();
  if (p) {
    try {
      p.postMessage({ type: "patch", id, patch });
    } catch {}
    return;
  }
  localUpsert({ id, ...patch });
}
function maybePersist(item) {
  if (item.state !== "done" || !item.url)
    return;
  if (persistedIds.has(item.id))
    return;
  persistedIds.add(item.id);
  const write = () => {
    try {
      const stored = JSON.parse(localStorage.getItem("juicebox_uploads") || "[]");
      const arr = Array.isArray(stored) ? stored : [];
      const id = item.serverId || extractServerId(item.url ?? "") || item.id || Math.random().toString(36).slice(2);
      const entry = {
        id,
        filename: item.filename,
        mime_type: item.mimeType,
        size_bytes: item.size,
        uploaded_at: Math.floor(Date.now() / 1000),
        url: item.url,
        expires_at: Math.floor(Date.now() / 1000) + item.ttlHours * 3600,
        delete_token: item.deleteToken || ""
      };
      const deduped = arr.filter((e) => !(e && (e.id === id || e.id === "" && e.filename === item.filename && e.size_bytes === item.size)));
      deduped.push(entry);
      localStorage.setItem("juicebox_uploads", JSON.stringify(deduped));
    } catch {}
  };
  if (typeof navigator !== "undefined" && navigator.locks?.request) {
    navigator.locks.request("juicebox-uploads", () => {
      write();
      return Promise.resolve();
    }).catch(() => write());
  } else if (typeof localStorage !== "undefined") {
    write();
  }
}
