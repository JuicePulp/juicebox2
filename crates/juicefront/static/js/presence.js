import { apiPresence } from "./api.js";
let eventSource = null;
let connected = false;
let devices = [];
const INITIAL_RETRY_MS = 5000;
const MAX_RETRY_MS = 60000;
const SUSTAINED_RETRY_MS = 300000;
let retryDelay = INITIAL_RETRY_MS;
let consecutiveFailures = 0;
let retryTimer = null;
let visibilityWired = false;
export function isAppMode() {
  return connected && devices.length > 0;
}
export function shouldGlowHost() {
  if (!isAppMode())
    return false;
  const cleared = localStorage.getItem("juicebox_host_glow_cleared");
  if (cleared === "true")
    return false;
  return true;
}
export function clearHostGlow() {
  localStorage.setItem("juicebox_host_glow_cleared", "true");
  document.querySelector("[data-host-btn]")?.classList.remove("host-btn--glow");
}
export function onDeviceConnected() {
  window.__juiceboxAppMode = true;
  if (shouldGlowHost()) {
    document.querySelector("[data-host-btn]")?.classList.add("host-btn--glow");
  }
  window.dispatchEvent(new CustomEvent("juicebox-app-mode", { detail: { connected: true, devices: [...devices] } }));
}
export function onDeviceDisconnected() {
  window.__juiceboxAppMode = devices.length > 0;
  window.dispatchEvent(new CustomEvent("juicebox-app-mode", { detail: { connected: isAppMode(), devices: [...devices] } }));
}
export function connectPresence() {
  if (eventSource || typeof window === "undefined")
    return;
  if (typeof document !== "undefined" && document.hidden)
    return;
  wireVisibilityHandler();
  eventSource = new EventSource(apiPresence);
  eventSource.onopen = () => {
    retryDelay = INITIAL_RETRY_MS;
    consecutiveFailures = 0;
  };
  eventSource.onmessage = (event) => {
    try {
      const data = JSON.parse(event.data);
      switch (data.type) {
        case "device_list":
          devices = data.devices || [];
          connected = devices.length > 0;
          window.__juiceboxAppMode = connected;
          if (connected)
            onDeviceConnected();
          else
            onDeviceDisconnected();
          break;
        case "device_connected":
          devices.push({ device_id: data.device_id, device_name: data.device_name });
          connected = true;
          onDeviceConnected();
          break;
        case "device_disconnected":
          devices = devices.filter((d) => d.device_id !== data.device_id);
          connected = devices.length > 0;
          onDeviceDisconnected();
          break;
        case "ping_response":
          window.dispatchEvent(new CustomEvent("juicebox-ping-response", { detail: { device_id: data.device_id } }));
          break;
      }
    } catch {}
  };
  eventSource.onerror = () => {
    connected = false;
    try {
      eventSource?.close();
    } catch {}
    eventSource = null;
    if (retryTimer)
      clearTimeout(retryTimer);
    consecutiveFailures++;
    const delay = consecutiveFailures >= 5 ? SUSTAINED_RETRY_MS : retryDelay;
    retryDelay = Math.min(retryDelay * 2, MAX_RETRY_MS);
    retryTimer = setTimeout(() => {
      retryTimer = null;
      connectPresence();
    }, delay);
  };
}
function wireVisibilityHandler() {
  if (visibilityWired || typeof document === "undefined")
    return;
  visibilityWired = true;
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) {
      if (retryTimer) {
        clearTimeout(retryTimer);
        retryTimer = null;
      }
      if (eventSource) {
        try {
          eventSource.close();
        } catch {}
        eventSource = null;
      }
    } else {
      retryDelay = INITIAL_RETRY_MS;
      consecutiveFailures = 0;
      connectPresence();
    }
  });
}
