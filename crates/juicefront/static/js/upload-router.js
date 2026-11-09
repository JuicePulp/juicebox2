
const ENDPOINT = "/router/upload-target";
// The home host is the default route; callers treat it as "use the current origin".
const HOME_HOST = "https://f.juicey.dev";
const SUCCESS_TTL_MS = 60_000;
const FAILURE_TTL_MS = 15_000;
const TIMEOUT_MS = 1_200;

let cache = { host: "", at: 0, ttl: FAILURE_TTL_MS };
let pending = null;

export function clearUploadHostCache() {
  cache = { host: "", at: 0, ttl: FAILURE_TTL_MS };
}

/** Returns a host origin, or `""` for the current origin; concurrent callers share one request. */
export async function resolveUploadHost() {
  if (Date.now() - cache.at < cache.ttl) {
    return cache.host;
  }
  if (pending) {
    return pending;
  }
  pending = (async () => {
    let host = "";
    let ttl = FAILURE_TTL_MS;
    try {
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), TIMEOUT_MS);
      let response;
      try {
        response = await fetch(ENDPOINT, {
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
    cache = { host, at: Date.now(), ttl };
    pending = null;
    return host;
  })();
  return pending;
}
