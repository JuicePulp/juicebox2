import {
  startUpload
} from "./upload-engine.js?v=2.1.32";
import { UPLOAD_URL, setUploadModeHint } from "./upload-config.js?v=2.1.32";
import { deleteTus } from "./tus.js";
const ctx = self;
const entries = new Map;
const dismissed = new Set;
const ports = new Set;
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
  let entry = entries.get(patch.id);
  if (!entry) {
    entry = { item: {}, handle: null, tusIds: [] };
    entries.set(patch.id, entry);
  }
  Object.assign(entry.item, patch);
  entryItem(entry.item);
  if (!ACTIVE_STATES.has(entry.item.state)) {
    // Release the file/transfer references. The item itself stays so late
    // events and removals still merge into a complete row, but snapshots only
    // send live work (see onconnect below).
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
  if (entries.has(msg.id))
    return;
  // This worker has no DOM: the page hands us the server-advertised upload
  // mode with every enqueue. Without it readUploadMode() falls back to
  // "standard" and every large upload silently takes the slow TUS path.
  if (msg.clientMode)
    setUploadModeHint(msg.clientMode);
  const item = {
    id: msg.id,
    filename: msg.filename,
    mimeType: msg.mimeType,
    size: msg.size,
    lastModified: msg.lastModified,
    clientMode: msg.clientMode || "",
    method: "",
    state: "queued",
    progress: 0,
    ttlHours: msg.ttlHours,
    quickLink: msg.options.quickLink,
    customHost: msg.options.customHost,
    uploadMode: msg.options.uploadMode,
    password: msg.options.password || "",
    createdAt: Date.now()
  };
  const entry = { item, handle: null, tusIds: [] };
  entries.set(item.id, entry);
  entryItem(item);
  entry.handle = startUpload(item, msg.file, {
    update: upsert,
    onTusCreate: (tusId) => {
      entry.tusIds.push(tusId);
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
  for (const tusId of entry.tusIds) {
    deleteTus(UPLOAD_URL, tusId);
  }
  if (entry.item.state === "queued" || entry.item.state === "compressing") {
    upsert({ id, state: "cancelled" });
  }
}
function handleRemove(id) {
  const entry = entries.get(id);
  if (entry) {
    entry.handle?.abort();
    for (const tusId of entry.tusIds) {
      deleteTus(UPLOAD_URL, tusId);
    }
  }
  dismissed.add(id);
  entries.delete(id);
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
    } else if (msg.type === "ping") {
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
