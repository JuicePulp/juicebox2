import {
  startUpload,
  generateId,
  extractServerId
} from "./upload-engine.js?v=2.2.0";
import {
  UPLOAD_URL,
  readUploadMode,
  uploadMethodFor,
  LAST_SPEED_KEY
} from "./upload-config.js?v=2.2.0";
import { deleteTus } from "./tus.js";
import { upsertLocalFile } from "./upload-storage.js";
import {
  loadQueueItems,
  queueItem,
  queueItemNow,
  removeQueueItem,
  flushQueue
} from "./upload-store.js?v=2.2.0";
import { announce } from "./util.js";
const listeners = new Set;
let items = [];
const persistedIds = new Set;
const persistPromises = new Map;
const removedIds = new Set;
const HYDRATED_ACTIVE_STATES = new Set(["queued", "compressing", "uploading", "finalizing"]);

function uniqueTusIds(item) {
  if (!item) return [];
  const ids = [...(Array.isArray(item.tusIds) ? item.tusIds : []), item.tusId];
  return [...new Set(ids.filter((id) => typeof id === "string" && id))];
}

function hydrateRecord(record) {
  if (!record || !record.id)
    return null;
  if (record.state === "done") {
    maybePersist(record).then(() => {
      void removeQueueItem(record.id);
      setTimeout(() => void removeQueueItem(record.id), 5000);
    });
    return null;
  }
  if (record.state === "error" || record.state === "cancelled") {
    void removeQueueItem(record.id);
    return null;
  }
  if (HYDRATED_ACTIVE_STATES.has(record.state)) {
    const paused = {
      ...record,
      state: "paused",
      pausedAt: Date.now(),
      errorCode: "",
      errorMessage: ""
    };
    queueItemNow(paused);
    return paused;
  }
  if (record.state === "paused")
    return record;
  void removeQueueItem(record.id);
  return null;
}

// Restore the queue before upload-card subscribes, so a reload paints the
// interrupted rows immediately instead of flashing an empty list. A live
// SharedWorker snapshot will switch the rows back to "uploading" if the worker
// survived the navigation.
if (typeof window !== "undefined") {
  try {
    const stored = await loadQueueItems();
    if (stored.length) {
      items = stored.map(hydrateRecord).filter(Boolean);
      items.sort((a, b) => (Number(a.createdAt) || 0) - (Number(b.createdAt) || 0));
    }
  } catch {}
  window.addEventListener("pagehide", () => {
    void flushQueue();
  });
}
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
  if (!item || removedIds.has(item.id))
    return;
  const idx = items.findIndex((i) => i.id === item.id);
  items = idx >= 0 ? items.map((i, n) => n === idx ? item : i) : [...items, item];
  const persisted = maybePersist(item);
  if (item.state === "error" || item.state === "cancelled") {
    void removeQueueItem(item.id);
  } else if (item.state === "done") {
    // Persist the completion briefly so a page that closes during the
    // localStorage write can still recover it on the next load. Once the write
    // completes, remove the queue record so deleted files cannot come back.
    queueItemNow(item);
    persisted.then(() => {
      void removeQueueItem(item.id);
      setTimeout(() => void removeQueueItem(item.id), 5000);
    });
  } else {
    queueItem(item);
  }
  publish();
}
function removeItem(id) {
  items = items.filter((i) => i.id !== id);
  removedIds.add(id);
  void removeQueueItem(id);
  publish();
}
export function wasRemoved(id) {
  return removedIds.has(id);
}
function reset(snapshot) {
  const merged = new Map(items.map((it) => [it.id, it]));
  for (const it of snapshot) {
    if (!it || !it.id || removedIds.has(it.id))
      continue;
    merged.set(it.id, it);
    if (!isTerminal(it.state))
      queueItem(it);
  }
  items = [...merged.values()];
  publish();
}
let port = null;
let workerDead = false;
let workerHeard = false;
let workerValidating = false;
const workerInputs = new Map;
const workerAckTimers = new Map;
const workerAcked = new Set;
const WORKER_ACK_TIMEOUT_MS = 8000;
const HEARTBEAT_INTERVAL_MS = 8000;
const PONG_TIMEOUT_MS = 6000;
const WORKER_INSTANCE_NAME = (() => {
  try {
    const version = document.getElementById("server-config")?.getAttribute("data-version") || "";
    if (version)
      return "juicebox-uploads-" + version;
  } catch {}
  return "juicebox-uploads";
})();
let heartbeatTimer = null;
let pingStartedAt = 0;

