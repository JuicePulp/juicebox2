/**
 * QR codes for file URLs.
 *
 * Generated fully client-side (no external API, so file URLs never leak
 * to a third party). The QR payload is the file **URL** in Byte mode,
 * rendered with high error correction so the Juicebox logo can sit in
 * the middle without breaking scans.
 */
import { iconHTML } from "./copy.js";
import { announce } from "./util.js";

/**
 * Lazily loaded on first use (dynamic import) so the QR library is never
 * pulled in until the modal actually opens.
 */
async function qrFactory() {
  const mod = await import("./vendor/qrcode.mjs");
  return mod.default ?? mod.qrcode ?? globalThis.qrcode;
}

export const QR_CANVAS_SIZE = 512;
/** Fraction of the QR side used for the centered logo. */
const LOGO_RATIO = 0.25;
/** Png used in the middle of the QR. `logo_big.png` (256px) stays crisp. */
const LOGO_SRC = "/static/assets/logo_big.png";

/**
 * Wide-gamut 2D context where supported. For black-and-white QR content
 * P3 white has the same peak luminance as sRGB white (white is already
 * 100% brightness either way), but this renders correctly on P3 displays
 * and keeps the center logo in its full-gamut colors. Falls back to sRGB.
 */
function qrContext(canvas) {
  try {
    const p3 = canvas.getContext("2d", { colorSpace: "display-p3" });
    if (p3) return p3;
  } catch {
    /* older browsers: fall through to sRGB */
  }
  return canvas.getContext("2d");
}

/**
 * Screen wake lock held while the QR modal is open so the display never
 * auto-dims -- this is what actually keeps the code at FULL brightness.
 */
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
    /* noop */
  }
  wakeLock = null;
}

/**
 * Compact payload for the QR: drop the scheme and the cosmetic file
 * extension (`host/f/<id>`). juicehost ignores the extension for lookup,
 * and scanners prepend `https://` to schemeless links -- so the code
 * stays smaller while scanning to the same file.
 */
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

/**
 * Build the QR module matrix for a URL.
 * Type number 0 = automatic version, "H" = ~30% error correction
 * (required for the center logo overlay).
 */
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

/** Paint the black-on-white QR modules (quiet zone included). */
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

/** Overlay just the Juicebox logo in the center (no margins, no backing). */
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
    // `decode()` rejects on some browsers for cached images; fall back to onload.
    if (typeof img.decode === "function") {
      img.decode().catch(() => {});
    }
  });
}

/**
 * Render a file-URL QR code (with centered Juicebox logo) into a canvas.
 * The base QR paints synchronously so something is visible immediately;
 * the logo is composited on top once the logo loads.
 */
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
  // The canvas may have been reused for another URL while loading.
  paintLogo(ctx, size, logo);
}

/** Derive a download filename like `juicebox-<id>.png` from a file URL. */
export function qrFilenameForUrl(url) {
  try {
    const segment = new URL(url).pathname.split("/").pop() || "";
    const clean = segment.replace(/\.[A-Za-z0-9]+$/, "").replace(/[^A-Za-z0-9_-]+/g, "");
    if (clean) return `juicebox-${clean}.png`;
  } catch {
    /* fall through */
  }
  const parts = String(url || "").split("/");
  const last = parts.pop() || "";
  const fallback = (last.split(".")[0] || "").replace(/[^A-Za-z0-9_-]+/g, "");
  return fallback ? `juicebox-${fallback}.png` : "juicebox-qr.png";
}

/** Download the rendered QR canvas as a PNG file. */
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
  // The OS can drop the wake lock (e.g. tab hidden); re-acquire it when
  // the user comes back while the QR modal is still open.
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
    /* clipboard may be unavailable; still show feedback */
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

/**
 * Open the shared QR modal for a file URL.
 * `labels` = { title, hint, copy, copied, download, close }.
 */
export function openQrModal(url, labels) {
  if (!url) return;
  labels = labels || {};
  // What you scan is what you see (and copy): the compact schemeless,
  // extensionless form.
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
