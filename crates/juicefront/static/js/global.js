import { initRipple } from "./ripple.js";
import { initHaptics } from "./haptics.js";
import { connectPresence } from "./presence.js";
import "./netdebug.js";

function setOffline(offline) {
  if (offline) {
    document.documentElement.setAttribute("data-backend-offline", "");
  } else {
    document.documentElement.removeAttribute("data-backend-offline");
  }
}

async function checkHealth() {
  if (document.hidden) return;
  try {
    var res = await fetch("/api/health");
    if (!res.ok) throw new Error("not ok");
    var data = await res.json();
    setOffline(data.status !== "ok");
  } catch {
    setOffline(true);
  }
}

function applyFlags() {
  var el = document.getElementById("page-flags");
  if (!el) return;
  if (el.hasAttribute("data-backend-offline")) {
    document.documentElement.setAttribute("data-backend-offline", "");
  }
  if (el.hasAttribute("data-banned")) {
    document.documentElement.setAttribute("data-banned", "");
  }
}

function boot() {
  applyFlags();
  try {
    initRipple();
  } catch {}
  try {
    initHaptics();
  } catch {}
  try {
    connectPresence();
  } catch {}
  checkHealth();
  setInterval(checkHealth, 300000);
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", boot);
} else {
  boot();
}