function isTerminal(state) {
  return state === "done" || state === "error" || state === "cancelled";
}
function clearWorkerTimer(id) {
  const timer = workerAckTimers.get(id);
  if (timer) {
    clearTimeout(timer);
    workerAckTimers.delete(id);
  }
}
function noteWorkerAlive(id) {
  workerHeard = true;
  workerValidating = false;
  if (!id)
    return;
  clearWorkerTimer(id);
  workerAcked.add(id);
}
function forgetWorkerItem(id) {
  clearWorkerTimer(id);
  workerInputs.delete(id);
  workerAcked.delete(id);
}
function startHeartbeat() {
  if (heartbeatTimer)
    return;
  heartbeatTimer = setInterval(() => {
    if (!port || workerDead) {
      clearInterval(heartbeatTimer);
      heartbeatTimer = null;
      return;
    }
    if (typeof document !== "undefined" && document.hidden) {
      pingStartedAt = 0;
      return;
    }
    if (workerInputs.size === 0 && workerAcked.size === 0)
      return;
    if (pingStartedAt) {
      if (Date.now() - pingStartedAt >= PONG_TIMEOUT_MS)
        restartWithoutWorker();
      return;
    }
    pingStartedAt = Date.now();
    try {
      port.postMessage({ type: "ping" });
    } catch {
      restartWithoutWorker();
    }
  }, HEARTBEAT_INTERVAL_MS);
}
function restartWithoutWorker() {
  pingStartedAt = 0;
  if (heartbeatTimer) {
    clearInterval(heartbeatTimer);
    heartbeatTimer = null;
  }
  const pending = [];
  for (const [id, input] of workerInputs) {
    if (!workerAcked.has(id))
      pending.push([id, input]);
  }
  const acked = [...workerAcked];
  if (port) {
    for (const [id] of pending) {
      try { port.postMessage({ type: "cancel", id }); } catch {}
    }
    try { port.close(); } catch {}
  }
  port = null;
  workerDead = true;
  workerValidating = false;
  for (const id of workerInputs.keys())
    clearWorkerTimer(id);
  workerInputs.clear();
  workerAcked.clear();

  // Uploads the worker never acknowledged can safely be run here.
  for (const [id, input] of pending) {
    const item = items.find((i) => i.id === id);
    if (!item || isTerminal(item.state))
      continue;
    startLocal({ ...item, state: "queued", progress: 0 }, input);
  }
  // Already-acknowledged uploads may be mid-flight. The page no longer holds
  // the File object, so keep the row and the TUS offset and ask for the file
  // again instead of starting a second upload.
  for (const id of acked) {
    const item = items.find((i) => i.id === id);
    if (item && !isTerminal(item.state)) {
      localUpsert({
        ...item,
        state: "paused",
        pausedAt: Date.now(),
        errorCode: "",
        errorMessage: "",
      });
    }
  }
}
function onWorkerMessage(e) {
  workerHeard = true;
  workerValidating = false;
  pingStartedAt = 0;
  const msg = e.data || {};
  if (msg.type === "snapshot" && Array.isArray(msg.items)) {
    const active = [];
    for (const it of msg.items) {
      if (!it || !it.id)
        continue;
      noteWorkerAlive(it.id);
      if (isTerminal(it.state)) {
        // Completed entries belong to the files history, not the upload queue.
        // Replaying them here would duplicate rows, re-copy URLs over the
        // clipboard, and resurrect files the user already deleted.
        forgetWorkerItem(it.id);
      } else {
        active.push(it);
      }
    }
    reset(active);
    const pausedIds = items
      .filter((it) => it && it.id && it.state === "paused")
      .map((it) => it.id);
    if (pausedIds.length) {
      try {
        port.postMessage({ type: "status", ids: pausedIds });
      } catch {}
    }
  } else if (msg.type === "item" && msg.item) {
    noteWorkerAlive(msg.item.id);
    if (isTerminal(msg.item.state))
      forgetWorkerItem(msg.item.id);
    upsertItem(msg.item);
  } else if (msg.type === "removed" && msg.id) {
    forgetWorkerItem(msg.id);
    removeItem(msg.id);
  }
}
function workerScriptUrl() {
  const url = new URL("./upload-worker.js", import.meta.url);
  try {
    const version = document.getElementById("server-config")?.getAttribute("data-version") || "";
    if (version)
      url.searchParams.set("v", version);
  } catch {}
  return url;
}
function ensurePort() {
  if (typeof window === "undefined") return null;
  if (workerDead) return null;
  if (port) return port;
  try {
    if (typeof SharedWorker === "undefined") return null;
    // Stable per-asset-version name: a reload reconnects to the worker that
    // already owns the upload, while a new asset version always gets a fresh
    // worker instead of inheriting stale code.
    var sw = new SharedWorker(workerScriptUrl(), {
      name: WORKER_INSTANCE_NAME,
      type: "module",
    });
    sw.port.onmessage = onWorkerMessage;
    sw.onerror = () => {
      restartWithoutWorker();
    };
    sw.port.start();
    port = sw.port;
    startHeartbeat();
    workerValidating = true;
    workerHeard = false;
    setTimeout(() => {
      if (!workerHeard)
        restartWithoutWorker();
      else
        workerValidating = false;
    }, 4000);
  } catch {
    port = null;
  }
  return port;
}
const localItems = new Map;
const localHandles = new Map;
function localUpsert(patch) {
  if (patch.state === "error") {
    const existing = localItems.get(patch.id) || items.find((entry) => entry.id === patch.id);
    const safe = patch.safeUpload ?? existing?.safeUpload;
    if (safe) {
      patch = {
        ...patch,
        state: "paused",
        pausedAt: Date.now(),
        errorCode: "",
        errorMessage: ""
      };
    }
  }
  let it = localItems.get(patch.id);
  if (!it) {
    it = { ...patch };
    localItems.set(patch.id, it);
  }
  Object.assign(it, patch);
  upsertItem({ ...it });
}
function trackTusCreate(id, tusId, info) {
  if (!id || !tusId)
    return;
  const current = localItems.get(id) || items.find((it) => it.id === id);
  if (!current)
    return;
  const patch = {
    id,
    tusIds: [...new Set([...uniqueTusIds(current), tusId])]
  };
  if (info && info.kind === "stream")
    patch.tusId = tusId;
  localUpsert(patch);
}

