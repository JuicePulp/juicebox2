/** HTML-escapes any value; `null` and `undefined` become an empty string. */
export function esc(s) {
  var d = document.createElement("div");
  d.appendChild(document.createTextNode(s == null ? "" : String(s)));
  return d.innerHTML;
}

var announcer = null;
export function announce(message) {
  if (typeof document === "undefined") return;
  if (!announcer) {
    announcer = document.createElement("div");
    announcer.setAttribute("role", "status");
    announcer.setAttribute("aria-live", "polite");
    announcer.setAttribute("aria-atomic", "true");
    announcer.style.cssText =
      "position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0";
    document.body.appendChild(announcer);
  }
  announcer.textContent = message;
}

/** Queue states that will never change again. */
export function isTerminal(state) {
  return state === "done" || state === "error" || state === "cancelled";
}

export function hasCookie(name) {
  try {
    return document.cookie.split("; ").some((c) => c.startsWith(name + "="));
  } catch {
    return false;
  }
}

export function readCookie(name) {
  try {
    const match = document.cookie.split("; ").find((c) => c.startsWith(name + "="));
    return match ? decodeURIComponent(match.split("=")[1] || "") : "";
  } catch {
    return "";
  }
}

export function writeCookie(name, value, maxAgeSecs) {
  try {
    document.cookie = `${name}=${encodeURIComponent(value)}; path=/; max-age=${maxAgeSecs}; SameSite=Lax`;
  } catch {}
}

export function formatSize(bytes) {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  var units = ["B", "KB", "MB", "GB", "TB"];
  var i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  var v = bytes / Math.pow(1024, i);
  return (v >= 10 || i === 0 ? Math.round(v) : v.toFixed(1)) + " " + units[i];
}

export function formatMaxSize(bytes) {
  var mb = bytes / (1024 * 1024);
  if (mb >= 1024) {
    var gb = mb / 1024;
    return (gb % 1 === 0 ? gb : gb.toFixed(1)) + " GB";
  }
  return (mb % 1 === 0 ? mb : mb.toFixed(1)) + " MB";
}
