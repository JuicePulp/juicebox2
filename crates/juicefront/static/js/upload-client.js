import {
  startUpload,
  generateId,
  extractServerId
} from "./upload-engine.js?v=2.1.32";
import {
  UPLOAD_URL,
  readUploadMode,
  uploadMethodFor,
  LAST_SPEED_KEY
} from "./upload-config.js?v=2.1.32";
import { deleteTus } from "./tus.js";
import { upsertLocalFile } from "./upload-storage.js";
import { announce } from "./util.js";
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
  const merged = new Map(items.map((it) => [it.id, it]));
  for (const it of snapshot)
    merged.set(it.id, it);
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
    if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function")
      return "juicebox-uploads-" + crypto.randomUUID();
  } catch {}
  return "juicebox-uploads-" + Date.now().toString(36) + "-" + Math.random().toString(36).slice(2);
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
  // Already-acknowledged uploads may be mid-flight; don't duplicate them.
  for (const id of acked) {
    const item = items.find((i) => i.id === id);
    if (item && !isTerminal(item.state)) {
      localUpsert({
        ...item,
        state: "error",
        errorCode: "WORKER_LOST",
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
    // A unique name per document load guarantees a fresh worker. A reload
    // must never reconnect to a worker that was already wedged or abandoned.
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
    onTusCreate: () => undefined,
    onTusDelete: (tusId) => {
      deleteTus(UPLOAD_URL, tusId);
    }
  });
  localHandles.set(item.id, handle);
}
function findActiveDuplicate(file) {
  if (!file)
    return null;
  return items.find((i) => i.filename === file.name && i.size === file.size && i.lastModified === file.lastModified && !isTerminal(i.state)) || null;
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
    customHost: input.customHost,
    uploadMode: input.uploadMode,
    password: input.password || "",
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
        quickLink: input.quickLink,
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
  const p = ensurePort();
  if (p) {
    try {
      p.postMessage({ type: "remove", id });
    } catch {}
  }
  localHandles.get(id)?.abort();
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
    return;
  if (persistedIds.has(item.id))
    return;
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
  if (typeof navigator !== "undefined" && navigator.locks?.request) {
    navigator.locks.request("juicebox-uploads", () => {
      write();
      return Promise.resolve();
    }).catch(() => write());
  } else if (typeof localStorage !== "undefined") {
    write();
  }
}
