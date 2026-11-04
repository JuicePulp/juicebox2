// Ask juicerouter which storage node an upload should use.
//
// The browser talks to juicefront's same-origin proxy (`/router/upload-target`),
// which forwards Cloudflare's country/colo headers to juicerouter. When the
// router is unavailable or not configured, callers fall back to the server's
// configured default storage node by leaving the `host` field out of the upload.

const ENDPOINT = "/router/upload-target";
const HOME_HOST = "https://f.juicey.dev";
const SUCCESS_TTL_MS = 60_000;
const FAILURE_TTL_MS = 15_000;
const TIMEOUT_MS = 1_200;

let cache = { host: "", at: 0, ttl: FAILURE_TTL_MS };
let pending = null;

export function clearUploadHostCache() {
  cache = { host: "", at: 0, ttl: FAILURE_TTL_MS };
}

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
        // Never send the canonical host. Omitting `host` makes the backend
        // use its configured default storage node instead.
        if (url && url !== HOME_HOST) {
          host = url;
        }
        ttl = SUCCESS_TTL_MS;
      }
    } catch {
      // Router down or blocked: keep the default node.
    }
    cache = { host, at: Date.now(), ttl };
    pending = null;
    return host;
  })();
  return pending;
}
