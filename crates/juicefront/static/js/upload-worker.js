import {
  startUpload
} from "./upload-engine.js?v=2.2.0-r13";
import { UPLOAD_URL, setUploadModeHint } from "./upload-config.js?v=2.2.0-r13";
import { deleteTus, uniqueTusIds } from "./tus.js?v=2.2.0-r13";
import { E } from "./upload-errors.js?v=2.2.0-r13";
import {
  queueItem,
  queueItemNow,
  removeQueueItem
} from "./upload-store.js?v=2.2.0-r13";
const ctx = self;
const WORKER_VERSION = (() => {
  try {
    return new URL(import.meta.url).searchParams.get("v") || "";
  } catch {
    return "";
  }
})();
const entries = new Map;
// A removed id must stay gone even if a late engine callback still reports it.
const dismissed = new Set;
const ports = new Set;
// Bounds worker memory; finished entries are evicted oldest-first once this is exceeded.
const MAX_ITEMS = 100;
const ACTIVE_STATES = new Set(["queued", "compressing", "uploading", "finalizing"]);
// Latches when a broadcast last reached at least one port; reported to clients so a
// worker that answers direct probe/ping messages but cannot broadcast is detectable.
let lastEmitOkAt = Date.now();
function emit(message) {
  let payload;
  try {
    // Clone once outside the loop: an uncloneable payload must never purge the
    // Set of otherwise healthy ports.
    payload = typeof structuredClone === "function" ? structuredClone(message) : message;
  } catch (err) {
    try {
      console.error("upload worker: could not clone message", { type: message?.type, name: err?.name, err });
    } catch {}
    return;
  }
  for (const port of ports) {
    try {
      port.postMessage(payload);
      lastEmitOkAt = Date.now();
    } catch (err) {
      // Only the port that actually threw is closed; keep every other port.
      ports.delete(port);
      try {
        console.error("upload worker: postMessage failed", { type: message?.type, name: err?.name, err });
      } catch {}
    }
  }
}
function entryItem(item) {
  if (dismissed.has(item.id))
    return;
  emit({ type: "item", item });
}
function upsert(patch) {
  if (dismissed.has(patch.id))
    return;
  if (patch.state === "error") {
    const existing = entries.get(patch.id);
    const safe = patch.safeUpload ?? existing?.item?.safeUpload;
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
  let entry = entries.get(patch.id);
  if (!entry) {
    entry = { item: {}, handle: null, tusIds: [] };
    entries.set(patch.id, entry);
  }
  Object.assign(entry.item, patch);
  const state = entry.item.state;
  if (state === "done") {
    queueItemNow(entry.item);
  } else if (state === "error" || state === "cancelled") {
    void removeQueueItem(entry.item.id);
  } else {
    queueItem(entry.item);
  }
  entryItem(entry.item);
  if (!ACTIVE_STATES.has(state)) {
    entry.handle = null;
    entry.tusIds = [];
  }
  trim();
}
function trim() {
  if (entries.size <= MAX_ITEMS)
    return;
  const finished = [...entries.values()].filter((e) => e.item.state === "done" || e.item.state === "error" || e.item.state === "cancelled").sort((a, b) => a.item.createdAt - b.item.createdAt);
  while (entries.size > MAX_ITEMS && finished.length) {
    const oldest = finished.shift();
    entries.delete(oldest.item.id);
  }
}
function handleEnqueue(msg, _port) {
  if (entries.has(msg.id)) {
    const existing = entries.get(msg.id);
    const state = existing?.item?.state;
    const paused = state === "paused";
    const terminal = state === "done" || state === "error" || state === "cancelled";
    if (paused || (!terminal && msg.file)) {
      // Paused items resume from the saved offset. A live entry that is re-enqueued
      // with a fresh File must be replaced, not echoed back as a stale item.
      try {
        existing.handle?.abort();
      } catch {}
      if (!paused) {
        for (const tusId of uniqueTusIds(existing.item, existing.tusIds)) {
          deleteTus(UPLOAD_URL, tusId);
        }
      }
      entries.delete(msg.id);
    } else {
      if (existing)
        entryItem(existing.item);
      return;
    }
  }
  if (msg.clientMode)
    setUploadModeHint(msg.clientMode);
  const base = msg.item && typeof msg.item === "object" ? { ...msg.item } : {};
  const options = msg.options || {};
  const item = {
    ...base,
    id: msg.id,
    filename: msg.filename || base.filename || "upload",
    mimeType: msg.mimeType || base.mimeType || "application/octet-stream",
    size: Number(msg.size ?? base.size) || 0,
    lastModified: msg.lastModified ?? base.lastModified,
    clientMode: msg.clientMode || base.clientMode || "",
    method: base.method || "",
    state: "queued",
    progress: Number(base.progress) || 0,
    ttlHours: msg.ttlHours ?? base.ttlHours,
    quickLink: options.quickLink ?? base.quickLink,
    safeUpload: options.safeUpload ?? base.safeUpload ?? false,
    customHost: options.customHost ?? base.customHost,
    uploadMode: options.uploadMode ?? base.uploadMode,
    tusOffset: Number(base.tusOffset) || 0,
    tusParts: base.tusParts && typeof base.tusParts === "object" ? base.tusParts : undefined,
    password: options.password || base.password || "",
    createdAt: base.createdAt || Date.now()
  };
  delete item.pausedAt;
  delete item.errorCode;
  delete item.errorMessage;
  const entry = { item, handle: null, tusIds: uniqueTusIds(base) };
  entries.set(item.id, entry);
  queueItemNow(item);
  entryItem(item);
  try {
    entry.handle = startUpload(item, msg.file, {
      update: upsert,
      onTusCreate: (tusId, info) => {
        if (!entry.tusIds.includes(tusId))
          entry.tusIds.push(tusId);
        const patch = { id: item.id, tusIds: [...entry.tusIds] };
        if (info && info.kind === "stream")
          patch.tusId = tusId;
        upsert(patch);
      },
      onTusDelete: (tusId) => {
        deleteTus(UPLOAD_URL, tusId);
      }
    });
  } catch (err) {
    // A synchronous failure would otherwise leave the item queued with no handle.
    entry.handle = null;
    try {
      console.error("upload worker: startUpload failed", { item: item.id, err });
    } catch {}
    upsert({
      id: item.id,
      state: "error",
      errorCode: E.UPLOAD_FAILED,
      errorMessage: err?.message || (err == null ? "" : String(err)) || "The upload worker could not start the upload",
    });
  }
}
function handleCancel(id) {
  const entry = entries.get(id);
  if (!entry)
    return;
  entry.handle?.abort();
  for (const tusId of uniqueTusIds(entry.item, entry.tusIds)) {
    deleteTus(UPLOAD_URL, tusId);
  }
  void removeQueueItem(id);
  if (entry.item.state === "queued" || entry.item.state === "compressing") {
    upsert({ id, state: "cancelled" });
  }
}
function handleRemove(id) {
  const entry = entries.get(id);
  if (entry) {
    entry.handle?.abort();
    for (const tusId of uniqueTusIds(entry.item, entry.tusIds)) {
      deleteTus(UPLOAD_URL, tusId);
    }
  }
  dismissed.add(id);
  entries.delete(id);
  void removeQueueItem(id);
  emit({ type: "removed", id });
}
ctx.onconnect = (event) => {
  const port = event.ports[0];
  ports.add(port);
  port.onmessage = (e) => {
    const msg = e.data || {};
    if (msg.type === "probe" && msg.nonce) {
      try {
        port.postMessage({
          type: "probe-ok",
          nonce: msg.nonce,
          version: WORKER_VERSION,
          idleMs: Date.now() - lastEmitOkAt,
        });
      } catch {}
    } else if (msg.type === "enqueue") {
      handleEnqueue(msg, port);
    } else if (msg.type === "cancel" && msg.id) {
      handleCancel(msg.id);
    } else if (msg.type === "remove" && msg.id) {
      handleRemove(msg.id);
    } else if (msg.type === "patch" && msg.id && msg.patch) {
      upsert({ id: msg.id, ...msg.patch });
    } else if (msg.type === "status" && Array.isArray(msg.ids)) {
      for (const id of msg.ids) {
        const entry = entries.get(id);
        if (entry && !dismissed.has(id))
          entryItem(entry.item);
      }
    } else if (msg.type === "ping") {
      // Ping/pong is answered on this port directly, so it proves only that the
      // worker thread is alive — not that item/snapshot broadcasts still work.
      try { port.postMessage({ type: "pong", idleMs: Date.now() - lastEmitOkAt }); } catch {}
    } else if (msg.type === "disconnect") {
      // The page is going away; drop its port so broadcast work stays proportional to open tabs.
      ports.delete(port);
      try { port.close(); } catch {}
    }
  };
  port.start();
  port.postMessage({
    type: "snapshot",
    items: [...entries.values()]
      .filter((e) => !dismissed.has(e.item.id) && ACTIVE_STATES.has(e.item.state))
      .map((e) => e.item)
  });
  lastEmitOkAt = Date.now();
  port.addEventListener("close", () => {
    ports.delete(port);
  });
};
