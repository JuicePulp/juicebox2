const ENDPOINT = "/router/upload-target";
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
    try {
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), TIMEOUT_MS);
      let response;
      try {
        const params = new URLSearchParams();
        const size = Number(sizeBytes);
        const hours = Number(ttlHours);
        if (Number.isFinite(size) && size > 0) params.set("size", String(Math.ceil(size)));
        if (Number.isFinite(hours) && hours > 0) params.set("ttl_hours", String(hours));
        const url = params.toString() ? `${ENDPOINT}?${params}` : ENDPOINT;
        response = await fetch(url, {
          signal: controller.signal,
          credentials: "same-origin",
          headers: { accept: "application/json" },
        });
      } finally {
        clearTimeout(timer);
      }
      if (response.ok) {
        const data = await response.json();
        const url = typeof data.url === "string" ? data.url.replace(/\/+$/, "") : "";
        if (url && url !== HOME_HOST) {
          host = url;
        }
        ttl = SUCCESS_TTL_MS;
      }
    } catch {
    }
    cache.set(key, { host, at: Date.now(), ttl });
    pending.delete(key);
    return host;
  })();
  pending.set(key, request);
  return request;
}
