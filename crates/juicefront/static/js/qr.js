import { iconHTML } from "./copy.js";
import { announce } from "./util.js";

async function qrFactory() {
  const mod = await import("./vendor/qrcode.mjs");
  return mod.default ?? mod.qrcode ?? globalThis.qrcode;
}

export const QR_CANVAS_SIZE = 512;
const LOGO_RATIO = 0.25;
const LOGO_SRC = "/static/assets/logo_big.png";

function qrContext(canvas) {
  try {
    const p3 = canvas.getContext("2d", { colorSpace: "display-p3" });
    if (p3) return p3;
  } catch {
  }
  return canvas.getContext("2d");
}

let wakeLock = null;
async function holdFullBrightness() {
  if (wakeLock) return;
  try {
    if ("wakeLock" in navigator && document.visibilityState === "visible") {
      wakeLock = await navigator.wakeLock.request("screen");
    }
  } catch {
    wakeLock = null;
  }
}
function releaseFullBrightness() {
  try {
    if (wakeLock) void wakeLock.release();
  } catch {
  }
  wakeLock = null;
}

export function compactQrPayload(url) {
  const trimmed = String(url || "").trim();
  try {
    const u = new URL(trimmed);
    const seg = u.pathname.split("/").pop() ?? "";
    const dot = seg.lastIndexOf(".");
    const path =
      dot > 0
        ? u.pathname.slice(0, u.pathname.length - seg.length) + seg.slice(0, dot)
        : u.pathname;
    return `${u.host}${path}${u.search}${u.hash}`;
  } catch {
    return trimmed
      .replace(/^https?:\/\//, "")
      .replace(/\.[A-Za-z0-9]{2,5}([?#]|$)/, "$1");
  }
}

export async function qrMatrix(text) {
  const qrcode = await qrFactory();
  const qr = qrcode(0, "H");
  qr.addData(text, "Byte");
  qr.make();
  return {
    count: qr.getModuleCount(),
    isDark: (row, col) => qr.isDark(row, col),
  };
}

function paintModules(ctx, matrix, size) {
  const quiet = 4;
  const total = matrix.count + quiet * 2;
  const cell = size / total;
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, size, size);
  ctx.fillStyle = "#000000";
  for (let r = 0; r < matrix.count; r++) {
    for (let c = 0; c < matrix.count; c++) {
      if (matrix.isDark(r, c)) {
        ctx.fillRect(
          Math.round((c + quiet) * cell),
          Math.round((r + quiet) * cell),
          Math.ceil(cell),
          Math.ceil(cell),
        );
      }
    }
  }
  return cell;
}

function paintLogo(ctx, size, logo) {
  const logoSize = Math.round(size * LOGO_RATIO);
  const x = Math.round((size - logoSize) / 2);
  const y = Math.round((size - logoSize) / 2);
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(logo, x, y, logoSize, logoSize);
}

let logoCache = null;
function loadLogo() {
  if (logoCache) return Promise.resolve(logoCache);
  return new Promise((resolve) => {
    const img = new Image();
    img.decoding = "async";
    const done = (ok) => {
      if (ok) {
        logoCache = img;
        resolve(img);
      } else {
        resolve(null);
      }
    };
    img.onload = () => done(true);
    img.onerror = () => done(false);
    img.src = LOGO_SRC;
    if (typeof img.decode === "function") {
      img.decode().catch(() => {});
    }
  });
}

export async function renderQrWithLogo(canvas, url, size) {
  size = size || QR_CANVAS_SIZE;
  const text = compactQrPayload(url);
  canvas.width = size;
  canvas.height = size;
  const ctx = qrContext(canvas);
  if (!ctx) return;
  ctx.imageSmoothingEnabled = false;
  paintModules(ctx, await qrMatrix(text), size);
  const logo = await loadLogo();
  if (!logo) return;
  paintLogo(ctx, size, logo);
}

export function qrFilenameForUrl(url) {
  try {
    const segment = new URL(url).pathname.split("/").pop() || "";
    const clean = segment.replace(/\.[A-Za-z0-9]+$/, "").replace(/[^A-Za-z0-9_-]+/g, "");
    if (clean) return `juicebox-${clean}.png`;
  } catch {
  }
  const parts = String(url || "").split("/");
  const last = parts.pop() || "";
  const fallback = (last.split(".")[0] || "").replace(/[^A-Za-z0-9_-]+/g, "");
  return fallback ? `juicebox-${fallback}.png` : "juicebox-qr.png";
}

export function downloadQrCanvas(canvas, url) {
  canvas.toBlob((blob) => {
    if (!blob) return;
    const objectUrl = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = objectUrl;
    a.download = qrFilenameForUrl(url);
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(objectUrl), 4000);
  }, "image/png");
}

let modalEl = null;
let currentUrl = "";
let lastFocused = null;

function ensureModal() {
  if (modalEl && modalEl.isConnected) return modalEl;
  const el = document.createElement("div");
  el.id = "qr-modal";
  el.className = "mdloverlay qr-modal";
  el.setAttribute("role", "dialog");
  el.setAttribute("aria-modal", "true");
  el.setAttribute("aria-labelledby", "qr-modal-title");
  el.hidden = true;
  el.innerHTML =
    '<div class="mdlcontent qr-modal__content">' +
    '<div class="mdlheader">' +
    '<h2 id="qr-modal-title" class="mdltitle">' +
    '<span class="mdltitle-icon" data-qr-icon></span>' +
    "<span data-qr-title></span>" +
    "</h2>" +
    '<button type="button" class="mdlclose" data-qr-close>' +
    "<span data-qr-close-icon></span>" +
    "</button>" +
    "</div>" +
    '<div class="mdlbody qr-modal__body">' +
    '<div class="qr-modal__canvas-wrap">' +
    `<canvas class="qr-modal__canvas" width="${QR_CANVAS_SIZE}" height="${QR_CANVAS_SIZE}"></canvas>` +
    "</div>" +
    '<p class="qr-modal__url" data-qr-url></p>' +
    '<p class="qr-modal__hint" data-qr-hint></p>' +
    "</div>" +
    '<div class="mdlfooter qr-modal__footer">' +
    '<button type="button" class="mdlbtn mdlbtn--secondary" data-qr-copy></button>' +
    '<button type="button" class="mdlbtn mdlbtn--primary" data-qr-download></button>' +
    "</div>" +
    "</div>";
  const iconSlot = el.querySelector("[data-qr-icon]");
  if (iconSlot) iconSlot.innerHTML = iconHTML("qr-code", 24);
  const closeIcon = el.querySelector("[data-qr-close-icon]");
  if (closeIcon) closeIcon.innerHTML = iconHTML("close", 24);

  el.addEventListener("click", (e) => {
    const target = e.target;
    if (target === el) {
      closeQrModal();
      return;
    }
    if (target.closest("[data-qr-close]")) {
      closeQrModal();
      return;
    }
    if (target.closest("[data-qr-copy]")) {
      void copyCurrentUrl(el);
      return;
    }
    if (target.closest("[data-qr-download]")) {
      const canvas = el.querySelector(".qr-modal__canvas");
      if (canvas && currentUrl) downloadQrCanvas(canvas, currentUrl);
    }
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && modalEl && !modalEl.hidden) closeQrModal();
  });
  document.addEventListener("visibilitychange", () => {
    if (modalEl && !modalEl.hidden && !wakeLock) void holdFullBrightness();
  });
  document.body.appendChild(el);
  modalEl = el;
  return el;
}

async function copyCurrentUrl(root) {
  if (!currentUrl) return;
  try {
    await navigator.clipboard.writeText(currentUrl);
  } catch {
  }
  const btn = root.querySelector("[data-qr-copy]");
  if (btn) {
    const original = btn.dataset.label || btn.textContent || "";
    btn.dataset.label = original;
    btn.textContent = btn.dataset.copied || "Copied!";
    btn.classList.add("qr-modal__copied");
    setTimeout(() => {
      btn.textContent = original;
      btn.classList.remove("qr-modal__copied");
    }, 2000);
  }
  announce(currentUrl);
}

export function closeQrModal() {
  if (!modalEl || modalEl.hidden) return;
  modalEl.hidden = true;
  document.body.classList.remove("qr-modal-open");
  releaseFullBrightness();
  if (lastFocused && lastFocused.isConnected) lastFocused.focus();
  lastFocused = null;
}

export function openQrModal(url, labels) {
  if (!url) return;
  labels = labels || {};
  currentUrl = compactQrPayload(url);
  const el = ensureModal();
  const title = el.querySelector("[data-qr-title]");
  if (title) title.textContent = labels.title || "";
  const hint = el.querySelector("[data-qr-hint]");
  if (hint) hint.textContent = labels.hint || "";
  const urlEl = el.querySelector("[data-qr-url]");
  if (urlEl) {
    urlEl.textContent = currentUrl;
    urlEl.title = currentUrl;
  }
  const copyBtn = el.querySelector("[data-qr-copy]");
  if (copyBtn) {
    copyBtn.textContent = labels.copy || "";
    copyBtn.dataset.label = labels.copy || "";
    copyBtn.dataset.copied = labels.copied || "";
  }
  const dlBtn = el.querySelector("[data-qr-download]");
  if (dlBtn) dlBtn.textContent = labels.download || "";
  const closeBtn = el.querySelector("[data-qr-close]");
  if (closeBtn) closeBtn.setAttribute("aria-label", labels.close || "");

  lastFocused = document.activeElement;
  el.hidden = false;
  document.body.classList.add("qr-modal-open");
  void holdFullBrightness();
  const canvas = el.querySelector(".qr-modal__canvas");
  if (canvas) {
    const ctx = qrContext(canvas);
    if (ctx) {
      ctx.fillStyle = "#ffffff";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
    }
    void renderQrWithLogo(canvas, url).catch(() => {});
  }
  if (closeBtn) closeBtn.focus();
}
