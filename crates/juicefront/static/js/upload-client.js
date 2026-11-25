import {
  startUpload,
  generateId,
  extractServerId
} from "./upload-engine.js?v=2.2.0-r13";
import {
  UPLOAD_URL,
  readUploadMode,
  uploadMethodFor,
  lastUploadBps
} from "./upload-config.js?v=2.2.0-r13";
import { deleteTus, uniqueTusIds } from "./tus.js?v=2.2.0-r13";
import { upsertLocalFile } from "./upload-storage.js?v=2.2.0-r13";
import {
  loadQueueItems,
  queueItem,
  queueItemNow,
  removeQueueItem,
  flushQueue
} from "./upload-store.js?v=2.2.0-r13";
import { announce, isTerminal } from "./util.js?v=2.2.0-r13";
import { E } from "./upload-errors.js?v=2.2.0-r13";
const listeners = new Set;
let items = [];
const persistedIds = new Set;
const persistPromises = new Map;
const removedIds = new Set;
// A reloaded page cannot reattach to an in-flight worker upload, so these become resumable.
const HYDRATED_ACTIVE_STATES = new Set(["queued", "compressing", "uploading", "finalizing"]);

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
      errorCode: 0,
      errorMessage: ""
    };
    // Do not write the paused state yet: another tab's worker may still be uploading it.
    // The worker snapshot promotes it back to active; a failed handshake leaves it paused.
    return paused;
  }
  if (record.state === "paused")
    return record;
  void removeQueueItem(record.id);
  return null;
}

