import {
  UPLOAD_URL,
  TUS_THRESHOLD,
  TUS_MIN_CHUNK,
  TUS_TARGET_CHUNK_SECS,
  PARALLEL_STREAMS,
  TUS_MIN_PART,
  TUS_MAX_PARTS,
  PIPELINE_WINDOW,
  TUNER_INTERVAL_MS,
  INITIAL_WORKERS,
  MAX_PART_RUNS,
  LAST_SPEED_KEY,
  lastUploadBps,
  streamsForRate,
  detectNetTier,
  TIER_DEFAULT_BPS,
  TIER_CHUNK,
  gzipEligible,
  isKnownCompressed,
  RESERVE_URL,
  DIRECT_RESERVE_URL,
  DIRECT_COMPLETE_URL,
  DIRECT_PARALLEL_RESERVE_URL,
  DIRECT_PARALLEL_COMPLETE_URL,
  DIRECT_PARALLEL_THRESHOLD,
  STREAM_START_CHUNK,
  STREAM_MIN_CHUNK,
  STREAM_MAX_CHUNK,
  STREAM_TARGET_SECS,
  DIRECT_PART_TARGET,
  DIRECT_MAX_PART_BYTES,
  DIRECT_MAX_PARTS,
  DIRECT_COMPLETE_TIMEOUT_MS,
  readUploadMode,
  uploadMethodFor,
  preferStream
} from "./upload-config.js?v=2.2.0-r2";
import { tryAcquireStream, releaseStream } from "./stream-limiter.js?v=2.2.0-r2";
import { tweakEngineEnabled } from "./upload-tweaks.js?v=2.2.0-r2";
import { announce } from "./util.js?v=2.2.0-r2";
import { publicFileInfo } from "./api.js?v=2.2.0-r2";
import { compressFile } from "./compress.js?v=2.2.0-r2";
import { encodeTusMeta } from "./tus.js?v=2.2.0-r2";
export function extractServerId(url) {
  const m = url.match(/\/f\/([A-Za-z0-9_-]+)/);
  return m ? m[1] : "";
}
export function generateId() {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return Math.random().toString(36).slice(2) + Date.now().toString(36);
}
// 520-524 are Cloudflare origin failures and behave like 502-504 for retries.
function isTransientStatus(status) {
  return status === 0 || status >= 520 && status <= 524 || status === 502 || status === 503 || status === 504;
}
/** `Retry-After` (or legacy `x-ratelimit-after`) in seconds, clamped to 60 s; `fallbackMs` when absent or unparsable. */
function retryAfterMs(res, fallbackMs) {
  try {
    const raw = res.headers.get("Retry-After") || res.headers.get("x-ratelimit-after");
    if (raw) {
      const secs = Number(String(raw).split(",")[0].trim());
      if (Number.isFinite(secs) && secs >= 0)
        return Math.min(secs * 1000, 60000);
    }
  } catch {}
  return fallbackMs;
}
/** Retries with exponential backoff and jitter, honouring `Retry-After` on 429; non-transient responses are returned so callers inspect status. */
async function fetchWithRetry(input, init, maxRetries = 3, baseDelayMs = 1000, timeoutMs) {
  let lastErr;
  for (let attempt = 0;attempt <= maxRetries; attempt++) {
    try {
      const res = await fetchWithTimeout(input, init, timeoutMs);
      if (res.ok || attempt === maxRetries) {
        return res;
      }
      if (res.status === 429) {
        lastErr = new Error(`HTTP 429`);
        await new Promise((r) => setTimeout(r, retryAfterMs(res, baseDelayMs * Math.pow(2, attempt))));
        continue;
      }
      if (!isTransientStatus(res.status)) {
        return res;
      }
      lastErr = new Error(`HTTP ${res.status}`);
    } catch (err) {
      lastErr = err;
      if (attempt === maxRetries)
        throw err;
    }
    const delay = baseDelayMs * Math.pow(2, attempt) + Math.random() * 500;
    await new Promise((r) => setTimeout(r, delay));
  }
  throw lastErr;
}
const FETCH_TIMEOUT_MS = 30000;
/** `fetch` bounded by `ms` and linked to the caller's signal; caller abort wins. */
function fetchWithTimeout(input, init, ms) {
  const ctrl = new AbortController;
  const userSignal = init ? init.signal : undefined;
  const timer = setTimeout(() => {
    try {
      ctrl.abort();
    } catch {}
  }, ms === undefined ? FETCH_TIMEOUT_MS : ms);
  const onAbort = () => {
    clearTimeout(timer);
    try {
      ctrl.abort();
    } catch {}
  };
  if (userSignal) {
    if (userSignal.aborted)
      onAbort();
    else
      userSignal.addEventListener("abort", onAbort, { once: true });
  }
  return fetch(input, { ...init, signal: ctrl.signal }).finally(() => {
    clearTimeout(timer);
    if (userSignal)
      userSignal.removeEventListener("abort", onAbort);
  });
}
// Safe mode polls TUS health on this cadence to notice session loss before the next PATCH.
const SAFE_HEALTH_MS = 15000;
/** `{ lost: true }` means the session is gone for good (404); `null` means unknown and must not be read as lost. */
async function tusHealth(tusId, signal) {
  if (!tusId)
    return null;
  try {
    const res = await fetchWithTimeout(
      `${UPLOAD_URL}/api/tus/${encodeURIComponent(tusId)}/health`,
      {
        method: "GET",
        headers: { "Tus-Resumable": "1.0.0" },
        signal
      },
      12000
    );
    if (res.status === 404)
      return { lost: true };
    if (!res.ok)
      return null;
    const data = await res.json();
    return data && typeof data === "object" ? data : null;
  } catch {
    return null;
  }
}
function healthOffset(health) {
  const value = Number(health?.offset);
  return Number.isFinite(value) ? Math.max(0, value) : null;
}
function healthStorageReady(health) {
  const storage = health?.storage;
  return !!storage && storage.found !== false && storage.ready === true;
}
async function directPartHealth(uploadUrl, id, expectedSize) {
  if (!uploadUrl || !id)
    return null;
  try {
    const base = new URL(uploadUrl, typeof location !== "undefined" ? location.href : "http://localhost");
    const res = await fetchWithTimeout(
      `${base.origin}/api/upload-progress/${encodeURIComponent(id)}`,
      {},
      10000
    );
    if (!res.ok)
      return null;
    const data = await res.json();
    if (!data || data.ready !== true)
      return null;
    if (Number.isFinite(expectedSize) && Number(data.total) !== expectedSize)
      return null;
    return data;
  } catch {
    return null;
  }
}

/** Normalises "already exists": a 409 carries the resume location, and a 400 mentioning it is retried once. */
async function createTusSession(input, init) {
  const readText = async (r) => r.text().catch(() => "");
  const echo = (r, text) => new Response(text, { status: r.status, headers: r.headers });
  const parseConflict = (res) => {
    const location = res.headers.get("Location");
    if (location) {
      const offset = Number(res.headers.get("Upload-Offset"));
      return {
        res,
        alreadyExists: false,
        completing: false,
        existing: {
          location,
          offset: Number.isFinite(offset) ? Math.max(0, offset) : 0
        }
      };
    }
    if (res.headers.get("X-Juicebox-Part-Completing") === "1") {
      return { res, alreadyExists: false, completing: true, existing: null };
    }
    return { res, alreadyExists: false, completing: false, existing: null };
  };
  let res = await fetchWithTimeout(input, init);
  if (res.status === 409)
    return parseConflict(res);
  if (res.status === 400) {
    const text = await readText(res);
    if (!/already exists/i.test(text)) {
      return { res: echo(res, text), alreadyExists: false, completing: false, existing: null };
    }
    await new Promise((r) => setTimeout(r, 500));
    res = await fetchWithTimeout(input, init);
    if (res.status === 409)
      return parseConflict(res);
    if (res.status === 400) {
      const text2 = await readText(res);
      return {
        res: echo(res, text2),
        alreadyExists: /already exists/i.test(text2),
        completing: false,
        existing: null
      };
    }
  }
  return { res, alreadyExists: false, completing: false, existing: null };
}
/** Polls until the file is ready; `null` on timeout (60 s), abort, or 410 (reservation dropped). */
async function waitForReservedFile(id, signal, timeoutMs = 60000) {
  const deadline = performance.now() + timeoutMs;
  while (!signal.aborted && performance.now() < deadline) {
    try {
      const res = await fetchWithTimeout(publicFileInfo(id), { signal });
      if (res.ok) {
        const info = await res.json();
        if (info?.status === "ready" && info.url)
          return info;
      } else if (res.status === 410) {
        return null;
      }
    } catch {
      if (signal.aborted)
        return null;
    }
    await new Promise((r) => setTimeout(r, 2000));
  }
  return null;
}

/** Entry point for one upload; resume state on `item` wins over a fresh transport choice. Returns `{ abort }`. */
export function startUpload(item, file, controls) {
  if (item.method === "direct-stream" && item.tusId)
    return startTusStreamUpload(item, file, controls);
  if (item.method === "tus" && item.tusSessionId)
    return startTusUpload(item, file, controls);
  if (item.password) {
    try {
      announce("Protected upload: using secure relay");
    } catch {}
    updateMethod(controls, item, "direct");
    return startDirectUpload(item, file, controls);
  }
  const mode = item.clientMode || readUploadMode();
  if (mode === "direct-prefer") {
    if (preferStream(mode, file.size, lastUploadBps())) {
      updateMethod(controls, item, "direct-stream");
      return startTusStreamUpload(item, file, controls);
    }
    if (file.size >= DIRECT_PARALLEL_THRESHOLD) {
      updateMethod(controls, item, "direct-parallel");
      return startDirectParallelUpload(item, file, controls);
    }
    updateMethod(controls, item, "direct");
    return startDirectTicketUpload(item, file, controls);
  }
  const label = uploadMethodFor(mode, file.size, 0);
  updateMethod(controls, item, label);
  return file.size > TUS_THRESHOLD ? startTusUpload(item, file, controls) : startDirectUpload(item, file, controls);
}

