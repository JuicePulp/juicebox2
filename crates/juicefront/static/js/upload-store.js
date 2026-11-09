import { isTerminal } from "./util.js";

const DB_NAME = "juicebox_upload_queue";
const DB_VERSION = 1;
const STORE_NAME = "uploads";
const MAX_AGE_MS = 24 * 60 * 60 * 1000;
// Progress updates arrive faster than IndexedDB can commit, so writes are debounced.
const WRITE_DELAY_MS = 180;

// Allowlist, not denylist: file handles, passwords, and blobs must never reach disk.
const KEEP_FIELDS = [
  "id",
  "filename",
  "mimeType",
  "size",
  "lastModified",
  "clientMode",
  "method",
  "state",
  "progress",
  "ttlHours",
  "quickLink",
  "safeUpload",
  "customHost",
  "uploadMode",
  "createdAt",
  "requiresPassword",
  "fingerprint",
  "serverId",
  "url",
  "rawUrl",
  "downloadUrl",
  "deleteToken",
  "expiresAt",
  "protected",
  "reserveId",
  "reserveUrl",
  "reserveRawUrl",
  "reserveDownloadUrl",
  "reserveDeleteToken",
  "tusId",
  "tusSessionId",
  "tusOffset",
  "tusParts",
  "numParts",
  "cobaltJobId",
  "delegatedFileId",
  "delegatedDeleteToken",
  "errorCode",
  "errorMessage",
  "pausedAt",
];

let dbPromise = null;
let loadPromise = null;
let flushTimer = null;
let flushing = false;
let flushAgain = false;
const memory = new Map;
const dirty = new Set;

function hasIndexedDb() {
  try {
    return typeof indexedDB !== "undefined" && indexedDB !== null;
  } catch {
    return false;
  }
}

function openDb() {
  if (dbPromise)
    return dbPromise;
  dbPromise = new Promise((resolve, reject) => {
    if (!hasIndexedDb()) {
      reject(new Error("IndexedDB unavailable"));
      return;
    }
    let settled = false;
    let request;
    try {
      request = indexedDB.open(DB_NAME, DB_VERSION);
    } catch (err) {
      reject(err);
      return;
    }
    // `onblocked` can hang forever in some browsers, so cap the open.
    const timer = setTimeout(() => {
      if (settled)
        return;
      settled = true;
      reject(new Error("queue store open timed out"));
    }, 1500);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(STORE_NAME))
        db.createObjectStore(STORE_NAME, { keyPath: "id" });
    };
    request.onsuccess = () => {
      if (settled) {
        try { request.result.close(); } catch {}
        return;
      }
      settled = true;
      clearTimeout(timer);
      resolve(request.result);
    };
    request.onerror = () => {
      if (settled)
        return;
      settled = true;
      clearTimeout(timer);
      reject(request.error || new Error("queue store open failed"));
    };
    request.onblocked = () => {
      if (settled)
        return;
      settled = true;
      clearTimeout(timer);
      reject(new Error("queue store blocked"));
    };
  });
  return dbPromise;
}

function sanitize(item) {
  if (!item || !item.id)
    return null;
  const out = {};
  for (const key of KEEP_FIELDS) {
    const value = item[key];
    if (value !== undefined && value !== null)
      out[key] = value;
  }
  if (Array.isArray(item.tusIds) && item.tusIds.length) {
    out.tusIds = [...new Set(item.tusIds.filter((id) => typeof id === "string" && id))];
  }
  if (item.tusParts && typeof item.tusParts === "object" && !Array.isArray(item.tusParts)) {
    const parts = {};
    for (const [key, value] of Object.entries(item.tusParts)) {
      if (!/^\d+$/.test(key) || !value || typeof value !== "object")
        continue;
      const id = typeof value.id === "string" ? value.id : "";
      if (!id)
        continue;
      const offset = Number(value.offset);
      const length = Number(value.length);
      parts[key] = {
        id,
        offset: Number.isFinite(offset) ? Math.max(0, Math.trunc(offset)) : 0,
        length: Number.isFinite(length) ? Math.max(0, Math.trunc(length)) : 0,
        ...(value.done ? { done: true } : {})
      };
    }
    if (Object.keys(parts).length)
      out.tusParts = parts;
    else
      delete out.tusParts;
  }
  if (Number.isFinite(out.tusOffset)) {
    out.tusOffset = Math.max(0, Math.trunc(out.tusOffset));
  }
  if (Number.isFinite(out.size)) {
    out.size = Math.max(0, Math.trunc(out.size));
  }
  if (Number.isFinite(out.progress)) {
    out.progress = Math.max(0, Math.min(100, out.progress));
  }
  if (Number.isFinite(out.numParts)) {
    out.numParts = Math.max(0, Math.trunc(out.numParts));
  }
  out.persistedAt = Date.now();
  return out;
}