function startLocal(item, input) {
  localItems.set(item.id, { ...item });
  const handle = startUpload(item, input.file, {
    update: localUpsert,
    onTusCreate: (tusId, info) => trackTusCreate(item.id, tusId, info),
    onTusDelete: (tusId) => {
      deleteTus(UPLOAD_URL, tusId);
    }
  });
  localHandles.set(item.id, handle);
}
function findActiveDuplicate(file) {
  if (!file)
    return null;
  return items.find((i) => i.filename === file.name && i.size === file.size && i.lastModified === file.lastModified && !isTerminal(i.state) && i.state !== "paused") || null;
}
function matchesPausedFile(item, file, fingerprint) {
  if (!item || item.state !== "paused" || !file)
    return false;
  if (item.fingerprint && fingerprint)
    return item.fingerprint === fingerprint;
  return item.filename === file.name && item.size === file.size && item.lastModified === file.lastModified;
}
export function findPausedUpload(file, fingerprint) {
  return items.find((item) => matchesPausedFile(item, file, fingerprint)) || null;
}
function lastUploadBps() {
  try {
    const stored = Number(localStorage.getItem(LAST_SPEED_KEY));
    return Number.isFinite(stored) && stored > 0 ? stored : 0;
  } catch {
    return 0;
  }
}
export function enqueueUpload(input) {
  const duplicate = findActiveDuplicate(input.file);
  if (duplicate) {
    try {
      announce(`${input.file.name} is already uploading`);
    } catch {}
    return duplicate.id;
  }
  const id = generateId();
  const clientMode = readUploadMode();
  const item = {
    id,
    filename: input.file.name,
    mimeType: input.file.type || "application/octet-stream",
    size: input.file.size,
    lastModified: input.file.lastModified,
    clientMode,
    method: uploadMethodFor(clientMode, input.file.size, lastUploadBps()),
    state: "queued",
    progress: 0,
    ttlHours: input.ttlHours,
    quickLink: input.quickLink,
    safeUpload: !!input.safeUpload,
    customHost: input.customHost,
    uploadMode: input.uploadMode,
    password: input.password || "",
    requiresPassword: !!input.password,
    fingerprint: input.fingerprint || "",
    createdAt: Date.now()
  };
  upsertItem({ ...item });
  queueItemNow(item);
  const p = ensurePort();
  if (p) {
    try {
      const options = {
        ttlHours: input.ttlHours,
        customHost: input.customHost,
        uploadMode: input.uploadMode,
        quickLink: input.quickLink,
        safeUpload: !!input.safeUpload,
        password: input.password || ""
      };
      p.postMessage({
        type: "enqueue",
        id,
        filename: item.filename,
        mimeType: item.mimeType,
        size: item.size,
        lastModified: item.lastModified,
        clientMode,
        ttlHours: input.ttlHours,
        options,
        item,
        file: input.file
      });
      workerInputs.set(id, input);
      workerAcked.delete(id);
      clearWorkerTimer(id);
      workerAckTimers.set(id, setTimeout(() => {
        workerAckTimers.delete(id);
        if (!workerAcked.has(id))
          restartWithoutWorker();
      }, WORKER_ACK_TIMEOUT_MS));
    } catch {
      forgetWorkerItem(id);
      startLocal(item, input);
    }
  } else {
    startLocal(item, input);
  }
  return id;
}