function updateMethod(controls, item, method) {
  try {
    controls?.update?.({ id: item.id, method });
  } catch {}
}
// rAF is absent in workers and tests; fall back so LiveProgress stays usable there.
const _raf = typeof requestAnimationFrame !== "undefined" ? requestAnimationFrame : (cb) => setTimeout(cb, 16);
const _caf = typeof cancelAnimationFrame !== "undefined" ? cancelAnimationFrame : clearTimeout;

class LiveProgress {
  loaded = 0;
  total = 0;
  shown = 0;
  lastTime = 0;
  speed = 0;
  raf = null;
  callback;
  maxProjectAhead;
  constructor(callback, options = {}) {
    this.callback = callback;
    this.maxProjectAhead = Number.isFinite(options.maxProjectAhead) ? Math.max(0, options.maxProjectAhead) : Infinity;
  }
  begin(totalBytes) {
    this.total = totalBytes;
    this.loaded = 0;
    this.shown = 0;
    this.speed = 0;
    this.lastTime = performance.now();
    this.emit(this.loaded);
    if (this.raf === null)
      this.startLoop();
  }
  setTotal(bytes) {
    if (bytes > 0 && Number.isFinite(bytes))
      this.total = bytes;
  }
  set(bytes) {
    const now = performance.now();
    const dt = (now - this.lastTime) / 1000;
    if (dt >= 0.15 && bytes > this.loaded) {
      const inst = (bytes - this.loaded) / dt;
      const clamped = Math.min(inst, Math.max(this.speed * 3, 64 * 1024 * 1024));
      this.speed = this.speed ? this.speed * 0.7 + clamped * 0.3 : clamped;
    }
    if (bytes > this.loaded) {
      this.loaded = bytes;
      this.lastTime = now;
    }
    this.emit(this.loaded);
  }
  emit(bytes) {
    if (this.total <= 0)
      return;
    const pct = Math.min(bytes / this.total * 100, 100);
    if (!Number.isFinite(pct) || pct <= this.shown)
      return;
    this.shown = pct;
    this.callback(pct);
  }
  startLoop() {
    const tick = () => {
      const dt = (performance.now() - this.lastTime) / 1000;
      if (this.speed > 0 && this.total > 0) {
        const projected = Math.min(
          this.loaded + this.speed * dt,
          this.loaded + this.speed * 2,
          this.loaded + this.maxProjectAhead,
          this.total
        );
        this.emit(projected);
      }
      this.raf = _raf(tick);
    };
    this.raf = _raf(tick);
  }
  destroy() {
    if (this.raf !== null) {
      _caf(this.raf);
      this.raf = null;
    }
  }
}
async function reserveQuickLink(item, signal) {
  const reserveRes = await fetchWithTimeout(RESERVE_URL, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      filename: item.filename,
      mime_type: item.mimeType || "application/octet-stream",
      file_size: Number(item.size) || 0,
      ttl_hours: Number(item.ttlHours),
      host: item.customHost || undefined,
      password: item.password || undefined
    }),
    ...signal ? { signal } : {}
  });
  if (!reserveRes.ok)
    return null;
  const reserve = await reserveRes.json();
  return {
    id: reserve.id || "",
    url: reserve.url || "",
    rawUrl: reserve.raw_url || "",
    downloadUrl: reserve.download_url || "",
    deleteToken: reserve.delete_token || ""
  };
}
function startDirectUpload(item, file, controls) {
  const { update } = controls;
  const runState = { cancelled: false };
  const waitCtrl = new AbortController;
  let xhr = null;
  const run = (async () => {
    let reserveId = item.reserveId || "";
    let reserveUrl = item.reserveUrl || "";
    let reserveRawUrl = item.reserveRawUrl || "";
    let reserveDownloadUrl = item.reserveDownloadUrl || "";
    let reserveDeleteToken = item.reserveDeleteToken || "";
    if (item.quickLink && !reserveId) {
      try {
        const reserve = await reserveQuickLink(item);
        if (reserve) {
          reserveId = reserve.id;
          reserveUrl = reserve.url;
          reserveRawUrl = reserve.rawUrl;
          reserveDownloadUrl = reserve.downloadUrl;
          reserveDeleteToken = reserve.deleteToken;
          if (reserveUrl)
            update({
              id: item.id,
              reserveId,
              reserveUrl,
              reserveRawUrl,
              reserveDownloadUrl,
              reserveDeleteToken,
              serverId: reserveId
            });
        }
      } catch {}
      if (runState.cancelled)
        throw new Error("Upload cancelled");
    }
    update({ id: item.id, state: "compressing" });
    const { file: uploadFile, isGzip } = await compressFile(file);
    if (runState.cancelled)
      throw new Error("Upload cancelled");
    update({ id: item.id, state: "uploading", progress: 0 });
    const fd = new FormData;
    fd.append("ttl_hours", String(item.ttlHours));
    if (item.customHost)
      fd.append("host", item.customHost);
    fd.append("upload_mode", item.uploadMode);
    if (reserveId)
      fd.append("reserve_id", reserveId);
    if (item.password)
      fd.append("password", item.password);
    fd.append("file", uploadFile);
    const safe = !!item.safeUpload;
    const progress = new LiveProgress((pct) => {
      update({ id: item.id, progress: Math.min(pct, 99.4) });
    });
    progress.begin(uploadFile.size);
    const sendOnce = () => new Promise((resolve) => {
      xhr = new XMLHttpRequest;
      xhr.open("POST", `${UPLOAD_URL}/upload`, true);
      if (isGzip)
        xhr.setRequestHeader("X-File-Encoding", "gzip");
      if (reserveId && reserveDeleteToken) {
        xhr.setRequestHeader("X-Delete-Token", reserveDeleteToken);
      }
      xhr.timeout = 5 * 60 * 1000;
      xhr.upload.onprogress = (ev) => {
        if (!ev.lengthComputable || !ev.total || ev.total <= 0)
          return;
        progress.set(ev.loaded);
      };
      xhr.onload = () => {
        resolve({ status: xhr.status, body: xhr.responseText || "" });
      };
      xhr.onerror = () => {
        resolve({ status: 0, network: true });
      };
      xhr.ontimeout = () => {
        resolve({ status: 0, timeout: true });
      };
      xhr.onabort = () => {
        resolve({ status: 0, aborted: true });
      };
      xhr.send(fd);
    });
    const maxAttempts = safe ? 3 : 1;
    let result = null;
    for (let attempt = 0; attempt < maxAttempts; attempt++) {
      if (runState.cancelled)
        break;
      if (attempt > 0) {
        update({ id: item.id, state: "uploading", progress: 0 });
        progress.begin(uploadFile.size);
        await new Promise((r) => setTimeout(r, 1000 * 2 ** (attempt - 1)));
        if (runState.cancelled)
          break;
        if (reserveId) {
          const info = await waitForReservedFile(reserveId, waitCtrl.signal, 12000);
          if (info) {
            result = { status: 200, body: JSON.stringify(info) };
            break;
          }
        }
      }
      result = await sendOnce();
      if (result.aborted || runState.cancelled)
        break;
      if (result.status >= 200 && result.status < 300)
        break;
      const retryable = result.status === 0 || result.status === 408 || result.status >= 500;
      if (!safe || !retryable || attempt === maxAttempts - 1)
        break;
    }
    progress.destroy();
    if (runState.cancelled || result?.aborted) {
      update({ id: item.id, state: "cancelled" });
      return;
    }
    if (safe && result?.status === 409 && reserveId) {
      // A 409 after a retry usually means the earlier attempt landed; let the reservation decide.
      const info = await waitForReservedFile(reserveId, waitCtrl.signal, 12000);
      if (info)
        result = { status: 200, body: JSON.stringify(info) };
    }
    if (result && result.status >= 200 && result.status < 300) {
      let res = {};
      try {
        res = JSON.parse(result.body);
      } catch {}
      if (reserveUrl && !res.url)
        res.url = reserveUrl;
      if (reserveRawUrl && !res.raw_url)
        res.raw_url = reserveRawUrl;
      if (reserveDownloadUrl && !res.download_url)
        res.download_url = reserveDownloadUrl;
      if (reserveDeleteToken && !res.delete_token)
        res.delete_token = reserveDeleteToken;
      update({
        id: item.id,
        serverId: res.id || reserveId || "",
        url: res.url || "",
        rawUrl: res.raw_url || "",
        downloadUrl: res.download_url || "",
        deleteToken: res.delete_token || "",
        expiresAt: res.expires_at ? Number(res.expires_at) : Math.round(Date.now() / 1000) + item.ttlHours * 3600,
        protected: !!res.protected,
        state: "done",
        progress: 100
      });
      return;
    }
    let code;
    let msg;
    try {
      const err = JSON.parse(result?.body || "");
      if (err.error)
        code = err.error;
      if (err.message)
        msg = err.message;
    } catch {}
    if (result?.timeout) {
      code = code || "UPLOAD_TIMEOUT";
      msg = msg || "Upload timed out. Try a smaller file.";
    } else if (!result || result.status === 0) {
      code = code || "NETWORK_ERROR";
    } else if (result.status >= 520 && result.status <= 524) {
      code = code || "UPLOAD_TIMEOUT";
      msg = msg || "Connection timed out (Cloudflare). Try a smaller file or try again later.";
    }
    update({ id: item.id, errorCode: code, errorMessage: msg, state: "error" });
  })().catch((err) => {
    update({
      id: item.id,
      state: runState.cancelled ? "cancelled" : "error",
      errorCode: runState.cancelled ? undefined : "NETWORK_ERROR",
      errorMessage: err?.message
    });
  });
  return {
    abort: () => {
      runState.cancelled = true;
      waitCtrl.abort();
      xhr?.abort();
    }
  };
}
async function gzipBlob(blob) {
  return new Response(blob.stream().pipeThrough(new CompressionStream("gzip"))).blob();
}
/** Raw octet-stream POST with a one-shot ticket; an idle watchdog aborts after 30 s without progress. */
function xhrSendBytes(uploadUrl, blob, ticket, onBytes, runState, xhrRef) {
  return new Promise((resolve, reject) => {
    const request = new XMLHttpRequest;
    xhrRef.current = request;
    request.open("POST", uploadUrl, true);
    request.setRequestHeader("Authorization", `Bearer ${ticket}`);
    request.setRequestHeader("Content-Type", "application/octet-stream");
    let idleTimer = null;
    const armIdle = () => {
      clearTimeout(idleTimer);
      idleTimer = setTimeout(() => {
        try {
          request.abort();
        } catch {}
      }, 30000);
    };
    const clearIdle = () => clearTimeout(idleTimer);
    request.upload.onprogress = (ev) => {
      armIdle();
      if (!ev.lengthComputable || !ev.total || ev.total <= 0)
        return;
      if (Number.isFinite(ev.loaded))
        onBytes(ev.loaded);
    };
    request.onload = () => {
      clearIdle();
      resolve({
        status: request.status,
        ok: request.status >= 200 && request.status < 300,
        body: request.responseText
      });
    };
    request.onerror = () => {
      clearIdle();
      reject(new Error("NETWORK_ERROR"));
    };
    request.onabort = () => {
      clearIdle();
      if (runState.cancelled)
        reject(new Error("Upload cancelled"));
      else
        reject(new Error("NETWORK_ERROR"));
    };
    armIdle();
    request.send(blob);
  });
}
/** Reserve, upload with a short-lived ticket, then complete; a 401/403 retries once with a fresh reservation. Returns `{ abort }`. */
function startDirectTicketUpload(item, file, controls) {
  const { update } = controls;
  const runState = { cancelled: false };
  const safe = !!item.safeUpload;
  const xhrRef = { current: null };
  const run = (async () => {
    const attempt = async (isRetry) => {
      const reserveRes = await fetchWithTimeout(DIRECT_RESERVE_URL, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          filename: item.filename,
          mime_type: item.mimeType || "application/octet-stream",
          file_size: file.size,
          ttl_hours: Number(item.ttlHours),
          host: item.customHost || undefined
        })
      });
      if (runState.cancelled)
        throw new Error("Upload cancelled");
      if (!reserveRes.ok) {
        let code;
        let msg;
        try {
          const err = await reserveRes.json();
          if (err.error)
            code = err.error;
          if (err.message)
            msg = err.message;
        } catch {}
        update({ id: item.id, errorCode: code, errorMessage: msg, state: "error" });
        return;
      }
      const reserve = await reserveRes.json();
      const ticket = reserve.ticket || "";
      const uploadUrl = reserve.upload_url || "";
      const fileId = reserve.file_id || "";
      const shareUrl = reserve.url || "";
      const shareRawUrl = reserve.raw_url || "";
      const shareDownloadUrl = reserve.download_url || "";
      if (!ticket || !uploadUrl || !fileId) {
        update({ id: item.id, errorCode: "RESERVE_FAILED", state: "error" });
        return;
      }
      if (shareUrl)
        update({ id: item.id, reserveUrl: shareUrl, reserveRawUrl: shareRawUrl, reserveDownloadUrl: shareDownloadUrl, serverId: fileId });
      update({ id: item.id, state: "uploading", progress: 0 });
      const progress = new LiveProgress((pct) => {
        update({ id: item.id, progress: Math.min(pct, 99.4) });
      });
      progress.begin(file.size);
      let uploadStatus = null;
      const maxUploadTries = safe ? 3 : 1;
      for (let uploadTry = 0; uploadTry < maxUploadTries; uploadTry++) {
        try {
          uploadStatus = await xhrSendBytes(uploadUrl, file, ticket, (loaded) => progress.set(loaded), runState, xhrRef);
          if (!safe || uploadStatus.ok)
            break;
          const retryableStatus = uploadStatus.status === 408 || uploadStatus.status >= 500;
          if (!retryableStatus || uploadTry === maxUploadTries - 1)
            break;
          const health = await directPartHealth(uploadUrl, fileId, file.size);
          if (health) {
            uploadStatus = { status: 200, ok: true, body: JSON.stringify(health) };
            break;
          }
          progress.begin(file.size);
          await new Promise((r) => setTimeout(r, 1000 * 2 ** uploadTry));
        } catch (err) {
          if (runState.cancelled)
            throw err;
          if (uploadTry === maxUploadTries - 1) {
            progress.destroy();
            update({ id: item.id, errorCode: "NETWORK_ERROR", state: "error" });
            return;
          }
          const health = await directPartHealth(uploadUrl, fileId, file.size);
          if (health) {
            uploadStatus = { status: 200, ok: true, body: JSON.stringify(health) };
            break;
          }
          progress.begin(file.size);
          await new Promise((r) => setTimeout(r, 1000 * 2 ** uploadTry));
        }
      }
      progress.destroy();
      if (runState.cancelled)
        throw new Error("Upload cancelled");
      if (safe && uploadStatus?.status === 409)
        uploadStatus = { ...uploadStatus, ok: true };
      if ((uploadStatus.status === 401 || uploadStatus.status === 403) && !isRetry) {
        attempt(true);
        return;
      }
      if (!uploadStatus.ok) {
        let code;
        let msg;
        try {
          const err = JSON.parse(uploadStatus.body);
          if (err.error)
            code = err.error;
          if (err.message)
            msg = err.message;
        } catch {}
        update({ id: item.id, errorCode: code, errorMessage: msg, state: "error" });
        return;
      }
      update({ id: item.id, state: "finalizing" });
      const completeRes = await fetchWithTimeout(DIRECT_COMPLETE_URL, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ file_id: fileId, ticket })
      });
      if (runState.cancelled)
        throw new Error("Upload cancelled");
      if ((completeRes.status === 401 || completeRes.status === 403) && !isRetry) {
        attempt(true);
        return;
      }
      if (safe && completeRes.status === 409) {
        const info = await publicFileInfo(fileId);
        if (info.ok) {
          const data = await info.json();
          update({
            id: item.id,
            serverId: data.id || fileId,
            url: data.url || shareUrl,
            rawUrl: data.raw_url || shareRawUrl,
            downloadUrl: data.download_url || shareDownloadUrl,
            deleteToken: data.delete_token || "",
            expiresAt: data.expires_at ? Number(data.expires_at) : Math.round(Date.now() / 1000) + item.ttlHours * 3600,
            protected: !!data.protected,
            state: "done",
            progress: 100
          });
          return;
        }
      }
      if (!completeRes.ok) {
        let code;
        let msg;
        try {
          const err = await completeRes.json();
          if (err.error)
            code = err.error;
          if (err.message)
            msg = err.message;
        } catch {}
        update({ id: item.id, errorCode: code, errorMessage: msg, state: "error" });
        return;
      }
      const data = await completeRes.json();
      update({
        id: item.id,
        serverId: data.id || fileId,
        url: data.url || shareUrl,
        rawUrl: data.raw_url || shareRawUrl,
        downloadUrl: data.download_url || shareDownloadUrl,
        deleteToken: data.delete_token || "",
        expiresAt: data.expires_at ? Number(data.expires_at) : Math.round(Date.now() / 1000) + item.ttlHours * 3600,
        protected: !!data.protected,
        state: "done",
        progress: 100
      });
    };
    await attempt(false);
  })().catch((err) => {
    update({
      id: item.id,
      state: runState.cancelled ? "cancelled" : "error",
      errorCode: runState.cancelled ? undefined : "NETWORK_ERROR",
      errorMessage: err?.message
    });
  });
  return {
    abort: () => {
      runState.cancelled = true;
      xhrRef.current?.abort();
    }
  };
}
/** Splits the file into server-assigned parts sent by a growing and shrinking pool; a 413/422 falls back to TUS. Returns `{ abort }`. */
function startDirectParallelUpload(item, file, controls) {
  const { update } = controls;
  const runState = { cancelled: false };
  const safe = !!item.safeUpload;
  const liveXhrs = new Set;
  let fallback = null;
  const run = (async () => {
    const partTarget = Math.min(DIRECT_PART_TARGET, DIRECT_MAX_PART_BYTES);
    const numParts = Math.max(2, Math.min(DIRECT_MAX_PARTS, Math.ceil(file.size / partTarget)));
    const reserveRes = await fetchWithTimeout(DIRECT_PARALLEL_RESERVE_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        filename: item.filename,
        mime_type: item.mimeType || "application/octet-stream",
        file_size: file.size,
        ttl_hours: Number(item.ttlHours),
        num_parts: numParts,
        host: item.customHost || undefined
      })
    });
    if (runState.cancelled)
      throw new Error("Upload cancelled");
    if (!reserveRes.ok) {
      if (reserveRes.status === 413 || reserveRes.status === 422) {
        try {
          announce("Too many parts to split - switching to resumable upload");
        } catch {}
        fallback = startTusUpload(item, file, controls);
        return;
      }
      let code;
      let msg;
      try {
        const err = await reserveRes.json();
        if (err.error)
          code = err.error;
        if (err.message)
          msg = err.message;
      } catch {}
      update({ id: item.id, errorCode: code, errorMessage: msg, state: "error" });
      return;
    }
    const reserve = await reserveRes.json();
    const ticket = reserve.ticket || "";
    const fileId = reserve.file_id || "";
    const shareUrl = reserve.url || "";
    const shareRawUrl = reserve.raw_url || "";
    const shareDownloadUrl = reserve.download_url || "";
    const parts = Array.isArray(reserve.parts) ? reserve.parts : [];
    const step = parts.length ? Math.ceil(file.size / parts.length) : 0;
    let splitOk = ticket && fileId && parts.length >= 2;
    let splitSum = 0;
    for (let i = 0; i < parts.length && splitOk; i++) {
      const p = parts[i];
      const want = Math.min(step, file.size - i * step);
      if (!p || p.part_size !== want || !p.part_id || !p.ticket || !p.upload_url)
        splitOk = false;
      else
        splitSum += p.part_size;
    }
    if (!splitOk || splitSum !== file.size) {
      update({ id: item.id, errorCode: "RESERVE_FAILED", state: "error" });
      return;
    }
    if (shareUrl)
      update({ id: item.id, reserveUrl: shareUrl, reserveRawUrl: shareRawUrl, reserveDownloadUrl: shareDownloadUrl, serverId: fileId });
    update({ id: item.id, state: "uploading", progress: 0 });
    const loaded = new Array(parts.length).fill(0);
    let sentTotal = 0;
    let windowBytes = 0;
    const progress = new LiveProgress((pct) => {
      update({ id: item.id, progress: Math.min(pct, 99.4) });
    });
    progress.begin(file.size);
    const report = () => {
      const total = loaded.reduce((a, b) => a + b, 0);
      if (total > sentTotal) {
        windowBytes += total - sentTotal;
        sentTotal = total;
      }
      progress.set(total);
    };
    const putPart = async (index) => {
      const p = parts[index];
      const blob = file.slice(index * step, Math.min((index + 1) * step, file.size));
      const maxTries = safe ? 5 : 3;
      for (let attemptNum = 0; attemptNum < maxTries; attemptNum++) {
        if (runState.cancelled)
          throw new Error("Upload cancelled");
        const xhrRef = {};
        liveXhrs.add(xhrRef);
        try {
          const st = await xhrSendBytes(p.upload_url, blob, p.ticket, (n) => {
            loaded[index] = n;
            report();
          }, runState, xhrRef);
          if (st.ok || (safe && st.status === 409)) {
            loaded[index] = p.part_size;
            report();
            return;
          }
          throw new Error(`part ${index} failed: ${st.status}`);
        } catch (err) {
          if (runState.cancelled)
            throw err;
          const health = safe ? await directPartHealth(p.upload_url, p.part_id, p.part_size) : null;
          if (health) {
            loaded[index] = p.part_size;
            report();
            return;
          }
          if (attemptNum === maxTries - 1)
            throw err;
          await new Promise((r) => setTimeout(r, 500 * Math.pow(2, Math.min(attemptNum, 3))));
        } finally {
          liveXhrs.delete(xhrRef);
        }
      }
    };
    const maxWorkers = Math.min(parts.length, PARALLEL_STREAMS);
    const fixedDefaults = !tweakEngineEnabled();
    let workerCap = fixedDefaults ? maxWorkers : Math.max(1, Math.min(maxWorkers, INITIAL_WORKERS));
    let next = 0;
    let activeWorkers = 0;
    let failure = null;
    let rateEwma = 0;
    let bestRate = 0;
    const crew = [];
    const emitDbg = () => update({
      id: item.id,
      dbg: {
        w: activeWorkers,
        t: workerCap,
        sat: workerCap >= maxWorkers,
        r: Math.round(rateEwma),
        c: Math.min(next, parts.length),
        n: parts.length,
        k: partTarget,
        g: false
      }
    });
    const runWorker = async () => {
      activeWorkers++;
      try {
        for (;;) {
          if (failure || runState.cancelled)
            return;
          if (activeWorkers > workerCap)
            return;
          if (next >= parts.length)
            return;
          const i = next++;
          try {
            await putPart(i);
          } catch (err) {
            failure ??= err;
            return;
          }
        }
      } finally {
        activeWorkers--;
      }
    };
    const spawnWorker = () => {
      if (failure || runState.cancelled || next >= parts.length || activeWorkers >= workerCap)
        return false;
      crew.push(runWorker());
      emitDbg();
      return true;
    };
    const tuner = setInterval(() => {
      const rate = windowBytes * 1000 / TUNER_INTERVAL_MS;
      windowBytes = 0;
      rateEwma = rateEwma ? rateEwma * 0.6 + rate * 0.4 : rate;
      if (fixedDefaults || failure || runState.cancelled) {
        emitDbg();
        return;
      }
      if (workerCap < maxWorkers && next < parts.length && rateEwma > 0) {
        if (rateEwma >= bestRate * 0.95) {
          bestRate = Math.max(bestRate, rateEwma);
          workerCap += 1;
          spawnWorker();
        } else if (workerCap > 1 && rateEwma < bestRate * 0.7) {
          workerCap -= 1;
        }
      }
      emitDbg();
    }, TUNER_INTERVAL_MS);
    for (let w = 0; w < workerCap; w++)
      spawnWorker();
    for (;; ) {
      const batch = crew.slice();
      await Promise.all(batch);
      if (crew.length <= batch.length)
        break;
    }
    clearInterval(tuner);
    progress.destroy();
    if (failure && !runState.cancelled)
      throw failure;
    if (runState.cancelled)
      throw new Error("Upload cancelled");
    update({ id: item.id, state: "finalizing" });
    const partIds = parts.map((p) => p.part_id);
    let data = null;
    for (let a = 0; a < 3 && !data; a++) {
      try {
        const completeRes = await fetchWithTimeout(DIRECT_PARALLEL_COMPLETE_URL, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ file_id: fileId, ticket, part_ids: partIds })
        }, DIRECT_COMPLETE_TIMEOUT_MS);
        if (runState.cancelled)
          throw new Error("Upload cancelled");
        if (completeRes.ok) {
          data = await completeRes.json();
          break;
        }
        if (safe && completeRes.status === 409) {
          const info = await publicFileInfo(fileId);
          if (info.ok) {
            data = await info.json();
            break;
          }
        }
        let code;
        let msg;
        try {
          const err = await completeRes.json();
          if (err.error)
            code = err.error;
          if (err.message)
            msg = err.message;
        } catch {}
        update({ id: item.id, errorCode: code, errorMessage: msg, state: "error" });
        return;
      } catch (err) {
        if (runState.cancelled)
          throw err;
        if (a === 2)
          throw err;
        await new Promise((r) => setTimeout(r, 1500));
      }
    }
    if (!data) {
      update({ id: item.id, errorCode: "NETWORK_ERROR", state: "error" });
      return;
    }
    update({
      id: item.id,
      serverId: data.id || fileId,
      url: data.url || shareUrl,
      rawUrl: data.raw_url || shareRawUrl,
      downloadUrl: data.download_url || shareDownloadUrl,
      deleteToken: data.delete_token || "",
      expiresAt: data.expires_at ? Number(data.expires_at) : Math.round(Date.now() / 1000) + item.ttlHours * 3600,
      protected: !!data.protected,
      state: "done",
      progress: 100
    });
  })().catch((err) => {
    update({
      id: item.id,
      state: runState.cancelled ? "cancelled" : "error",
      errorCode: runState.cancelled ? undefined : "NETWORK_ERROR",
      errorMessage: err?.message
    });
  });
  return {
    abort: () => {
      runState.cancelled = true;
      liveXhrs.forEach((r) => {
        try {
          r.current?.abort();
        } catch {}
      });
      fallback?.abort();
    }
  };
}
async function uploadTusPart(item, file, sessionId, partIndex, totalParts, signal, hooks, reserveReady, getReserveId, getReserveToken, clearReserve, controls, netTier, probeShared, safe) {
  const partSize = Math.ceil(file.size / totalParts);
  const start = partIndex * partSize;
  const end = Math.min(start + partSize, file.size);
  const partFile = file.slice(start, end);
  const knownText = gzipEligible(item.filename, item.mimeType);
  const knownCompressed = !knownText && isKnownCompressed(item.filename, item.mimeType);
  // Gzip is decided once: text-like names start compressed, others follow the first chunk.
  let probeDone = knownText || knownCompressed || probeShared.done;
  let useGzip = knownText || probeShared.done && probeShared.use;
  const runSession = async () => {
    const partCtrl = new AbortController;
    const partSignal = partCtrl.signal;
    const relayCancel = () => partCtrl.abort();
    signal.addEventListener("abort", relayCancel);
    let inflight = new Map;
    let partTusId = "";
    let healthTimer = null;
    let healthLost = false;
    try {
      await reserveReady;
      const liveReserveId = getReserveId();
      const liveReserveToken = getReserveToken();
      const meta = {
        filename: item.filename,
        mimetype: item.mimeType || "application/octet-stream",
        ttl: String(item.ttlHours),
        file_size: String(file.size),
        session_id: sessionId,
        part_index: String(partIndex),
        total_parts: String(totalParts)
      };
      if (item.customHost)
        meta.host = item.customHost;
      if (liveReserveId) {
        meta.reserve_id = liveReserveId;
        if (liveReserveToken)
          meta.delete_token = liveReserveToken;
      }
      meta.upload_mode = item.uploadMode;
      const metadata = encodeTusMeta(meta);
      const created = await createTusSession(`${UPLOAD_URL}/api/tus`, {
        method: "POST",
        headers: {
          "Upload-Length": String(partFile.size),
          "Upload-Metadata": metadata,
          "Tus-Resumable": "1.0.0"
        },
        signal
      });
      if (created.res && created.res.status === 400 && liveReserveId) {
        await clearReserve?.();
        throw new Error("Upload reservation expired");
      }
      if (created.completing) {
        hooks.onPartState?.({ offset: partFile.size, length: partFile.size, done: true });
        return { status: 202 };
      }
      if (created.alreadyExists)
        throw new Error("TUS part already exists without a resume location");
      let uploadUrl;
      let resumeOffset = 0;
      if (created.existing) {
        uploadUrl = `${UPLOAD_URL}${created.existing.location}`;
        resumeOffset = Math.min(partFile.size, created.existing.offset);
        const tusIdMatch = created.existing.location.match(/\/api\/tus\/(.+)$/);
        if (tusIdMatch) {
          partTusId = tusIdMatch[1];
          controls.onTusCreate(partTusId, { kind: "part", partIndex });
        }
      } else {
        if (!created.res.ok)
          throw new Error(`TUS create failed: ${created.res.status}`);
        const location = created.res.headers.get("Location");
        if (!location)
          throw new Error("No TUS location");
        uploadUrl = `${UPLOAD_URL}${location}`;
        const tusIdMatch = location.match(/\/api\/tus\/(.+)$/);
        if (tusIdMatch) {
          partTusId = tusIdMatch[1];
          controls.onTusCreate(partTusId, { kind: "part", partIndex });
        }
      }
      if (resumeOffset > 0) {
        hooks.onWire?.(resumeOffset);
        hooks.onAcked?.(resumeOffset);
      }
      if (safe && partTusId) {
        hooks.onPartState?.({
          id: partTusId,
          offset: resumeOffset,
          length: partFile.size,
          done: false
        });
        healthTimer = setInterval(async () => {
          const health = await tusHealth(partTusId, partSignal);
          if (!health)
            return;
          if (health.lost) {
            healthLost = true;
            return;
          }
          if (healthStorageReady(health) && Number(health.length) >= partFile.size) {
            hooks.onPartState?.({
              id: partTusId,
              offset: partFile.size,
              length: partFile.size,
              done: false
            });
          }
        }, SAFE_HEALTH_MS);
      }
      const patchChunk = (body, chunkOffset, rawSize) => new Promise((resolve, reject) => {
        let last = 0;
        let idleTimer = null;
        let idleFired = false;
        const finalizingChunk = chunkOffset + rawSize >= partFile.size;
        const idleMs = finalizingChunk ? DIRECT_COMPLETE_TIMEOUT_MS : 30000;
        const armIdle = () => {
          clearTimeout(idleTimer);
          idleTimer = setTimeout(() => {
            idleFired = true;
            try {
              x.abort();
            } catch {}
          }, idleMs);
        };
        const attempt = (tries) => {
          const x = new XMLHttpRequest;
          x.open("PATCH", uploadUrl, true);
          x.setRequestHeader("Content-Type", "application/offset+octet-stream");
          x.setRequestHeader("Upload-Offset", String(chunkOffset));
          x.setRequestHeader("Tus-Resumable", "1.0.0");
          if (useGzip || !probeDone)
            x.setRequestHeader("X-File-Encoding", "gzip");
          const onAbort = () => x.abort();
          partSignal.addEventListener("abort", onAbort);
          const cleanup = () => {
            clearTimeout(idleTimer);
            partSignal.removeEventListener("abort", onAbort);
          };
          const discoverOffset = async () => {
            if (safe && partTusId) {
              const health = await tusHealth(partTusId, partSignal);
              if (health?.lost)
                return null;
              const fromHealth = healthOffset(health);
              if (fromHealth !== null)
                return fromHealth;
            }
            try {
              const res = await fetchWithTimeout(uploadUrl, {
                method: "GET",
                headers: { "Tus-Resumable": "1.0.0" },
                signal: partSignal
              });
              if (!res.ok)
                return null;
              const off = Number(res.headers.get("Upload-Offset"));
              return Number.isFinite(off) ? off : null;
            } catch {
              return null;
            }
          };
          const topUp = () => {
            if (body.size > last) {
              hooks.onWire(body.size - last);
              last = body.size;
            }
          };
          x.upload.onprogress = (ev) => {
            armIdle();
            if (ev.lengthComputable && ev.loaded > last) {
              hooks.onWire(ev.loaded - last);
              last = ev.loaded;
            }
          };
          const retry = () => {
            cleanup();
            if (tries < 3 && !partSignal.aborted) {
              setTimeout(() => attempt(tries + 1), 500 * 2 ** tries + Math.random() * 250);
            } else {
              reject(new Error(`TUS patch failed: ${x.status || "network"}`));
            }
          };
          x.onload = () => {
            topUp();
            cleanup();
            if (x.status === 429 && tries < 6 && !partSignal.aborted) {
              let waitMs = 1000;
              try {
                const raw = x.getResponseHeader("Retry-After") || x.getResponseHeader("x-ratelimit-after");
                if (raw) {
                  const secs = Number(String(raw).split(",")[0].trim());
                  if (Number.isFinite(secs) && secs >= 0)
                    waitMs = Math.min(secs * 1000, 60000);
                }
              } catch {}
              setTimeout(() => attempt(tries + 1), waitMs + Math.random() * 250);
              return;
            }
            const transient = x.status === 0 || x.status === 408 || x.status >= 500;
            // A 409 means the server offset moved under us; adopt it if it is ahead.
            if (x.status === 409 && !partSignal.aborted) {
              discoverOffset().then((serverOff) => {
                if (partSignal.aborted) {
                  reject(new Error("Upload cancelled"));
                  return;
                }
                if (serverOff !== null && serverOff > chunkOffset) {
                  resolve({ status: 204, newOffset: serverOff, text: "" });
                } else if (tries < 3) {
                  setTimeout(() => attempt(tries + 1), (serverOff === null ? 500 : 150) * 2 ** tries + Math.random() * 150);
                } else {
                  reject(new Error(`TUS patch failed: 409 at offset ${chunkOffset}`));
                }
              });
              return;
            }
            if (transient && tries < 3) {
              if (!partSignal.aborted) {
                setTimeout(() => attempt(tries + 1), 500 * 2 ** tries + Math.random() * 250);
                return;
              }
            }
            resolve({
              status: x.status,
              newOffset: Number(x.getResponseHeader("Upload-Offset")) || 0,
              text: x.responseText || ""
            });
          };
          x.onerror = retry;
          x.ontimeout = retry;
          x.onabort = () => {
            cleanup();
            if (idleFired && tries < 6 && !partSignal.aborted) {
              setTimeout(() => attempt(tries + 1), 1000);
              return;
            }
            reject(new Error(signal.aborted ? "Upload cancelled" : "chunk dropped"));
          };
          armIdle();
          x.send(body);
        };
        attempt(0);
      });
      const profile = TIER_CHUNK[netTier];
      const partMaxChunk = Math.max(TUS_MIN_CHUNK, Math.min(profile.max, Math.ceil(partFile.size / 4)));
      let chunkSize = Math.min(profile.start, TUS_MIN_CHUNK);
      hooks.onChunk?.(chunkSize);
      let adaptBytes = 0;
      let adaptSince = performance.now();
      let ackedOffset = resumeOffset;
      const noteAcked = (bytes) => {
        hooks.onAcked(bytes);
        ackedOffset += bytes;
        hooks.onPartOffset?.(ackedOffset);
        adaptBytes += bytes;
        const dt = (performance.now() - adaptSince) / 1000;
        if (dt >= 1 && adaptBytes > 0) {
          const rate = adaptBytes / dt;
          chunkSize = Math.round(Math.min(partMaxChunk, Math.max(TUS_MIN_CHUNK, rate * TUS_TARGET_CHUNK_SECS)));
          hooks.onChunk?.(chunkSize);
          adaptBytes = 0;
          adaptSince = performance.now();
        }
      };
      let offset = resumeOffset;
      while (offset < partFile.size || inflight.size > 0) {
        if (signal.aborted)
          throw new Error("Upload cancelled");
        if (healthLost)
          throw new Error("TUS part session lost");
        const windowSize = probeDone && (hooks.rateHint?.() ?? 0) > 4 * 1024 * 1024 ? PIPELINE_WINDOW : 1;
        while (inflight.size < windowSize && offset < partFile.size) {
          await hooks.beforeChunk?.();
          const at = offset;
          const chunkEnd = Math.min(at + chunkSize, partFile.size);
          const chunk = partFile.slice(at, chunkEnd);
          if (probeShared.done && !probeDone) {
            probeDone = true;
            useGzip = probeShared.use;
          }
          let wireSize = chunk.size;
          hooks.onDispatch?.(chunk.size);
          const bodyP = useGzip || !probeDone ? gzipBlob(chunk) : Promise.resolve(chunk);
          const p = bodyP.then((b) => {
            wireSize = b.size;
            return patchChunk(b, at, chunk.size);
          });
          inflight.set(at, { p, rawSize: chunk.size, wireSize });
          offset = chunkEnd;
        }
        const head = inflight.entries().next();
        if (head.done)
          break;
        const [headOffset, slot] = head.value;
        let r;
        try {
          r = await slot.p;
        } finally {
          inflight.delete(headOffset);
        }
        if (r.status === 204) {
          if (!probeDone) {
            probeDone = true;
            useGzip = slot.wireSize <= slot.rawSize * 0.9;
            probeShared.done = true;
            probeShared.use = useGzip;
            hooks.onGzip?.(useGzip);
          }
          noteAcked(slot.rawSize);
        } else if (r.status >= 200 && r.status < 300) {
          noteAcked(slot.rawSize);
          const pending = [...inflight.values()];
          inflight.clear();
          await Promise.allSettled(pending.map((s) => s.p));
          let data;
          try {
            data = JSON.parse(r.text);
          } catch {}
          hooks.onPartState?.({
            id: partTusId,
            offset: partFile.size,
            length: partFile.size,
            done: true
          });
          return { status: 200, data };
        } else {
          throw new Error(`TUS patch failed: ${r.status}`);
        }
      }
      if (healthLost)
        throw new Error("TUS part session lost");
      await reserveReady;
      const finalReserveId = getReserveId();
      const finalRes = await fetchWithRetry(uploadUrl, {
        method: "PATCH",
        headers: {
          "Content-Type": "application/offset+octet-stream",
          "Upload-Offset": String(offset),
          "Tus-Resumable": "1.0.0",
          ...finalReserveId ? { "X-Reserve-Id": finalReserveId } : {}
        },
        body: new Blob([]),
        signal
      }, 3, 1000, DIRECT_COMPLETE_TIMEOUT_MS);
      if (finalRes.ok) {
        const data = await finalRes.json();
        hooks.onPartState?.({
          id: partTusId,
          offset: partFile.size,
          length: partFile.size,
          done: true
        });
        return { status: 200, data };
      }
      if (finalRes.status === 404 && safe)
        throw new Error("TUS part session lost during finalize");
      return { status: finalRes.status };
    } finally {
      if (healthTimer)
        clearInterval(healthTimer);
      signal.removeEventListener("abort", relayCancel);
      partCtrl.abort();
      for (const s of inflight.values())
        s.p.catch(() => {});
    }
  };
  let lastError;
  const maxSessionTries = safe ? 5 : 3;
  // Each retry starts a fresh TUS session: a half-written session's offset cannot be trusted.
  for (let attemptNo = 0;attemptNo < maxSessionTries; attemptNo++) {
    try {
      return await runSession();
    } catch (err) {
      if (signal.aborted || /cancel/i.test(String(err?.message ?? ""))) {
        throw err;
      }
      lastError = err;
      console.warn(`TUS part ${partIndex} attempt ${attemptNo + 1} failed (${err?.message}); retrying with fresh session`);
      hooks.onReset();
      await new Promise((r) => setTimeout(r, 500 * 2 ** attemptNo + Math.random() * 250));
    }
  }
  throw lastError;
}
// Consecutive PATCH failures tolerated before a stream upload gives up; safe mode doubles it.
const STREAM_MAX_FAILURES = 5;
const STREAM_RETRY_DELAY_MS = 600;

