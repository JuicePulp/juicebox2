import { defineMiddleware } from "astro/middleware";

const API_INTERNAL = process.env.API_INTERNAL_URL || "http://127.0.0.1:6401";
const JUICEBACK_URL = process.env.JUICEBACK_URL || "http://127.0.0.1:6401";
const JUICEHOST_URL = process.env.JUICEHOST_URL || "http://127.0.0.1:6402";

// Hop-by-hop headers that must not be forwarded through a proxy.
const HOP_BY_HOP = new Set([
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "te",
  "trailers",
  "transfer-encoding",
  "upgrade",
  "host",
  "content-length",
]);

// Proxies whose X-Forwarded-For we believe, beyond loopback.
// Same format as the backends' TRUSTED_PROXY_CIDRS (comma-separated IPs/CIDRs).
const TRUSTED_PROXY_CIDRS = (process.env.TRUSTED_PROXY_CIDRS || "")
  .split(",")
  .map((s) => s.trim())
  .filter(Boolean);

function ipv4ToU32(ip: string): number | null {
  const parts = ip.split(".");
  if (parts.length !== 4) return null;
  let out = 0;
  for (const part of parts) {
    if (!/^\d+$/.test(part)) return null;
    const n = Number(part);
    if (n < 0 || n > 255) return null;
    out = out * 256 + n;
  }
  return out >>> 0;
}

function normalizePeer(peer: string): string {
  // Node reports IPv4 peers as ::ffff:a.b.c.d on dual-stack sockets.
  return peer.startsWith("::ffff:") ? peer.slice("::ffff:".length) : peer;
}

/** True when `peer` may supply X-Forwarded-For (loopback always trusted). */
function isTrustedProxy(peer: string): boolean {
  const ip = normalizePeer(peer);
  if (ip === "127.0.0.1" || ip === "::1" || ip === "localhost") return true;
  const addr = ipv4ToU32(ip);
  if (addr === null) return false;
  for (const cidr of TRUSTED_PROXY_CIDRS) {
    const slash = cidr.indexOf("/");
    if (slash === -1) {
      if (ipv4ToU32(normalizePeer(cidr)) === addr) return true;
      continue;
    }
    const base = ipv4ToU32(normalizePeer(cidr.slice(0, slash)));
    const bits = Number(cidr.slice(slash + 1));
    if (base === null || !Number.isInteger(bits) || bits < 0 || bits > 32)
      continue;
    const mask = bits === 0 ? 0 : (0xffffffff << (32 - bits)) >>> 0;
    if ((addr & mask) === (base & mask)) return true;
  }
  return false;
}

/** Map a request path to the upstream service it should hit, or null for frontend routes. */
function proxyTarget(pathname: string): string | null {
  if (pathname.startsWith("/api/")) return JUICEBACK_URL;
  if (pathname === "/upload" || pathname.startsWith("/upload/")) return JUICEBACK_URL;
  if (pathname.startsWith("/file/")) return JUICEBACK_URL;
  if (pathname.startsWith("/f/")) return JUICEHOST_URL;
  return null;
}

function cleanHeaders(headers: Headers): Headers {
  const out = new Headers();
  for (const [key, value] of headers) {
    const lower = key.toLowerCase();
    if (HOP_BY_HOP.has(lower)) continue;
    if (lower === "set-cookie") continue;
    out.append(key, value);
  }
  // Set-Cookie must be copied per-value so multiple cookies survive.
  for (const cookie of headers.getSetCookie()) {
    out.append("Set-Cookie", cookie);
  }
  return out;
}

export const onRequest = defineMiddleware(async (context, next) => {
  // Pages read JUICEBACK_URL at render time; give it a local default so
  // server-side fetches work even when the var isn't set in the shell.
  process.env.JUICEBACK_URL ||= JUICEBACK_URL;

  const { request, redirect } = context;
  const url = new URL(request.url);

  if (url.pathname.startsWith("/admin/") || url.pathname === "/admin") {
    if (url.pathname.endsWith("/login")) {
      return next();
    }

    try {
      const res = await fetch(`${API_INTERNAL}/api/admin/check`, {
        headers: { cookie: request.headers.get("cookie") || "" },
      });
      if (!res.ok) {
        return redirect("/admin/login");
      }
    } catch {
      return redirect("/admin/login");
    }
  }

  const target = proxyTarget(url.pathname);
  if (!target) {
    // Clickjacking protection must be a real header: CSP frame-ancestors
    // is ignored when delivered via a <meta> element.
    const res = await next();
    res.headers.set("X-Frame-Options", "DENY");
    return res;
  }

  const headers = cleanHeaders(request.headers);
  headers.set("x-forwarded-host", url.host);
  headers.set("x-forwarded-proto", url.protocol.slice(0, -1));
  // Never blindly forward client-supplied X-Forwarded-For: only a trusted
  // proxy (loopback, TRUSTED_PROXY_CIDRS) may vouch for it. Otherwise stamp
  // the direct peer address so downstream IP decisions can't be spoofed.
  headers.delete("x-forwarded-for");
  const peer = context.clientAddress || "";
  const incomingXff = request.headers.get("x-forwarded-for");
  if (incomingXff && peer && isTrustedProxy(peer)) {
    headers.set("x-forwarded-for", incomingXff);
  } else if (peer) {
    headers.set("x-forwarded-for", peer);
  }

  const init: RequestInit = {
    method: request.method,
    headers,
    redirect: "manual",
  };
  if (request.method !== "GET" && request.method !== "HEAD" && request.body) {
    init.body = request.body;
    // @ts-expect-error -- required for stream bodies in undici/node fetch
    init.duplex = "half";
  }

  try {
    const upstream = await fetch(target + url.pathname + url.search, init);
    return new Response(upstream.body, {
      status: upstream.status,
      headers: cleanHeaders(upstream.headers),
    });
  } catch {
    return new Response("Proxy error: upstream unavailable", {
      status: 502,
      headers: { "Content-Type": "text/plain" },
    });
  }
});