function scheduleFlush(delayMs = WRITE_DELAY_MS) {
  if (flushTimer)
    return;
  flushTimer = setTimeout(() => {
    flushTimer = null;
    void flushQueue();
  }, delayMs);
}

// Best effort: quota and private-mode failures are swallowed; the queue still works in memory.
async function writeRecords(ids) {
  let db;
  try {
    db = await openDb();
  } catch {
    return;
  }
  await new Promise((resolve) => {
    let tx;
    try {
      tx = db.transaction(STORE_NAME, "readwrite");
    } catch {
      resolve();
      return;
    }
    const store = tx.objectStore(STORE_NAME);
    for (const id of ids) {
      if (memory.has(id)) {
        try { store.put(memory.get(id)); } catch {}
      } else {
        try { store.delete(id); } catch {}
      }
    }
    tx.oncomplete = () => resolve();
    tx.onerror = () => resolve();
    tx.onabort = () => resolve();
  });
}

export function queueItem(item) {
  const record = sanitize(item);
  if (!record)
    return;
  if (record.state === "error" || record.state === "cancelled") {
    void removeQueueItem(record.id);
    return;
  }
  memory.set(record.id, record);
  dirty.add(record.id);
  scheduleFlush();
}

export function queueItemNow(item) {
  const record = sanitize(item);
  if (!record)
    return;
  if (record.state === "error" || record.state === "cancelled") {
    void removeQueueItem(record.id);
    return;
  }
  memory.set(record.id, record);
  dirty.add(record.id);
  if (flushTimer) {
    clearTimeout(flushTimer);
    flushTimer = null;
  }
  scheduleFlush(0);
}

export async function removeQueueItem(id) {
  if (!id)
    return;
  memory.delete(id);
  dirty.delete(id);
  if (!hasIndexedDb())
    return;
  let db;
  try {
    db = await openDb();
  } catch {
    return;
  }
  await new Promise((resolve) => {
    let tx;
    try {
      tx = db.transaction(STORE_NAME, "readwrite");
    } catch {
      resolve();
      return;
    }
    try {
      tx.objectStore(STORE_NAME).delete(id);
    } catch {}
    tx.oncomplete = () => resolve();
    tx.onerror = () => resolve();
    tx.onabort = () => resolve();
  });
}

export async function flushQueue() {
  if (flushing) {
    flushAgain = true;
    return;
  }
  if (dirty.size === 0)
    return;
  flushing = true;
  const ids = [...dirty];
  dirty.clear();
  // `dirty` holds ids, not snapshots, so the written value is always the latest in `memory`.
  await writeRecords(ids);
  flushing = false;
  if (flushAgain || dirty.size > 0) {
    flushAgain = false;
    scheduleFlush(0);
  }
}

export async function loadQueueItems() {
  if (loadPromise)
    return loadPromise;
  loadPromise = (async () => {
    const now = Date.now();
    const rows = [];
    const stale = [];
    if (hasIndexedDb()) {
      try {
        const db = await openDb();
        await new Promise((resolve) => {
          let tx;
          try {
            tx = db.transaction(STORE_NAME, "readonly");
          } catch {
            resolve();
            return;
          }
          const request = tx.objectStore(STORE_NAME).getAll();
          request.onsuccess = () => {
            const all = Array.isArray(request.result) ? request.result : [];
            for (const row of all) {
              if (!row || !row.id) {
                continue;
              }
              if (row.persistedAt && now - row.persistedAt > MAX_AGE_MS) {
                stale.push(row.id);
                continue;
              }
              if (isTerminal(row.state) && row.state !== "done") {
                stale.push(row.id);
                continue;
              }
              memory.set(row.id, row);
              rows.push(row);
            }
            resolve();
          };
          request.onerror = () => resolve();
          tx.onerror = () => resolve();
          tx.onabort = () => resolve();
        });
      } catch {
      }
    }
    for (const row of memory.values()) {
      if (!rows.some((entry) => entry.id === row.id))
        rows.push(row);
    }
    if (stale.length) {
      for (const id of stale)
        void removeQueueItem(id);
    }
    return rows.sort((a, b) => (Number(a.createdAt) || 0) - (Number(b.createdAt) || 0));
  })();
  return loadPromise;
}