function startTusStreamUpload(item, file, controls) {
  const { update } = controls;
  const controller = new AbortController;
  const signal = controller.signal;
  const safe = !!item.safeUpload;
  let tusId = item.tusId || "";
  let healthTimer = null;

  (async () => {
    const resumeProgress = Math.max(0, Math.min(99.4, Number(item.progress) || 0));
    update({ id: item.id, state: "uploading", progress: resumeProgress });

    const probeOffset = async (url) => {
      try {
        const res = await fetchWithTimeout(url, {
          method: "GET",
          headers: { "Tus-Resumable": "1.0.0" },
          signal
        });
        if (!res.ok)
          return null;
        const off = Number(res.headers.get("Upload-Offset"));
        return Number.isFinite(off) ? Math.max(0, off) : null;
      } catch {
        return null;
      }
    };

    let reserveId = item.reserveId || "";
    let reserveUrl = item.reserveUrl || "";
    let reserveRawUrl = item.reserveRawUrl || "";
    let reserveDownloadUrl = item.reserveDownloadUrl || "";
    let reserveDeleteToken = item.reserveDeleteToken || "";
    if (item.quickLink && !reserveId) {
      try {
        const reserve = await reserveQuickLink(item, signal);
        if (reserve) {
          reserveId = reserve.id;
          reserveUrl = reserve.url;
          reserveRawUrl = reserve.rawUrl;
          reserveDownloadUrl = reserve.downloadUrl;
          reserveDeleteToken = reserve.deleteToken;
          if (reserveUrl)
            update({
              id: item.id,
              reserveId,
              reserveUrl,
              reserveRawUrl,
              reserveDownloadUrl,
              reserveDeleteToken,
              serverId: reserveId
            });
        }
      } catch {}
    }

    const meta = {
      filename: item.filename,
      mimetype: item.mimeType || "application/octet-stream",
      ttl: String(item.ttlHours),
      file_size: String(file.size)
    };
    if (item.customHost)
      meta.host = item.customHost;
    if (item.uploadMode)
      meta.upload_mode = item.uploadMode;

    let uploadUrl = "";
    let offset = 0;
    let serverLost = false;
    const createStreamSession = async (withReserve) => {
      const createMeta = { ...meta };
      if (withReserve && reserveId) {
        createMeta.reserve_id = reserveId;
        if (reserveDeleteToken)
          createMeta.delete_token = reserveDeleteToken;
      }
      return createTusSession(`${UPLOAD_URL}/api/tus`, {
        method: "POST",
        headers: {
          "Upload-Length": String(file.size),
          "Upload-Metadata": encodeTusMeta(createMeta),
          "Tus-Resumable": "1.0.0"
        },
        signal
      });
    };
    const adoptSession = async (withReserve) => {
      let created = await createStreamSession(withReserve);
      if (!created.res?.ok && created.res?.status === 400 && reserveId) {
        reserveId = "";
        reserveUrl = "";
        reserveRawUrl = "";
        reserveDownloadUrl = "";
        reserveDeleteToken = "";
        update({
          id: item.id,
          reserveId: "",
          reserveUrl: "",
          reserveRawUrl: "",
          reserveDownloadUrl: "",
          reserveDeleteToken: "",
          serverId: ""
        });
        created = await createStreamSession(false);
      }
      if (created.completing || created.alreadyExists)
        throw new Error("Upload session was rejected; please retry");
      if (!created.res?.ok)
        throw new Error(`TUS create failed: ${created.res?.status ?? "network"}`);
      const location = created.res.headers.get("Location");
      if (!location)
        throw new Error("No TUS location");
      uploadUrl = `${UPLOAD_URL}${location}`;
      const idMatch = location.match(/\/api\/tus\/(.+)$/);
      if (idMatch) {
        tusId = idMatch[1];
        controls.onTusCreate?.(tusId, { kind: "stream" });
      }
    };
    if (item.tusId) {
      const candidate = `${UPLOAD_URL}/api/tus/${encodeURIComponent(item.tusId)}`;
      const existingOffset = await probeOffset(candidate);
      if (existingOffset !== null) {
        uploadUrl = candidate;
        tusId = item.tusId;
        offset = Math.min(existingOffset, file.size);
        controls.onTusCreate?.(tusId, { kind: "stream" });
      }
    }

    if (!uploadUrl)
      await adoptSession(!!reserveId);

    const progress = new LiveProgress((pct) => {
      update({ id: item.id, progress: Math.min(pct, 99.4) });
    });
    progress.begin(file.size);
    if (offset > 0)
      progress.set(offset);
    update({ id: item.id, tusOffset: offset });
    if (safe) {
      healthTimer = setInterval(async () => {
        const health = await tusHealth(tusId, signal);
        if (!health)
          return;
        if (health.lost) {
          serverLost = true;
          return;
        }
        const hOff = healthOffset(health);
        if (hOff !== null)
          update({ id: item.id, tusOffset: hOff });
      }, SAFE_HEALTH_MS);
    }

    const headersFor = (offset) => ({
      "Content-Type": "application/offset+octet-stream",
      "Upload-Offset": String(offset),
      "Tus-Resumable": "1.0.0",
      ...reserveId ? { "X-Reserve-Id": reserveId } : {},
      ...reserveId && reserveDeleteToken ? { "X-Delete-Token": reserveDeleteToken } : {}
    });

    const discoverOffset = async () => {
      if (safe && tusId) {
        const health = await tusHealth(tusId, signal);
        if (health?.lost)
          return null;
        const fromHealth = healthOffset(health);
        if (fromHealth !== null)
          return fromHealth;
      }
      return probeOffset(uploadUrl);
    };

    const patch = (start, end, timeoutMs) => new Promise((resolve, reject) => {
      const x = new XMLHttpRequest;
      x.open("PATCH", uploadUrl, true);
      const headers = headersFor(start);
      for (const name of Object.keys(headers))
        x.setRequestHeader(name, headers[name]);
      if (timeoutMs)
        x.timeout = timeoutMs;
      let last = 0;
      x.upload.onprogress = (ev) => {
        if (!ev.lengthComputable || ev.loaded <= last)
          return;
        last = ev.loaded;
        progress.set(Math.min(start + ev.loaded, file.size));
      };
      const onAbort = () => x.abort();
      signal.addEventListener("abort", onAbort, { once: true });
      const cleanup = () => signal.removeEventListener("abort", onAbort);
      x.onload = () => {
        cleanup();
        const offset = Number(x.getResponseHeader("Upload-Offset"));
        resolve({
          status: x.status,
          text: x.responseText || "",
          offset: Number.isFinite(offset) ? offset : 0
        });
      };
      x.onerror = () => {
        cleanup();
        reject(new Error("network"));
      };
      x.ontimeout = () => {
        cleanup();
        reject(new Error("timeout"));
      };
      x.onabort = () => {
        cleanup();
        reject(signal.aborted ? new Error("Upload cancelled") : new Error("aborted"));
      };
      x.send(file.slice(start, end));
    });

    let failures = 0;
    let chunkSize = STREAM_START_CHUNK;
    let finished = false;
    const maxStreamFailures = safe ? STREAM_MAX_FAILURES * 2 : STREAM_MAX_FAILURES;
    const complete = (data) => {
      if (healthTimer)
        clearInterval(healthTimer);
      progress.destroy();
      const merged = { ...data };
      if (reserveUrl && !merged.url)
        merged.url = reserveUrl;
      if (reserveRawUrl && !merged.raw_url)
        merged.raw_url = reserveRawUrl;
      if (reserveDownloadUrl && !merged.download_url)
        merged.download_url = reserveDownloadUrl;
      if (reserveDeleteToken && !merged.delete_token)
        merged.delete_token = reserveDeleteToken;
      update({ id: item.id, state: "finalizing" });
      update({
        id: item.id,
        serverId: merged.id || reserveId || "",
        url: merged.url,
        rawUrl: merged.raw_url || "",
        downloadUrl: merged.download_url || "",
        deleteToken: merged.delete_token || "",
        expiresAt: merged.expires_at ? Number(merged.expires_at) : Math.round(Date.now() / 1000) + item.ttlHours * 3600,
        protected: !!merged.protected,
        state: "done",
        progress: 100
      });
    };
    const readyFile = async () => {
      for (const id of [reserveId, tusId]) {
        if (!id)
          continue;
        try {
          const res = await fetchWithTimeout(publicFileInfo(id), { signal });
          if (!res.ok)
            continue;
          const info = await res.json();
          if (info?.status === "ready" && info.url)
            return info;
        } catch {}
      }
      return null;
    };
    // Safe mode restarts a lost session; if the reserve file is already ready, nothing is left to send.
    const restartSession = async () => {
      const info = await readyFile();
      if (info) {
        complete({
          ...info,
          delete_token: reserveDeleteToken || info.delete_token || ""
        });
        finished = true;
        serverLost = false;
        offset = file.size;
        return;
      }
      uploadUrl = "";
      offset = 0;
      serverLost = false;
      failures = 0;
      progress.set(0);
      update({ id: item.id, progress: 0, tusOffset: 0, state: "uploading" });
      await adoptSession(!!reserveId);
    };

    while (offset < file.size || serverLost) {
      if (finished)
        break;
      if (signal.aborted)
        throw new Error("Upload cancelled");
      if (safe && serverLost) {
        await restartSession();
        continue;
      }
      const end = Math.min(offset + chunkSize, file.size);
      const chunkStarted = performance.now();
      let r;
      try {
        r = await patch(offset, end, 0);
      } catch (err) {
        if (signal.aborted)
          throw new Error("Upload cancelled");
        failures++;
        if (failures > maxStreamFailures)
          throw err;
        const next = await discoverOffset();
        if (next === null) {
          if (safe) {
            serverLost = true;
            continue;
          }
          throw err;
        }
        offset = Math.min(next, file.size);
        progress.set(offset);
        await new Promise((res) => setTimeout(res, STREAM_RETRY_DELAY_MS * failures));
        continue;
      }

      if (r.status === 200) {
        let data;
        try {
          data = JSON.parse(r.text);
        } catch {
          throw new Error("Upload finished but the server response was unreadable");
        }
        complete(data);
        return;
      }
      if (r.status === 204) {
        // Trust the larger of the reported and sent offset; proxies sometimes drop the header.
        const next = r.offset > offset ? r.offset : end;
        const sent = Math.max(0, next - offset);
        const secs = (performance.now() - chunkStarted) / 1000;
        offset = next;
        progress.set(offset);
        update({ id: item.id, tusOffset: offset });
        failures = 0;
        if (secs > 0.2 && sent > 0) {
          const rate = sent / secs;
          chunkSize = Math.max(STREAM_MIN_CHUNK, Math.min(STREAM_MAX_CHUNK, Math.round(rate * STREAM_TARGET_SECS)));
        }
        continue;
      }
      if (r.status === 409) {
        const next = await discoverOffset();
        if (next === null) {
          if (safe) {
            serverLost = true;
            continue;
          }
          throw new Error("Upload session was lost");
        }
        offset = next;
        continue;
      }
      if (r.status === 404 && safe) {
        serverLost = true;
        continue;
      }
      if (r.status === 429) {
        failures++;
        if (failures > maxStreamFailures)
          throw new Error("Upload rate limited");
        await new Promise((res) => setTimeout(res, 2000 * failures));
        continue;
      }
      const retryable = r.status === 400 || r.status === 408 || r.status >= 500;
      if (!retryable)
        throw new Error(`Upload rejected: ${r.status}`);
      failures++;
      if (failures > maxStreamFailures)
        throw new Error(`Upload failed: ${r.status}`);
      const next = await discoverOffset();
      if (next === null) {
        if (safe) {
          serverLost = true;
          continue;
        }
        throw new Error("Upload session was lost");
      }
      if (next > offset) {
        offset = next;
        progress.set(offset);
      }
      await new Promise((res) => setTimeout(res, STREAM_RETRY_DELAY_MS * failures));
      continue;
    }

    if (finished)
      return;
    offset = (await discoverOffset()) ?? file.size;
    if (offset < file.size)
      throw new Error("Upload incomplete");
    update({ id: item.id, state: "finalizing" });
    // Zero-length PATCH at the final offset asks the server to merge and complete.
    const fin = await patch(file.size, file.size, DIRECT_COMPLETE_TIMEOUT_MS);
    if (fin.status === 404) {
      const info = await waitForReservedFile(reserveId || tusId, signal, 60000);
      if (info) {
        complete({
          ...info,
          delete_token: reserveDeleteToken || info.delete_token || ""
        });
        return;
      }
      throw new Error("Upload finished but the file could not be confirmed");
    }
    if (fin.status !== 200)
      throw new Error(`Upload finished but finalize failed: ${fin.status}`);
    let data;
    try {
      data = JSON.parse(fin.text);
    } catch {
      throw new Error("Upload finished but the server response was unreadable");
    }
    complete(data);
  })().catch((err) => {
    if (healthTimer)
      clearInterval(healthTimer);
    if (signal.aborted) {
      update({ id: item.id, state: "cancelled" });
      return;
    }
    // A failed resume parks the item paused; `error` would drop it and lose the resume state.
    if (item.resumeAttempt) {
      update({
        id: item.id,
        state: "paused",
        pausedAt: Date.now(),
        errorCode: "",
        errorMessage: ""
      });
      return;
    }
    update({
      id: item.id,
      errorCode: "NETWORK_ERROR",
      errorMessage: err?.message,
      state: "error"
    });
  });

  return {
    abort: () => controller.abort()
  };
}