if (typeof window !== "undefined") {
  try {
    const stored = await loadQueueItems();
    if (stored.length) {
      items = stored.map(hydrateRecord).filter(Boolean);
      items.sort((a, b) => (Number(a.createdAt) || 0) - (Number(b.createdAt) || 0));
    }
  } catch {}
  // `pagehide` rather than `beforeunload` so iOS Safari still flushes the queue.
  window.addEventListener("pagehide", (event) => {
    if (!event.persisted) {
      try { port?.postMessage({ type: "disconnect" }); } catch {}
    }
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
let workerRetryAt = 0;
let workerHeard = false;
let workerValidating = false;
const workerInputs = new Map;
const workerAckTimers = new Map;
const workerAcked = new Set;
// Only item/snapshot deliveries advance this — pong/probe-ok are answered on the
// tab's own port and cannot prove that broadcasts still reach any tab.
let lastItemAt = 0;
// The worker must ack an enqueue within this window or the upload restarts locally.
const WORKER_ACK_TIMEOUT_MS = 8000;
const HEARTBEAT_INTERVAL_MS = 8000;
// No item/snapshot for this long while an upload is tracked means the worker can
// answer pings but can no longer broadcast. Longer than any normal quiet phase
// (TUS health polls, ticket finalisation) so busy uploads are not retired.
const DEAF_WORKER_MS = 45000;
const PONG_TIMEOUT_MS = 6000;
const WORKER_PROBE_TIMEOUT_MS = 4000;
// A failed handshake disables the worker only briefly; another tab may have fixed it.
const WORKER_RETRY_MS = 30000;
let workerGeneration = 0;
const workerProbes = new Map();
function workerVersion() {
  try {
    return document.getElementById("server-config")?.getAttribute("data-version") || "";
  } catch {
    return "";
  }
}
function workerInstanceName() {
  const version = workerVersion() || "dev";
  return "juicebox-uploads-" + version + (workerGeneration ? "-g" + workerGeneration : "");
}
let heartbeatTimer = null;
let workerHelloTimer = null;
let pingStartedAt = 0;

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
  // Only this tab's enqueues count. A tab displaying another tab's upload must not
  // heartbeat for it, or a false timeout here would incorrectly pause that upload.
  if (!id || !workerInputs.has(id))
    return;
  clearWorkerTimer(id);
  workerAcked.add(id);
}
function forgetWorkerItem(id) {
  clearWorkerTimer(id);
  workerInputs.delete(id);
  workerAcked.delete(id);
}
function probeWorker(workerPort, timeoutMs) {
  return new Promise((resolve) => {
    const nonce = Date.now().toString(36) + Math.random().toString(36).slice(2);
    let timer = 0;
    const done = (ok) => {
      if (!workerProbes.has(nonce))
        return;
      workerProbes.delete(nonce);
      clearTimeout(timer);
      resolve(ok);
    };
    workerProbes.set(nonce, done);
    timer = setTimeout(() => done(false), timeoutMs);
    try {
      workerPort.postMessage({ type: "probe", nonce });
    } catch {
      done(false);
    }
  });
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
    // A worker can keep answering pings on this tab's port while its broadcast
    // Set is empty, so no item/snapshot ever arrives. A long silent stretch while
    // this tab has an upload in flight means it is deaf: retire it and fall back.
    if (workerInputs.size > 0 && lastItemAt && Date.now() - lastItemAt > DEAF_WORKER_MS) {
      restartWithoutWorker();
      return;
    }
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
function collectWorkerUploads() {
  const pending = [];
  for (const [id, input] of workerInputs) {
    if (!workerAcked.has(id))
      pending.push([id, input]);
  }
  return { pending, acked: [...workerAcked] };
}
function fallbackLocalUploads(pending, acked) {
  for (const [id, input] of pending) {
    const item = items.find((i) => i.id === id);
    if (!item || isTerminal(item.state))
      continue;
    startLocal({ ...item, state: "queued", progress: 0 }, input);
  }
  for (const id of acked) {
    const item = items.find((i) => i.id === id);
    if (item && !isTerminal(item.state)) {
      localUpsert({
        ...item,
        state: "paused",
        pausedAt: Date.now(),
        errorCode: 0,
        errorMessage: "",
      });
    }
  }
}
// Bumps the generation so the next ensurePort() opens a genuinely new
// SharedWorker (and new script URL), then restarts this tab's uploads locally.
function retireWithFallback() {
  const { pending, acked } = collectWorkerUploads();
  if (port) {
    // Cancel acked ids too: that transfer is still running in the worker we abandon.
    for (const id of [...pending.map(([id]) => id), ...acked]) {
      try { port.postMessage({ type: "cancel", id }); } catch {}
    }
  }
  retireWorker();
  fallbackLocalUploads(pending, acked);
}
// Worker-acked items restart as paused; that transfer may still be running.
function restartWithoutWorker() {
  pingStartedAt = 0;
  retireWithFallback();
  workerDead = true;
  workerRetryAt = Date.now() + WORKER_RETRY_MS;
}
function onWorkerMessage(e) {
  workerHeard = true;
  workerValidating = false;
  pingStartedAt = 0;
  const msg = e.data || {};
  if (msg.type === "probe-ok" && msg.nonce) {
    const done = workerProbes.get(msg.nonce);
    const expected = workerVersion();
    let ok = !expected || !msg.version || msg.version === expected;
    // A worker reporting a long broadcast idle while this tab has also seen no
    // item/snapshot is a second, independent signal that it has gone deaf.
    if (ok && (workerInputs.size || workerAcked.size) && typeof msg.idleMs === "number" && Date.now() - lastItemAt > DEAF_WORKER_MS && msg.idleMs > DEAF_WORKER_MS)
      ok = false;
    if (done)
      done(ok);
    return;
  }
  if (msg.type === "snapshot" && Array.isArray(msg.items)) {
    lastItemAt = Date.now();
    const active = [];
    for (const it of msg.items) {
      if (!it || !it.id)
        continue;
      noteWorkerAlive(it.id);
      if (isTerminal(it.state)) {
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
    lastItemAt = Date.now();
    noteWorkerAlive(msg.item.id);
    if (isTerminal(msg.item.state))
      forgetWorkerItem(msg.item.id);
    upsertItem(msg.item);
  } else if (msg.type === "removed" && msg.id) {
    forgetWorkerItem(msg.id);
    removeItem(msg.id);
  }
}
// Cache-busts the worker module after a deploy so a stale upload engine cannot persist.
function workerScriptUrl() {
  const url = new URL("./upload-worker.js", import.meta.url);
  const version = workerVersion();
  if (version)
    url.searchParams.set("v", version);
  if (workerGeneration)
    url.searchParams.set("g", String(workerGeneration));
  return url;
}
// One SharedWorker per version; no hello within 4 s means upload locally.
function ensurePort() {
  if (typeof window === "undefined") return null;
  if (workerDead) {
    if (Date.now() < workerRetryAt) return null;
    workerDead = false;
    pingStartedAt = 0;
  }
  if (port) return port;
  try {
    if (typeof SharedWorker === "undefined") return null;
    var sw = new SharedWorker(workerScriptUrl(), {
      name: workerInstanceName(),
      type: "module",
    });
    sw.port.onmessage = onWorkerMessage;
    // ping/probe are answered on this port directly; an error or an uncloneable
    // message here means the protocol is broken even if pings still succeed.
    const onWorkerError = () => {
      restartWithoutWorker();
    };
    sw.onerror = onWorkerError;
    sw.onmessageerror = onWorkerError;
    sw.port.onmessageerror = onWorkerError;
    sw.port.onerror = onWorkerError;
    sw.port.start();
    port = sw.port;
    lastItemAt = Date.now();
    startHeartbeat();
    workerValidating = true;
    workerHeard = false;
    if (workerHelloTimer)
      clearTimeout(workerHelloTimer);
    workerHelloTimer = setTimeout(() => {
      workerHelloTimer = null;
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
function retireWorker() {
  pingStartedAt = 0;
  if (workerHelloTimer) {
    clearTimeout(workerHelloTimer);
    workerHelloTimer = null;
  }
  if (heartbeatTimer) {
    clearInterval(heartbeatTimer);
    heartbeatTimer = null;
  }
  for (const id of workerInputs.keys())
    clearWorkerTimer(id);
  workerInputs.clear();
  workerAcked.clear();
  for (const done of workerProbes.values())
    done(false);
  workerProbes.clear();
  if (port) {
    try { port.close(); } catch {}
  }
  port = null;
  workerDead = false;
  workerRetryAt = 0;
  workerGeneration += 1;
}
async function checkWorker() {
  let p = ensurePort();
  if (!p)
    return null;
  if (await probeWorker(p, WORKER_PROBE_TIMEOUT_MS))
    return p;
  const hasUploads = workerInputs.size > 0 || workerAcked.size > 0;
  // A worker answering probes but delivering no item/snapshot for two ack
  // windows is deaf, not busy. Only a worker with fresh item activity is kept.
  const stale = !lastItemAt || Date.now() - lastItemAt > DEAF_WORKER_MS;
  if (hasUploads && !stale)
    return null;
  if (hasUploads)
    retireWithFallback();
  else
    retireWorker();
  p = ensurePort();
  if (p && await probeWorker(p, WORKER_PROBE_TIMEOUT_MS))
    return p;
  retireWorker();
  workerDead = true;
  workerRetryAt = Date.now() + WORKER_RETRY_MS;
  return null;
}
let workerCheck = null;
function ensureHealthyPort() {
  if (!workerCheck) {
    workerCheck = checkWorker().finally(() => {
      workerCheck = null;
    });
  }
  return workerCheck;
}
// Control APIs are synchronous while ensureHealthyPort() is async, so at minimum
// never post them to a port already known to be dead; callers fall back locally.
function controlPort() {
  if (workerDead || !port)
    return null;
  return port;
}
const localItems = new Map;
const localHandles = new Map;
function localUpsert(patch) {
  // Safe mode keeps a failed upload resumable instead of surfacing a hard error.
  if (patch.state === "error") {
    const existing = localItems.get(patch.id) || items.find((entry) => entry.id === patch.id);
    const safe = patch.safeUpload ?? existing?.safeUpload;
    if (safe) {
      patch = {
        ...patch,
        state: "paused",
        pausedAt: Date.now(),
        errorCode: 0,
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
/** Starts an upload and returns its queue id; an active duplicate of the same file is reused. */
export async function enqueueUpload(input) {
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
    method: uploadMethodFor(clientMode, input.file.size, lastUploadBps(), input.customHost),
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
  const p = await ensureHealthyPort();
  if (removedIds.has(id))
    return id;
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

/** Resumes a paused item; returns `""` when the file, fingerprint, or required password does not match. */
export async function resumeUpload(id, file, options = {}, fingerprint = "") {
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
    errorCode: 0,
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
  const p = await ensureHealthyPort();
  if (removedIds.has(id))
    return id;
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
  void ensureHealthyPort();
}
export function cancelUpload(id) {
  const p = controlPort();
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
  const p = controlPort();
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
  const p = controlPort();
  if (p) {
    try {
      p.postMessage({ type: "patch", id, patch });
    } catch {}
    return;
  }
  localUpsert({ id, ...patch });
}
// The cross-tab lock serializes the read-modify-write so tabs do not lose each other's rows.
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
