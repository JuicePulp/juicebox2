/**
 * SSE client for device presence.
 *
 * Connects to /api/presence and tracks connected devices.
 * Dispatches custom events so other parts of the UI can react.
 */

import { apiPresence } from "./api";

declare global {
  interface Window {
    __juiceboxAppMode: boolean;
  }
}

let eventSource: EventSource | null = null;
let connected = false;
let devices: Array<{ device_id: string; device_name: string }> = [];

// Reconnect state: exponential backoff so a dead backend doesn't get
// hammered (and doesn't flood the dev proxy log with ECONNREFUSED).
const INITIAL_RETRY_MS = 5000;
const MAX_RETRY_MS = 60000;
let retryDelay = INITIAL_RETRY_MS;
let retryTimer: ReturnType<typeof setTimeout> | null = null;
let visibilityWired = false;

/** Whether juicebox-plus is currently connected. */
export function isAppMode(): boolean {
  return connected && devices.length > 0;
}

/** Check if glow should be shown (device connected since last clear). */
export function shouldGlowHost(): boolean {
  if (!isAppMode()) return false;
  const cleared = localStorage.getItem("juicebox_host_glow_cleared");
  if (cleared === "true") return false;
  return true;
}

/** Mark the host glow as cleared (called when user opens host settings). */
export function clearHostGlow(): void {
  localStorage.setItem("juicebox_host_glow_cleared", "true");
  document.querySelector("[data-host-btn]")?.classList.remove("host-btn--glow");
}

/** Mark that a new device has connected. */
export function onDeviceConnected(): void {
  window.__juiceboxAppMode = true;
  if (shouldGlowHost()) {
    document.querySelector("[data-host-btn]")?.classList.add("host-btn--glow");
  }
  window.dispatchEvent(new CustomEvent("juicebox-app-mode", { detail: { connected: true, devices: [...devices] } }));
}

/** Mark that devices have disconnected. */
export function onDeviceDisconnected(): void {
  window.__juiceboxAppMode = devices.length > 0;
  window.dispatchEvent(new CustomEvent("juicebox-app-mode", { detail: { connected: isAppMode(), devices: [...devices] } }));
}

/** Connect to the SSE presence stream. */
export function connectPresence(): void {
  if (eventSource || typeof window === "undefined") return;
  // Don't poll in background tabs; the visibility handler reconnects.
  if (typeof document !== "undefined" && document.hidden) return;

  wireVisibilityHandler();
  eventSource = new EventSource(apiPresence);

  eventSource.onopen = () => {
    retryDelay = INITIAL_RETRY_MS;
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
    // Close the dead source: otherwise its native retry loop keeps firing
    // alongside our scheduled reconnect (duplicate streams + log spam).
    try {
      eventSource?.close();
    } catch {}
    eventSource = null;
    if (retryTimer) clearTimeout(retryTimer);
    const delay = retryDelay;
    retryDelay = Math.min(retryDelay * 2, MAX_RETRY_MS);
    retryTimer = setTimeout(() => {
      retryTimer = null;
      connectPresence();
    }, delay);
  };
}

/** Pause polling while the tab is hidden; resume on return. */
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
      connectPresence();
    }
  });
}
