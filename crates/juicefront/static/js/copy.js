import { announce } from "./util.js";
import { t } from "./i18n.js";

var iconCache = null;
var iconCounter = 0;

function iconStore() {
  if (!iconCache) {
    try {
      iconCache = JSON.parse(document.getElementById("jb-icons").textContent || "{}");
    } catch {
      iconCache = {};
    }
  }
  return iconCache;
}

export function iconHTML(name, size, className) {
  var raw = iconStore()[name] || "";
  if (!raw) return "";
  var uid = "j" + iconCounter++;
  var cls = className ? ' class="' + className + '"' : "";
  return raw
    .replace("<svg", '<svg width="' + size + '" height="' + size + '"' + cls + ' style="display:block;shape-rendering:crispEdges"')
    .replace(/id="([^"]+)"/g, 'id="$1-' + uid + '"')
    .replace(/url\(#([^)]+)\)/g, "url(#$1-" + uid + ")");
}

export function iconForMime(mime) {
  mime = mime || "";
  if (mime.indexOf("image/") === 0) return "image";
  if (mime.indexOf("video/") === 0) return "video";
  if (mime.indexOf("audio/") === 0) return "audio-waveform";
  if (/spreadsheet|csv|excel/.test(mime)) return "presentation";
  if (/archive|zip|gzip|rar|7z|tar|bzip|xz|compress|zstd/.test(mime)) return "archive";
  if (/wordprocessingml|presentationml/.test(mime)) return "file-text";
  if (/json|xml|javascript|ecmascript|typescript|html|css/.test(mime)) return "code";
  return "file-text";
}

export function extIcon(name) {
  if (/\.(png|jpg|jpeg|gif|webp|svg|avif|bmp|ico)$/i.test(name)) return "image";
  if (/\.(mp4|webm|avi|mov|mkv)$/i.test(name)) return "video";
  if (/\.(mp3|wav|ogg|flac|aac)$/i.test(name)) return "audio-waveform";
  if (/\.(xlsx?|csv)$/i.test(name)) return "presentation";
  if (/\.(zip|gz|tar|rar|7z|bz2|xz|zst)$/i.test(name)) return "archive";
  if (/\.(json|xml|js|ts|html|css)$/i.test(name)) return "code";
  return "file-text";
}

var VANITY_KEYS = {
  FILE_TOO_LARGE: "error.file_too_large",
  EXPIRED: "error.expired",
  INVALID_ID: "error.invalid_id",
  NOT_FOUND: "error.not_found",
  RATE_LIMITED: "error.rate_limited",
  UPSTREAM_ERROR: "error.upstream",
  STORAGE_FULL: "error.storage_full",
  NETWORK_ERROR: "error.network",
  UPLOAD_FAILED: "error.upload_failed",
  UPLOAD_TIMEOUT: "error.upload_timeout",
  TUS_ERROR: "error.tus",
  INVALID_REQUEST: "error.invalid_request",
  UNAUTHORIZED: "error.unauthorized",
  UNKNOWN: "error.unknown",
};

export function vanityMsg(locale, errorCode, rawMessage) {
  if (rawMessage) return rawMessage;
  if (errorCode && VANITY_KEYS[errorCode]) return t(locale, VANITY_KEYS[errorCode]);
  return t(locale, "error.upload_failed");
}

export function copyBarStrings(locale) {
  return {
    title: t(locale, "upload.copy_bar_title"),
    aria: t(locale, "upload.copy_bar_aria"),
    copied: t(locale, "upload.copy_bar_copied"),
    announceMsg: t(locale, "upload.copy_bar_announce"),
  };
}

export function makeCopyBar(url, strings) {
  strings = strings || {};
  var btn = document.createElement("button");
  btn.type = "button";
  btn.className = "copy-bar";
  btn.title = strings.title || "Click to copy link";
  btn.setAttribute("aria-label", strings.aria || "Copy link to clipboard");
  btn.innerHTML =
    '<div class="copy-bar__text-wrapper"><span class="copy-bar__copied-text">' +
    (strings.copied || "Copied!") +
    '</span><span class="copy-bar__url"></span></div>' +
    iconHTML("copy", 24, "copy-bar__copy-icon");
  var urlEl = btn.querySelector(".copy-bar__url");
  if (urlEl) urlEl.textContent = url;
  btn.addEventListener("click", function () {
    try {
      var currentUrl = (urlEl && urlEl.textContent.trim()) || url;
      navigator.clipboard.writeText(currentUrl).then(function () {
        btn.classList.add("copy-bar--copied");
        announce(strings.announceMsg || "Link copied to clipboard");
        setTimeout(function () {
          btn.classList.remove("copy-bar--copied");
        }, 2000);
      });
    } catch {}
  });
  return btn;
}

var confirmTimers = new WeakMap();

export function createDeleteButton(opts) {
  var btn = document.createElement("button");
  btn.type = "button";
  btn.className = "delete-btn";
  btn.setAttribute("aria-label", "Delete file");
  btn.innerHTML = iconHTML("trash", 24);

  btn.addEventListener("click", function (e) {
    if (opts.immediate || e.shiftKey || btn.dataset.confirming) {
      fetch(opts.apiBase + "/file/" + opts.fileId, {
        method: "DELETE",
        headers: { "X-Delete-Token": opts.deleteToken },
      })
        .then(function (res) {
          if (res.ok) {
            var item = btn.closest(".file-item, .file-card");
            if (item) opts.onDeleted(item);
            announce("File deleted");
          }
        })
        .catch(function (err) {
          console.error("Failed to delete file:", err);
          announce("Failed to delete file");
        });
    } else {
      btn.dataset.confirming = "true";
      var clr = setTimeout(function () {
        btn.dataset.confirming = "";
      }, 10000);
      confirmTimers.set(btn, clr);
    }
  });

  return btn;
}
