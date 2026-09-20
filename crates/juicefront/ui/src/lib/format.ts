/**
 * Shared utility functions for formatting, icons, and UI components.
 * Used by both SSR (Astro frontmatter) and client-side scripts.
 */

import { iconSvgHtml } from "./icons";

let _announcer: HTMLElement | null = null;

/** Announce a message to screen readers via a live region. */
export function announce(message: string): void {
  if (typeof document === "undefined") return;
  if (!_announcer) {
    _announcer = document.createElement("div");
    _announcer.setAttribute("role", "status");
    _announcer.setAttribute("aria-live", "polite");
    _announcer.setAttribute("aria-atomic", "true");
    _announcer.style.cssText =
      "position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0";
    document.body.appendChild(_announcer);
  }
  _announcer.textContent = message;
}

/** Format bytes into a human-readable string (e.g. "1.5 MB"). */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  );
  const v = bytes / Math.pow(1024, i);
  return `${v >= 10 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

/** Format a max upload size (bytes) as whole MB/GB (e.g. "500 MB", "5 GB"). */
export function formatMaxSize(bytes: number): string {
  const mb = bytes / (1024 * 1024);
  if (mb >= 1024) {
    const gb = mb / 1024;
    return gb % 1 === 0 ? `${gb} GB` : `${gb.toFixed(1)} GB`;
  }
  return mb % 1 === 0 ? `${mb} MB` : `${mb.toFixed(1)} MB`;
}

/** Map a MIME type to an icon name. */
export function iconForMime(mime: string): string {
  if (mime.startsWith("image/")) return "image";
  if (mime.startsWith("video/")) return "video";
  if (mime.startsWith("audio/")) return "audio-waveform";
  if (/spreadsheet|csv|excel/.test(mime)) return "presentation";
  if (/archive|zip|gzip|rar|7z|tar|bzip|xz|compress|zstd/.test(mime))
    return "archive";
  if (/wordprocessingml|presentationml/.test(mime)) return "file-text";
  if (/json|xml|javascript|ecmascript|typescript|html|css/.test(mime))
    return "code";
  return "file-text";
}

export function iconHTML(name: string, size = 18): string {
  return iconSvgHtml(name, size);
}

/** Percentage of TTL remaining (0-100). */
export function pctRemaining(expiresAt: number, uploadedAt: number): number {
  const total = (expiresAt - uploadedAt) * 1000;
  if (total <= 0) return 0;
  const left = expiresAt * 1000 - Date.now();
  return Math.max(0, Math.min(100, (left / total) * 100));
}

const DEFAULT_HOSTS = new Set([
  "localhost:6402",
  "127.0.0.1:6402",
  "localhost:6400",
  "127.0.0.1:6400",
]);

function stripHost(host: string): string {
  return host.replace(/^https?:\/\//, "").replace(/\/$/, "");
}

/** Whether a storage host is one of this instance's default hosts. */
export function isDefaultHost(host: string, defaultHost?: string): boolean {
  if (!host) return true;
  const stripped = stripHost(host);
  if (DEFAULT_HOSTS.has(stripped)) return true;
  if (defaultHost) return stripped === stripHost(defaultHost);
  return false;
}

/** Localized strings for the copy-to-clipboard button. */
export interface CopyBarStrings {
  title: string;
  aria: string;
  copied: string;
  announceMsg: string;
}

/** Create a copy-to-clipboard button for a URL. */
export function makeCopyBar(url: string, strings?: CopyBarStrings): HTMLButtonElement {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "copy-bar";
  btn.title = strings?.title ?? "Click to copy link";
  btn.setAttribute("aria-label", strings?.aria ?? "Copy link to clipboard");
  btn.innerHTML =
    `<div class="copy-bar__text-wrapper"><span class="copy-bar__copied-text">${strings?.copied ?? "Copied!"}</span><span class="copy-bar__url"></span></div>` +
    iconSvgHtml("copy", 24, "copy-bar__copy-icon");
  const urlEl = btn.querySelector(".copy-bar__url");
  if (urlEl) urlEl.textContent = url;
  btn.addEventListener("click", async () => {
    try {
      const currentUrl = urlEl?.textContent?.trim() || url;
      await navigator.clipboard.writeText(currentUrl);
      btn.classList.add("copy-bar--copied");
      announce(strings?.announceMsg ?? "Link copied to clipboard");
      setTimeout(() => btn.classList.remove("copy-bar--copied"), 2000);
    } catch {}
  });
  return btn;
}
