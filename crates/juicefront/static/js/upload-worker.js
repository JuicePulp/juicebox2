import {
  startUpload
} from "./upload-engine.js?v=2.2.0";
import { UPLOAD_URL, setUploadModeHint } from "./upload-config.js?v=2.2.0";
import { deleteTus, uniqueTusIds } from "./tus.js";
import {
  queueItem,
  queueItemNow,
  removeQueueItem
} from "./upload-store.js?v=2.2.0";
const ctx = self;
const entries = new Map;
// A removed id must stay gone even if a late engine callback still reports it.
const dismissed = new Set;
const ports = new Set;
// Bounds worker memory; finished entries are evicted oldest-first once this is exceeded.
const MAX_ITEMS = 100;
const ACTIVE_STATES = new Set(["queued", "compressing", "uploading", "finalizing"]);
function emit(message) {
  for (const port of ports) {
    try {
      port.postMessage(message);
    } catch {}
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
        errorCode: "",
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
    if (existing && existing.item.state === "paused") {
      // A paused item has a stale handle after reload; replace it to resume from the saved offset.
      try {
        existing.handle?.abort();
      } catch {}
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
    if (msg.type === "enqueue") {
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
      // Ping/pong is the client's liveness check; a missed pong triggers a main-thread fallback.
      try { port.postMessage({ type: "pong" }); } catch {}
    }
  };
  port.start();
  port.postMessage({
    type: "snapshot",
    items: [...entries.values()]
      .filter((e) => !dismissed.has(e.item.id) && ACTIVE_STATES.has(e.item.state))
      .map((e) => e.item)
  });
  port.addEventListener("close", () => {
    ports.delete(port);
  });
};