function startTusUpload(item, file, controls) {
  const { update } = controls;
  const controller = new AbortController;
  const signal = controller.signal;
  const run = (async () => {
    const sessionId = item.tusSessionId || generateId();
    const probeState = { done: false, use: false };
    let curGzip = null;
    const tier = detectNetTier();
    let hintBps = 0;
    try {
      const stored = Number(localStorage.getItem(LAST_SPEED_KEY));
      if (Number.isFinite(stored) && stored > 0)
        hintBps = stored;
      else {
        const conn = navigator.connection;
        if (conn?.downlink)
          hintBps = conn.downlink * 1e6 / 8;
      }
    } catch {}
    const fixedDefaults = !tweakEngineEnabled();
    const safe = !!item.safeUpload;
    const maxPartRuns = safe ? MAX_PART_RUNS + 3 : MAX_PART_RUNS;
    const partsSnapshot = item.tusParts && typeof item.tusParts === "object" ? { ...item.tusParts } : {};
    let lastPartsSave = 0;
    const savePart = (index, patch, force) => {
      const key = String(index);
      partsSnapshot[key] = { ...(partsSnapshot[key] || {}), ...patch };
      const now = Date.now();
      if (!force && now - lastPartsSave < 1500)
        return;
      lastPartsSave = now;
      update({ id: item.id, tusParts: { ...partsSnapshot } });
    };
    const wantedStreams = fixedDefaults ? PARALLEL_STREAMS : streamsForRate(hintBps || TIER_DEFAULT_BPS[tier]);
    const savedParts = Number.isInteger(item.numParts) && item.numParts > 0 ? item.numParts : 0;
    const numParts = savedParts
      ? Math.max(1, Math.min(TUS_MAX_PARTS, savedParts))
      : Math.max(1, Math.min(TUS_MAX_PARTS, PARALLEL_STREAMS * 3, Math.floor(file.size / TUS_MIN_PART)));
    let targetWorkers = Math.min(wantedStreams, numParts);
    const uploadStart = performance.now();
    const PROGRESS_LEAD_BYTES = 16 * 1024 * 1024;
    const resumeProgress = Math.max(0, Math.min(99.4, Number(item.progress) || 0));
    const progress = new LiveProgress((pct) => {
      update({ id: item.id, progress: Math.min(pct, 99.4) });
    }, { maxProjectAhead: 4 * 1024 * 1024 });
    progress.begin(file.size);
    const partSent = new Array(numParts).fill(0);
    const partAcked = new Array(numParts).fill(0);
    let learnedAcked = 0;
    const sum = (a) => a.reduce((x, y) => x + y, 0);
    const refreshTotal = () => {
      if (curGzip !== true)
        return;
      const ackedSum = sum(partAcked);
      const ratio = ackedSum > 0 ? sum(partSent) / ackedSum : 1;
      progress.setTotal(Math.min(file.size, Math.max(sum(partSent), Math.round(file.size * ratio))));
    };
    update({
      id: item.id,
      state: "uploading",
      progress: resumeProgress,
      tusSessionId: sessionId,
      numParts
    });
    let reserveId = item.reserveId || "";
    let reserveUrl = item.reserveUrl || "";
    let reserveRawUrl = item.reserveRawUrl || "";
    let reserveDownloadUrl = item.reserveDownloadUrl || "";
    let reserveDeleteToken = item.reserveDeleteToken || "";
    let reserveCleared = false;
    const clearReserve = async () => {
      if (!reserveId || reserveCleared)
        return;
      reserveCleared = true;
      reserveId = "";
      reserveUrl = "";
      reserveRawUrl = "";
      reserveDownloadUrl = "";
      reserveDeleteToken = "";
      update({
        id: item.id,
        reserveId: "",
        reserveUrl: "",
        reserveRawUrl: "",
        reserveDownloadUrl: "",
        reserveDeleteToken: "",
        serverId: ""
      });
    };
    let reserveReady = Promise.resolve();
    if (item.quickLink && !reserveId) {
      reserveReady = (async () => {
        try {
          const reserve = await reserveQuickLink(item, signal);
          if (reserve) {
            reserveId = reserve.id;
            reserveUrl = reserve.url;
            reserveRawUrl = reserve.rawUrl;
            reserveDownloadUrl = reserve.downloadUrl;
            reserveDeleteToken = reserve.deleteToken;
            if (reserveUrl)
              update({
                id: item.id,
                reserveId,
                reserveUrl,
                reserveRawUrl,
                reserveDownloadUrl,
                reserveDeleteToken,
                serverId: reserveId
              });
          }
        } catch {}
      })();
    }
    const getReserveId = () => reserveId;
    const getReserveToken = () => reserveDeleteToken;
    const partTusIds = new Set([
      ...(Array.isArray(item.tusIds) ? item.tusIds : []),
      item.tusId
    ].filter((id) => typeof id === "string" && id));
    const partControls = {
      ...controls,
      onTusCreate: (tusId, info) => {
        if (tusId)
          partTusIds.add(tusId);
        controls.onTusCreate?.(tusId, info);
      }
    };
    const results = new Array(numParts);
    let cursor = 0;
    const runCounts = new Array(numParts).fill(0);
    const retryQueue = [];
    let activeWorkers = 0;
    let saturated = false;
    let ticksSinceAdd = 99;
    let lastWindowRate = 0;
    let lastAckAt = performance.now();
    let windowBytes = 0;
    let workerError;
    let curChunk = 0;
    let respawnTimer = null;
    let inflightEst = 0;
    let rateNow = 0;
    const paceGate = async () => {
      const cap = Math.max(24 * 1024 * 1024, Math.min(96 * 1024 * 1024, rateNow * 6));
      const gateStart = performance.now();
      while (!signal.aborted && inflightEst > cap) {
        await new Promise((r) => setTimeout(r, 200));
        if (performance.now() - gateStart > 60000 && performance.now() - lastAckAt > 60000) {
          inflightEst = 0;
          break;
        }
      }
    };
    const runWorker = async () => {
      activeWorkers++;
      try {
        for (;; ) {
          if (signal.aborted)
            throw new Error("Upload cancelled");
          if (activeWorkers > Math.max(1, targetWorkers))
            return;
          const i = retryQueue.length > 0 ? retryQueue.shift() : cursor < numParts ? cursor++ : undefined;
          if (i === undefined)
            return;
          runCounts[i]++;
          try {
            results[i] = await uploadTusPart(item, file, sessionId, i, numParts, signal, {
              onWire: (bytes) => {
                partSent[i] += bytes;
                progress.set(Math.min(sum(partSent), sum(partAcked) + PROGRESS_LEAD_BYTES));
              },
              onAcked: (bytes) => {
                partAcked[i] += bytes;
                windowBytes += bytes;
                learnedAcked += bytes;
                lastAckAt = performance.now();
                inflightEst = Math.max(0, inflightEst - bytes);
                refreshTotal();
                progress.set(sum(partAcked));
              },
              onReset: () => {
                partSent[i] = 0;
                partAcked[i] = 0;
                refreshTotal();
              },
              onChunk: (size) => {
                curChunk = size;
              },
              onGzip: (on) => {
                curGzip = on;
                refreshTotal();
              },
              beforeChunk: paceGate,
              onDispatch: (bytes) => {
                inflightEst += bytes;
              },
              onPartState: (patch) => {
                savePart(i, patch, true);
              },
              onPartOffset: (offset) => {
                savePart(i, { offset }, false);
              },
              rateHint: () => rateNow
            }, reserveReady, getReserveId, getReserveToken, clearReserve, partControls, tier, probeState, safe).catch((err) => {
              if (signal.aborted)
                return { status: 0 };
              throw err;
            });
          } catch (err) {
            if (signal.aborted)
              return;
            if (runCounts[i] < maxPartRuns) {
              retryQueue.push(i);
              scheduleRespawn();
            } else {
              workerError ??= err;
            }
          }
        }
      } finally {
        activeWorkers--;
        releaseStream();
      }
    };
    const workerPromises = [];
    const emitTuner = () => update({
      id: item.id,
      dbg: {
        w: activeWorkers,
        t: targetWorkers,
        sat: saturated,
        r: lastWindowRate,
        c: Math.min(cursor, numParts),
        n: numParts,
        k: curChunk,
        g: curGzip
      }
    });
    const spawnWorker = () => {
      // `tryAcquireStream` enforces the global budget; the tuner probes again later.
      if (!tryAcquireStream())
        return false;
      workerPromises.push(runWorker().catch((err) => {
        workerError ??= err;
      }));
      emitTuner();
      return true;
    };
    const scheduleRespawn = () => {
      if (respawnTimer !== null || signal.aborted)
        return;
      respawnTimer = setTimeout(() => {
        respawnTimer = null;
        if (signal.aborted)
          return;
        if (cursor < numParts || retryQueue.length > 0)
          spawnWorker();
      }, 800 + Math.random() * 400);
    };
    const initialWorkers = fixedDefaults ? targetWorkers : Math.max(1, Math.min(targetWorkers, numParts, INITIAL_WORKERS));
    for (let k = 0;k < initialWorkers; k++)
      spawnWorker();
    const PROBE_EVERY_TICKS = 10;
    const SHRINK_TICKS = 3;
    const WARMUP_TICKS = 2;
    let probeTicks = 0;
    let shrinkTicks = 0;
    let preTrialRate = 0;
    let evaluatingTrial = false;
    let rateEwma = 0;
    let baselineRate = 0;
    const recentRates = [];
    // Add a worker every 10 steady ticks if throughput rises 5%; sustained drops back off.
    const tuner = setInterval(() => {
      const rate = windowBytes * 1000 / TUNER_INTERVAL_MS;
      windowBytes = 0;
      rateEwma = rateEwma ? rateEwma * 0.6 + rate * 0.4 : rate;
      rateNow = rateEwma;
      if (fixedDefaults || signal.aborted) {
        lastWindowRate = rateEwma;
        emitTuner();
        return;
      }
      recentRates.push(rate);
      if (recentRates.length > 3)
        recentRates.shift();
      if (baselineRate === 0 && rateEwma > 0)
        baselineRate = rateEwma;
      if (ticksSinceAdd < WARMUP_TICKS) {
        ticksSinceAdd++;
        lastWindowRate = rateEwma;
        emitTuner();
        return;
      }
      const maxConc = Math.min(PARALLEL_STREAMS, numParts);
      const workLeft = cursor < numParts || retryQueue.length > 0;
      const steadyMin = recentRates.length >= 3 ? Math.min(...recentRates) : rateEwma;
      if (evaluatingTrial) {
        evaluatingTrial = false;
        probeTicks = 0;
        if (rateEwma >= preTrialRate * 1.05) {
          saturated = false;
          baselineRate = rateEwma;
        } else {
          saturated = true;
          targetWorkers = Math.max(1, activeWorkers - 1);
        }
        lastWindowRate = rateEwma;
        ticksSinceAdd++;
        emitTuner();
        return;
      }
      if (workLeft && activeWorkers < maxConc && baselineRate > 0 && steadyMin >= baselineRate * 0.9) {
        shrinkTicks = 0;
        if (spawnWorker()) {
          saturated = false;
          targetWorkers = activeWorkers + 1;
          ticksSinceAdd = 0;
          baselineRate = rateEwma;
        } else {
          saturated = true;
          probeTicks = 0;
        }
      } else if (baselineRate > 0 && rateEwma < baselineRate * 0.7 && activeWorkers > 1) {
        saturated = true;
        if (++shrinkTicks >= SHRINK_TICKS) {
          shrinkTicks = 0;
          targetWorkers = activeWorkers - 1;
          baselineRate = rateEwma;
        }
      } else {
        shrinkTicks = 0;
        if (workLeft && activeWorkers < maxConc && baselineRate > 0 && ++probeTicks >= PROBE_EVERY_TICKS) {
          probeTicks = 0;
          preTrialRate = rateEwma;
          evaluatingTrial = true;
          if (spawnWorker()) {
            targetWorkers = activeWorkers + 1;
            ticksSinceAdd = 0;
          } else {
            evaluatingTrial = false;
          }
        }
      }
      lastWindowRate = rateEwma;
      ticksSinceAdd++;
      emitTuner();
    }, TUNER_INTERVAL_MS);
    for (;; ) {
      const batch = workerPromises.slice();
      await Promise.all(batch);
      if (workerPromises.length <= batch.length)
        break;
    }
    if (respawnTimer !== null) {
      clearTimeout(respawnTimer);
      respawnTimer = null;
    }
    clearInterval(tuner);
    progress.destroy();
    if (workerError && !signal.aborted)
      throw workerError;
    const settled = results.filter((r) => !!r);
    const elapsedSec = (performance.now() - uploadStart) / 1000;
    if (elapsedSec > 1 && learnedAcked > 0) {
      try {
        localStorage.setItem(LAST_SPEED_KEY, String(learnedAcked / elapsedSec));
      } catch {}
    }
    const findReadyPart = async (ids, timeoutMs) => {
      const list = [...new Set(ids)].filter(Boolean).slice(0, 24);
      if (!list.length)
        return null;
      const deadline = performance.now() + timeoutMs;
      while (!signal.aborted && performance.now() < deadline) {
        for (const id of list) {
          try {
            const res = await fetchWithTimeout(publicFileInfo(id), { signal }, 8000);
            if (res.ok) {
              const info = await res.json();
              if (info?.status === "ready" && info.url)
                return info;
            }
          } catch {}
          if (signal.aborted)
            return null;
        }
        await new Promise((r) => setTimeout(r, 1200));
      }
      return null;
    };
    let merged = settled.find((r) => r.status === 200 && r.data?.url);
    if (!merged && reserveId && !signal.aborted) {
      update({ id: item.id, state: "finalizing" });
      const info = await waitForReservedFile(reserveId, signal);
      if (info) {
        merged = {
          status: 200,
          data: {
            ...info,
            delete_token: reserveDeleteToken || info.delete_token || ""
          }
        };
      }
    }
    if (!merged && !signal.aborted) {
      update({ id: item.id, state: "finalizing" });
      const info = await findReadyPart(partTusIds, 8000);
      if (info) {
        merged = {
          status: 200,
          data: {
            ...info,
            delete_token: reserveDeleteToken || info.delete_token || ""
          }
        };
      }
    }
    if (merged && "data" in merged && merged.data) {
      const data = merged.data;
      if (reserveUrl && !data.url)
        data.url = reserveUrl;
      if (reserveRawUrl && !data.raw_url)
        data.raw_url = reserveRawUrl;
      if (reserveDownloadUrl && !data.download_url)
        data.download_url = reserveDownloadUrl;
      if (reserveDeleteToken && !data.delete_token)
        data.delete_token = reserveDeleteToken;
      update({ id: item.id, state: "finalizing" });
      update({
        id: item.id,
        serverId: data.id || reserveId || "",
        url: data.url,
        rawUrl: data.raw_url || "",
        downloadUrl: data.download_url || "",
        deleteToken: data.delete_token || "",
        expiresAt: data.expires_at ? Number(data.expires_at) : Math.round(Date.now() / 1000) + item.ttlHours * 3600,
        protected: !!data.protected,
        state: "done",
        progress: 100
      });
      return;
    }
    if (item.resumeAttempt) {
      update({
        id: item.id,
        state: "paused",
        pausedAt: Date.now(),
        errorCode: "",
        errorMessage: ""
      });
      return;
    }
    update({
      id: item.id,
      errorCode: "UPLOAD_FAILED",
      errorMessage: "Upload finished but the server did not confirm the merge (part statuses: " + [...new Set(settled.map((r) => r && r.status))].join(",") + "). Check your files before retrying.",
      state: "error"
    });
  })().catch((err) => {
    if (signal.aborted) {
      update({ id: item.id, state: "cancelled" });
      return;
    }
    if (item.resumeAttempt) {
      update({
        id: item.id,
        state: "paused",
        pausedAt: Date.now(),
        errorCode: "",
        errorMessage: ""
      });
      return;
    }
    update({
      id: item.id,
      errorCode: "NETWORK_ERROR",
      errorMessage: err?.message,
      state: "error"
    });
  });
  return {
    abort: () => controller.abort()
  };
}