export function resumeUpload(id, file, options = {}, fingerprint = "") {
  const current = items.find((item) => item.id === id);
  if (!current || !file)
    return "";
  if (current.fingerprint && fingerprint && current.fingerprint !== fingerprint)
    return "";
  if (current.requiresPassword && !options.password)
    return "";
  const next = {
    ...current,
    filename: file.name,
    mimeType: file.type || current.mimeType || "application/octet-stream",
    size: file.size,
    lastModified: file.lastModified,
    fingerprint: fingerprint || current.fingerprint || "",
    safeUpload: !!current.safeUpload,
    state: "queued",
    password: options.password || "",
    resumeAttempt: true,
    pausedAt: 0,
    errorCode: "",
    errorMessage: ""
  };
  upsertItem(next);
  queueItemNow(next);
  const input = {
    file,
    ttlHours: next.ttlHours,
    customHost: next.customHost,
    uploadMode: next.uploadMode,
    quickLink: next.quickLink,
    safeUpload: !!next.safeUpload,
    password: next.password || ""
  };
  const p = ensurePort();
  if (p) {
    try {
      p.postMessage({
        type: "enqueue",
        id,
        filename: next.filename,
        mimeType: next.mimeType,
        size: next.size,
        lastModified: next.lastModified,
        clientMode: next.clientMode,
        ttlHours: next.ttlHours,
        options: { password: next.password || "" },
        item: next,
        file
      });
      workerInputs.set(id, input);
      workerAcked.delete(id);
      clearWorkerTimer(id);
      workerAckTimers.set(id, setTimeout(() => {
        workerAckTimers.delete(id);
        if (!workerAcked.has(id))
          restartWithoutWorker();
      }, WORKER_ACK_TIMEOUT_MS));
    } catch {
      forgetWorkerItem(id);
      startLocal(next, input);
    }
  } else {
    startLocal(next, input);
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
    forgetWorkerItem(id);
    return;
  }
  localHandles.get(id)?.abort();
  forgetWorkerItem(id);
}
export function removeUpload(id) {
  const item = items.find((it) => it.id === id);
  if (item) {
    for (const tusId of uniqueTusIds(item))
      deleteTus(UPLOAD_URL, tusId);
  }
  const p = ensurePort();
  if (p) {
    try {
      p.postMessage({ type: "remove", id });
    } catch {}
  }
  localHandles.get(id)?.abort();
  localHandles.delete(id);
  localItems.delete(id);
  forgetWorkerItem(id);
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
    return Promise.resolve(false);
  const existing = persistPromises.get(item.id);
  if (existing)
    return existing;
  if (persistedIds.has(item.id))
    return Promise.resolve(true);
  persistedIds.add(item.id);
  const write = () => {
    const id = item.serverId || extractServerId(item.url ?? "") || item.id || Math.random().toString(36).slice(2);
    const uploadedAt = Math.floor((item.createdAt || Date.now()) / 1000);
    upsertLocalFile({
      id,
      filename: item.filename,
      mime_type: item.mimeType,
      size_bytes: item.size,
      uploaded_at: uploadedAt,
      url: item.url,
      raw_url: item.rawUrl || "",
      download_url: item.downloadUrl || "",
      expires_at: uploadedAt + item.ttlHours * 3600,
      delete_token: item.deleteToken || ""
    });
  };
  let promise;
  if (typeof navigator !== "undefined" && navigator.locks?.request) {
    promise = navigator.locks.request("juicebox-uploads", () => {
      write();
      return Promise.resolve();
    }).then(() => true).catch(() => {
      write();
      return true;
    });
  } else if (typeof localStorage !== "undefined") {
    write();
    promise = Promise.resolve(true);
  } else {
    promise = Promise.resolve(false);
  }
  persistPromises.set(item.id, promise);
  return promise;
}
