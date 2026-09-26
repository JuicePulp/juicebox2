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
  streamsForRate,
  detectNetTier,
  TIER_DEFAULT_BPS,
  TIER_CHUNK,
  gzipEligible,
  isKnownCompressed,
  RESERVE_URL,
  DIRECT_RESERVE_URL,
  DIRECT_COMPLETE_URL,
  readUploadMode
} from "./upload-config.js";
import { tryAcquireStream, releaseStream } from "./stream-limiter.js";
import { compressFile } from "./compress.js";
import { encodeTusMeta } from "./tus.js";
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
function isTransientStatus(status) {
  return status === 0 || status >= 520 && status <= 524 || status === 502 || status === 503 || status === 504;
}
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
async function fetchWithRetry(input, init, maxRetries = 3, baseDelayMs = 1000) {
  let lastErr;
  for (let attempt = 0;attempt <= maxRetries; attempt++) {
    try {
      const res = await fetchWithTimeout(input, init);
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
async function createTusSession(input, init) {
  const readText = async (r) => r.text().catch(() => "");
  const echo = (r, text) => new Response(text, { status: r.status, headers: r.headers });
  let res = await fetchWithTimeout(input, init);
  if (res.status === 400) {
    const text = await readText(res);
    if (!/already exists/i.test(text)) {
      return { res: echo(res, text), alreadyExists: false };
    }
    await new Promise((r) => setTimeout(r, 500));
    res = await fetchWithTimeout(input, init);
    if (res.status === 400) {
      const text2 = await readText(res);
      return {
        res: echo(res, text2),
        alreadyExists: /already exists/i.test(text2)
      };
    }
  }
  return { res, alreadyExists: false };
}
export function startUpload(item, file, controls) {
  if (readUploadMode() === "direct-prefer") {
    return startDirectTicketUpload(item, file, controls);
  }
  return file.size > TUS_THRESHOLD ? startTusUpload(item, file, controls) : startDirectUpload(item, file, controls);
}
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
  constructor(callback) {
    this.callback = callback;
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
        const projected = Math.min(this.loaded + this.speed * dt, this.loaded + this.speed * 2, this.total);
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
      ttl_hours: Number(item.ttlHours),
      host: item.customHost || undefined
    }),
    ...signal ? { signal } : {}
  });
  if (!reserveRes.ok)
    return null;
  const reserve = await reserveRes.json();
  return {
    id: reserve.id || "",
    url: reserve.url || "",
    deleteToken: reserve.delete_token || ""
  };
}
function startDirectUpload(item, file, controls) {
  const { update } = controls;
  const runState = { cancelled: false };
  let xhr = null;
  const run = (async () => {
    let reserveId = "";
    let reserveUrl = "";
    let reserveDeleteToken = "";
    if (item.quickLink) {
      try {
        const reserve = await reserveQuickLink(item);
        if (reserve) {
          reserveId = reserve.id;
          reserveUrl = reserve.url;
          reserveDeleteToken = reserve.deleteToken;
          if (reserveUrl)
            update({ id: item.id, reserveUrl, serverId: reserveId });
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
    fd.append("file", uploadFile);
    const progress = new LiveProgress((pct) => {
      update({ id: item.id, progress: Math.min(pct, 99.4) });
    });
    progress.begin(uploadFile.size);
    await new Promise((resolve) => {
      const finish = () => {
        progress.destroy();
        resolve();
      };
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
        if (xhr.status >= 200 && xhr.status < 300) {
          let res = {};
          try {
            res = JSON.parse(xhr.responseText);
          } catch {}
          if (reserveUrl && !res.url)
            res.url = reserveUrl;
          if (reserveDeleteToken && !res.delete_token)
            res.delete_token = reserveDeleteToken;
          update({
            id: item.id,
            serverId: res.id || reserveId || "",
            url: res.url || "",
            deleteToken: res.delete_token || "",
            expiresAt: res.expires_at ? Number(res.expires_at) : Math.round(Date.now() / 1000) + item.ttlHours * 3600,
            state: "done",
            progress: 100
          });
        } else {
          let code;
          let msg;
          try {
            const err = JSON.parse(xhr.responseText);
            if (err.error)
              code = err.error;
            if (err.message)
              msg = err.message;
          } catch {}
          if (xhr.status >= 520 && xhr.status <= 524) {
            code = code || "UPLOAD_TIMEOUT";
            msg = msg || "Connection timed out (Cloudflare). Try a smaller file or try again later.";
          }
          update({ id: item.id, errorCode: code, errorMessage: msg, state: "error" });
        }
        finish();
      };
      xhr.onerror = () => {
        update({ id: item.id, errorCode: "NETWORK_ERROR", state: "error" });
        finish();
      };
      xhr.ontimeout = () => {
        update({ id: item.id, errorCode: "UPLOAD_TIMEOUT", errorMessage: "Upload timed out. Try a smaller file.", state: "error" });
        finish();
      };
      xhr.onabort = () => {
        update({ id: item.id, state: "cancelled" });
        finish();
      };
      xhr.send(fd);
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
      xhr?.abort();
    }
  };
}
async function gzipBlob(blob) {
  return new Response(blob.stream().pipeThrough(new CompressionStream("gzip"))).blob();
}
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
function startDirectTicketUpload(item, file, controls) {
  const { update } = controls;
  const runState = { cancelled: false };
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
          ttl_hours: Number(item.ttlHours)
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
      if (!ticket || !uploadUrl || !fileId) {
        update({ id: item.id, errorCode: "RESERVE_FAILED", state: "error" });
        return;
      }
      if (shareUrl)
        update({ id: item.id, reserveUrl: shareUrl, serverId: fileId });
      update({ id: item.id, state: "uploading", progress: 0 });
      const progress = new LiveProgress((pct) => {
        update({ id: item.id, progress: Math.min(pct, 99.4) });
      });
      progress.begin(file.size);
      let uploadStatus;
      try {
        uploadStatus = await xhrSendBytes(uploadUrl, file, ticket, (loaded) => progress.set(loaded), runState, xhrRef);
      } catch (err) {
        progress.destroy();
        if (runState.cancelled)
          throw err;
        update({ id: item.id, errorCode: "NETWORK_ERROR", state: "error" });
        return;
      }
      progress.destroy();
      if (runState.cancelled)
        throw new Error("Upload cancelled");
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
        deleteToken: data.delete_token || "",
        expiresAt: data.expires_at ? Number(data.expires_at) : Math.round(Date.now() / 1000) + item.ttlHours * 3600,
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
async function uploadTusPart(item, file, sessionId, partIndex, totalParts, signal, hooks, reserveReady, getReserveId, controls, netTier, probeShared) {
  const partSize = Math.ceil(file.size / totalParts);
  const start = partIndex * partSize;
  const end = Math.min(start + partSize, file.size);
  const partFile = file.slice(start, end);
  const knownText = gzipEligible(item.filename, item.mimeType);
  const knownCompressed = !knownText && isKnownCompressed(item.filename, item.mimeType);
  let probeDone = knownText || knownCompressed || probeShared.done;
  let useGzip = knownText || probeShared.done && probeShared.use;
  const meta = {
    filename: item.filename,
    mimetype: item.mimeType || "application/octet-stream",
    ttl: String(item.ttlHours),
    session_id: sessionId,
    part_index: String(partIndex),
    total_parts: String(totalParts)
  };
  if (item.customHost)
    meta.host = item.customHost;
  meta.upload_mode = item.uploadMode;
  const metadata = encodeTusMeta(meta);
  const runSession = async () => {
    const partCtrl = new AbortController;
    const partSignal = partCtrl.signal;
    const relayCancel = () => partCtrl.abort();
    signal.addEventListener("abort", relayCancel);
    let inflight = new Map;
    try {
      const created = await createTusSession(`${UPLOAD_URL}/api/tus`, {
        method: "POST",
        headers: {
          "Upload-Length": String(partFile.size),
          "Upload-Metadata": metadata,
          "Tus-Resumable": "1.0.0"
        },
        signal
      });
      if (created.alreadyExists) {
        return { status: 202 };
      }
      if (!created.res.ok)
        throw new Error(`TUS create failed: ${created.res.status}`);
      const location = created.res.headers.get("Location");
      if (!location)
        throw new Error("No TUS location");
      const uploadUrl = `${UPLOAD_URL}${location}`;
      const tusIdMatch = location.match(/\/api\/tus\/(.+)$/);
      if (tusIdMatch)
        controls.onTusCreate(tusIdMatch[1]);
      const patchChunk = (body, chunkOffset) => new Promise((resolve, reject) => {
        let last = 0;
        let idleTimer = null;
        let idleFired = false;
        const armIdle = () => {
          clearTimeout(idleTimer);
          idleTimer = setTimeout(() => {
            idleFired = true;
            try {
              x.abort();
            } catch {}
          }, 30000);
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
            const offsetRace = x.status === 409 && tries < 3;
            if (x.status === 409 && tries >= 2 && tries < 4 && !partSignal.aborted) {
              discoverOffset().then((serverOff) => {
                if (serverOff !== null && serverOff > chunkOffset) {
                  resolve({ status: 204, newOffset: serverOff, text: "" });
                } else if (!partSignal.aborted) {
                  setTimeout(() => attempt(tries + 1), 500);
                } else {
                  reject(new Error("Upload cancelled"));
                }
              });
              return;
            }
            if (transient && tries < 3 || offsetRace) {
              if (!partSignal.aborted) {
                setTimeout(() => attempt(tries + 1), (offsetRace ? 150 : 500) * 2 ** tries + Math.random() * 250);
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
      const noteAcked = (bytes) => {
        hooks.onAcked(bytes);
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
      let offset = 0;
      while (offset < partFile.size || inflight.size > 0) {
        if (signal.aborted)
          throw new Error("Upload cancelled");
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
            return patchChunk(b, at);
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
          return { status: 200, data };
        } else {
          throw new Error(`TUS patch failed: ${r.status}`);
        }
      }
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
      });
      if (finalRes.ok) {
        const data = await finalRes.json();
        return { status: 200, data };
      }
      return { status: finalRes.status };
    } finally {
      signal.removeEventListener("abort", relayCancel);
      partCtrl.abort();
      for (const s of inflight.values())
        s.p.catch(() => {});
    }
  };
  let lastError;
  for (let attemptNo = 0;attemptNo < 3; attemptNo++) {
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
function startTusUpload(item, file, controls) {
  const { update } = controls;
  const controller = new AbortController;
  const signal = controller.signal;
  const run = (async () => {
    const sessionId = generateId();
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
    const wantedStreams = streamsForRate(hintBps || TIER_DEFAULT_BPS[tier]);
    const numParts = Math.max(1, Math.min(TUS_MAX_PARTS, PARALLEL_STREAMS * 3, Math.floor(file.size / TUS_MIN_PART)));
    let targetWorkers = Math.min(wantedStreams, numParts);
    const uploadStart = performance.now();
    const progress = new LiveProgress((pct) => {
      update({ id: item.id, progress: Math.min(pct, 99.4) });
    });
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
    update({ id: item.id, state: "uploading", progress: 0 });
    let reserveId = "";
    let reserveUrl = "";
    let reserveDeleteToken = "";
    let reserveReady = Promise.resolve();
    if (item.quickLink) {
      reserveReady = (async () => {
        try {
          const reserve = await reserveQuickLink(item, signal);
          if (reserve) {
            reserveId = reserve.id;
            reserveUrl = reserve.url;
            reserveDeleteToken = reserve.deleteToken;
            if (reserveUrl)
              update({ id: item.id, reserveUrl, serverId: reserveId });
          }
        } catch {}
      })();
    }
    const getReserveId = () => reserveId;
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
                progress.set(sum(partSent));
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
              rateHint: () => rateNow
            }, reserveReady, getReserveId, controls, tier, probeState).catch((err) => {
              if (signal.aborted)
                return { status: 0 };
              throw err;
            });
          } catch (err) {
            if (signal.aborted)
              return;
            if (runCounts[i] < MAX_PART_RUNS) {
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
    const initialWorkers = Math.max(1, Math.min(targetWorkers, numParts, INITIAL_WORKERS));
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
    const tuner = setInterval(() => {
      const rate = windowBytes * 1000 / TUNER_INTERVAL_MS;
      windowBytes = 0;
      rateEwma = rateEwma ? rateEwma * 0.6 + rate * 0.4 : rate;
      rateNow = rateEwma;
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
    const merged = settled.find((r) => r.status === 200 && r.data?.url);
    if (merged && "data" in merged && merged.data) {
      const data = merged.data;
      if (reserveUrl && !data.url)
        data.url = reserveUrl;
      if (reserveDeleteToken && !data.delete_token)
        data.delete_token = reserveDeleteToken;
      update({ id: item.id, state: "finalizing" });
      update({
        id: item.id,
        serverId: data.id || reserveId || "",
        url: data.url,
        deleteToken: data.delete_token || "",
        expiresAt: data.expires_at ? Number(data.expires_at) : Math.round(Date.now() / 1000) + item.ttlHours * 3600,
        state: "done",
        progress: 100
      });
      return;
    }
    update({
      id: item.id,
      errorCode: "UPLOAD_FAILED",
      errorMessage: "Parallel upload completed but no merge response (part statuses: " + [...new Set(settled.map((r) => r && r.status))].join(",") + ")",
      state: "error"
    });
  })().catch((err) => {
    if (signal.aborted) {
      update({ id: item.id, state: "cancelled" });
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
