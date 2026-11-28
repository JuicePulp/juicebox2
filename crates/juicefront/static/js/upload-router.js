const ENDPOINT = "/router/upload-target";
const HOSTS_ENDPOINT = "/router/hosts";
// The home host is the default route; callers treat it as "use the current origin".
const HOME_HOST = "https://f.juicey.dev";
const SUCCESS_TTL_MS = 60_000;
const FAILURE_TTL_MS = 15_000;
const TIMEOUT_MS = 1_200;

let cache = new Map();
let pending = new Map();

function cacheKey(sizeBytes, ttlHours) {
  const size = Number(sizeBytes);
  const ttl = Number(ttlHours);
  if (!Number.isFinite(size) || size <= 0) return "";
  return `${size}:${Number.isFinite(ttl) && ttl > 0 ? ttl : ""}`;
}

export function clearUploadHostCache() {
  cache = new Map();
  pending = new Map();
}

async function fetchJson(url) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), TIMEOUT_MS);
  try {
    const response = await fetch(url, {
      signal: controller.signal,
      credentials: "same-origin",
      headers: { accept: "application/json" },
    });
    if (!response.ok) return null;
    return await response.json();
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

function ttlKey(hours) {
  return Number.isFinite(hours) && hours > 0 ? String(hours) : "";
}

/** If the targeted lookup fails, pick any healthy node that can actually hold the upload. */
async function fallbackHost(sizeBytes, ttlHours) {
  const size = Number(sizeBytes);
  if (!Number.isFinite(size) || size <= 0) return "";
  const nodes = await fetchJson(HOSTS_ENDPOINT);
  if (!Array.isArray(nodes)) return "";

  const key = ttlKey(Number(ttlHours));
  const candidates = nodes.filter(function (node) {
    if (!node || node.health === undefined || node.health.state !== "healthy") return false;
    if (node.region === "home") return false;
    const url = typeof node.url === "string" ? node.url.replace(/\/+$/, "") : "";
    if (!url || url === HOME_HOST) return false;

    const capacity = node.capacity;
    if (!capacity) return false;
    const usable = Number(capacity.usable_bytes) || 0;
    // Multipart concat needs room for the parts plus the assembled target.
    if (usable < size * 2) return false;

    const max = Number(capacity.max_file_size_bytes);
    if (Number.isFinite(max) && max > 0 && size > max) return false;
    const caps = capacity.max_file_size_by_ttl_hours || {};
    if (key && Object.prototype.hasOwnProperty.call(caps, key)) {
      const cap = Number(caps[key]);
      if (Number.isFinite(cap) && cap > 0 && size > cap) return false;
    }
    return true;
  });

  candidates.sort(function (a, b) {
    const la = Number(a.health.latency_ms);
    const lb = Number(b.health.latency_ms);
    return (Number.isFinite(la) ? la : Infinity) - (Number.isFinite(lb) ? lb : Infinity);
  });
  return candidates.length ? candidates[0].url.replace(/\/+$/, "") : "";
}

/** Returns a host origin, or `""` for the current origin; concurrent callers with the same size share one request. */
export async function resolveUploadHost(sizeBytes, ttlHours) {
  const key = cacheKey(sizeBytes, ttlHours);
  const cached = cache.get(key);
  if (cached && Date.now() - cached.at < cached.ttl) {
    return cached.host;
  }
  const inFlight = pending.get(key);
  if (inFlight) {
    return inFlight;
  }
  const request = (async () => {
    let host = "";
    let ttl = FAILURE_TTL_MS;

    const params = new URLSearchParams();
    const size = Number(sizeBytes);
    const hours = Number(ttlHours);
    if (Number.isFinite(size) && size > 0) params.set("size", String(Math.ceil(size)));
    if (Number.isFinite(hours) && hours > 0) params.set("ttl_hours", String(hours));
    const url = params.toString() ? `${ENDPOINT}?${params}` : ENDPOINT;
    const data = await fetchJson(url);
    if (data && typeof data.url === "string") {
      const target = data.url.replace(/\/+$/, "");
      if (target && target !== HOME_HOST) {
        host = target;
        ttl = SUCCESS_TTL_MS;
      }
    }

    if (!host) {
      const fallback = await fallbackHost(sizeBytes, hours);
      if (fallback) {
        host = fallback;
        ttl = SUCCESS_TTL_MS;
      }
    }

    cache.set(key, { host, at: Date.now(), ttl });
    pending.delete(key);
    return host;
  })();
  pending.set(key, request);
  return request;
}
