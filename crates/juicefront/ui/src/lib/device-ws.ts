






import { apiPresence } from "./api";

declare global {
  interface Window {
    __juiceboxAppMode: boolean;
  }
}

let eventSource: EventSource | null = null;
let connected = false;
let devices: Array<{ device_id: string; device_name: string }> = [];





const INITIAL_RETRY_MS = 5000;
const MAX_RETRY_MS = 60000;
const SUSTAINED_RETRY_MS = 300000;
let retryDelay = INITIAL_RETRY_MS;
let consecutiveFailures = 0;
let retryTimer: ReturnType<typeof setTimeout> | null = null;
let visibilityWired = false;

function hasSessionCookie(): boolean {
  try {
    return document.cookie
      .split("; ")
      .some((c) => c.startsWith("jb_session="));
  } catch {
    return false;
  }
}


export function isAppMode(): boolean {
  return connected && devices.length > 0;
}


export function shouldGlowHost(): boolean {
  if (!isAppMode()) return false;
  const cleared = localStorage.getItem("juicebox_host_glow_cleared");
  if (cleared === "true") return false;
  return true;
}


export function clearHostGlow(): void {
  localStorage.setItem("juicebox_host_glow_cleared", "true");
  document.querySelector("[data-host-btn]")?.classList.remove("host-btn--glow");
}


export function onDeviceConnected(): void {
  window.__juiceboxAppMode = true;
  if (shouldGlowHost()) {
    document.querySelector("[data-host-btn]")?.classList.add("host-btn--glow");
  }
  window.dispatchEvent(new CustomEvent("juicebox-app-mode", { detail: { connected: true, devices: [...devices] } }));
}


export function onDeviceDisconnected(): void {
  window.__juiceboxAppMode = devices.length > 0;
  window.dispatchEvent(new CustomEvent("juicebox-app-mode", { detail: { connected: isAppMode(), devices: [...devices] } }));
}


export function connectPresence(): void {
  if (eventSource || typeof window === "undefined") return;
  
  if (typeof document !== "undefined" && document.hidden) return;
  
  if (!hasSessionCookie()) return;

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
          if (connected) onDeviceConnected();
          else onDeviceDisconnected();
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
    if (retryTimer) clearTimeout(retryTimer);
    
    
    
    consecutiveFailures++;
    const delay = consecutiveFailures >= 5 ? SUSTAINED_RETRY_MS : retryDelay;
    retryDelay = Math.min(retryDelay * 2, MAX_RETRY_MS);
    retryTimer = setTimeout(() => {
      retryTimer = null;
      connectPresence();
    }, delay);
  };
}


function wireVisibilityHandler(): void {
  if (visibilityWired || typeof document === "undefined") return;
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
