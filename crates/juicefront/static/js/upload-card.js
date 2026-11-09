import { t } from "./i18n.js?v=2.2.0-r2";
import { announce, formatSize } from "./util.js?v=2.2.0-r2";
import { playSound } from "./sounds.js?v=2.2.0-r2";
import {
  iconHTML,
  iconForMime,
  extIcon,
  makeCopyBar,
  copyBarStrings,
  attachUrlVariantsAfter,
  urlVariantLabels,
  createDeleteButton,
  vanityMsg,
} from "./copy.js?v=2.2.0-r2";
import {
  UPLOAD_URL,
  readMaxFileSize,
  readUploadPassword,
  readSafeUploadEnabled,
  TUS_THRESHOLD,
} from "./upload-config.js?v=2.2.0-r2";
import { apiFetch, apiFetchJob, publicFile } from "./api.js?v=2.2.0-r2";
import { enhanceRetention, rebuildRetention } from "./enhance-retention.js?v=2.2.0-r2";
import {
  enhanceHostSelector,
  readSelectedHost,
  readQuickLinkEnabled,
  readDangerLevel,
  initHostGlow,
} from "./enhance-host.js?v=2.2.0-r2";
import { connectPresence } from "./presence.js?v=2.2.0-r2";
import { resolveUploadHost } from "./upload-router.js?v=2.2.0-r2";
import { validateFileClient } from "./file-validation.js?v=2.2.0-r2";
import { upsertLocalFile } from "./upload-storage.js?v=2.2.0-r2";
import {
  enqueueUpload,
  cancelUpload,
  removeUpload,
  subscribe,
  getUploads,
  wasRemoved,
  connectUploads,
  findPausedUpload,
  resumeUpload,
} from "./upload-client-loader.js?v=2.2.0-r2";
import { extractServerId } from "./upload-engine.js?v=2.2.0-r2";
import { initHostSelector } from "./host-selector.js?v=2.2.0-r2";

var assetVersion = (() => {
  try {
    return document.getElementById("server-config")?.getAttribute("data-version") || "";
  } catch {
    return "";
  }
})();
// Folder zipping is only needed on a directory drop, so it stays out of the first load.
var folderZip = await import(
  assetVersion
    ? `./folder-zip.js?v=${encodeURIComponent(assetVersion)}`
    : "./folder-zip.js"
);

var LOCALE = (document.documentElement && document.documentElement.lang) || "en";
var rowIds = new Map();
var prevSeen = new Map();
var rowPartsCache = new WeakMap();
var suppressAdopt = 0;
var listContentRef = null;
var emptyTextRef = null;
var resumeMessages = new Map();
var resumeInput = null;
var resumeTargetId = "";

/** Copies the share URL, ignoring rejection because the Clipboard API can reject without a user gesture. */
function autoCopyUrl(url, copyBar) {
  if (!url || !navigator.clipboard) return;
  navigator.clipboard.writeText(url).then(
    function () {
      if (copyBar && copyBar.classList.contains("copy-bar") && copyBar.isConnected) {
        copyBar.classList.add("copy-bar--copied");
        setTimeout(function () {
          copyBar.classList.remove("copy-bar--copied");
        }, 2000);
      }
    },
    function () {},
  );
}

function getFormRef() {
  return document.querySelector("[data-upload-form]");
}
function getCardRef() {
  return document.getElementById("upload-card");
}
function getInputRef() {
  return document.getElementById("dropzone-input");
}
function getDropZoneRef() {
  return document.querySelector("[data-drop-zone]");
}
function getTextareaRef() {
  return document.querySelector("[data-text-textarea]");
}
function getCobaltTextareaRef() {
  return document.querySelector("[data-cobalt-textarea]");
}
function isCobaltEnabled() {
  var el = document.getElementById("server-config");
  return el && el.getAttribute("data-cobalt") === "true";
}
function cobaltServiceDomains() {
  var form = getFormRef();
  var raw = form ? form.getAttribute("data-cobalt-services") : "";
  return (raw || "")
    .split(",")
    .map(function (s) {
      return s.trim().toLowerCase();
    })
    .filter(Boolean);
}
function urlMatchesCobaltService(raw) {
  var host;
  try {
    host = new URL(raw).hostname.toLowerCase();
  } catch {
    return false;
  }
  return cobaltServiceDomains().some(function (domain) {
    return host === domain || host.endsWith("." + domain);
  });
}
function ttlHours() {
  var form = getFormRef();
  var checked = form ? form.querySelector('input[name="ttl_hours"]:checked') : null;
  return Number(checked ? checked.value : 0) || 24;
}

function cobaltFetchUrls() {
  var cobaltTextarea = getCobaltTextareaRef();
  var value = cobaltTextarea ? cobaltTextarea.value : "";
  var lines = value.split(/\r?\n/);
  var count = 0;
  for (var i = 0; i < lines.length; i++) {
    var url = lines[i].trim();
    if (!url) continue;
    startCobaltFetch(url);
    count++;
  }
  if (count > 0 && cobaltTextarea) cobaltTextarea.value = "";
  return count;
}

function cobaltHasUrls() {
  var cobaltTextarea = getCobaltTextareaRef();
  if (!cobaltTextarea) return true;
  return cobaltTextarea.value.trim().length > 0;
}

function handleCobaltKeydown(e) {
  // Ignore Enter that confirms an IME composition instead of submitting.
  if (e.isComposing || e.keyCode === 229) return;
  if (e.key !== "Enter") return;
  e.preventDefault();
  if (e.shiftKey) {
    var el = getCobaltTextareaRef();
    if (!el) return;
    var pos = el.selectionStart;
    el.setRangeText("\n", pos, pos, "end");
    return;
  }
  cobaltFetchUrls();
}
function setEmpty(visible) {
  if (emptyTextRef) emptyTextRef.classList.toggle("visible", visible);
}

/** SHA-256 over name, size and 256 KiB at each end; falls back to metadata without `crypto.subtle`, so `pausedFileMatches` also compares raw fields. */
async function fileFingerprint(file) {
  const fallback = "meta:" + file.name + ":" + file.size + ":" + file.lastModified;
  try {
    if (!file || typeof crypto === "undefined" || !crypto.subtle)
      return fallback;
    const sliceSize = 256 * 1024;
    const first = new Uint8Array(await file.slice(0, Math.min(file.size, sliceSize)).arrayBuffer());
    const last = new Uint8Array(await file.slice(Math.max(0, file.size - sliceSize), file.size).arrayBuffer());
    const prefix = new TextEncoder().encode(file.name + "\u0000" + file.size + "\u0000");
    const data = new Uint8Array(prefix.length + first.length + last.length);
    data.set(prefix, 0);
    data.set(first, prefix.length);
    data.set(last, prefix.length + first.length);
    const digest = await crypto.subtle.digest("SHA-256", data);
    return Array.from(new Uint8Array(digest))
      .map(function (byte) {
        return byte.toString(16).padStart(2, "0");
      })
      .join("");
  } catch {
    return fallback;
  }
}

function pausedFileMatches(item, file, fingerprint) {
  if (!item || !file) return false;
  if (item.fingerprint && fingerprint) return item.fingerprint === fingerprint;
  return item.filename === file.name && item.size === file.size && item.lastModified === file.lastModified;
}

function clearErrorRow(row) {
  if (!row) return;
  row.classList.remove("error");
  var parts = getRowParts(row);
  parts.fill.classList.remove("error");
  parts.pill.classList.remove("error");
}

function clearPausedRow(row) {
  if (!row) return;
  row.classList.remove("paused");
  var parts = getRowParts(row);
  parts.fill.classList.remove("paused");
  parts.pill.classList.remove("paused");
  var button = row.querySelector(".resume-upload-btn");
  if (button) button.remove();
}

function ensureResumeButton(row) {
  var button = row.querySelector(".resume-upload-btn");
  if (button) {
    button.textContent = t(LOCALE, "upload.resume_file");
    return button;
  }
  button = document.createElement("button");
  button.type = "button";
  button.className = "resume-upload-btn";
  button.textContent = t(LOCALE, "upload.resume_file");
  button.addEventListener("click", function () {
    openResumePicker(row);
  });
  var actionArea = row.querySelector(".file-action-area");
  (actionArea || row).appendChild(button);
  return button;
}

function ensureResumeInput() {
  if (resumeInput) return resumeInput;
  resumeInput = document.createElement("input");
  resumeInput.type = "file";
  resumeInput.hidden = true;
  resumeInput.setAttribute("aria-hidden", "true");
  resumeInput.addEventListener("change", function () {
    var id = resumeTargetId;
    var file = resumeInput.files && resumeInput.files[0];
    resumeTargetId = "";
    resumeInput.value = "";
    if (id && file) void resumeSelectedFile(id, file);
  });
  document.body.appendChild(resumeInput);
  return resumeInput;
}

function openResumePicker(row) {
  resumeTargetId = row.getAttribute("data-upload-id") || "";
  var input = ensureResumeInput();
  input.value = "";
  input.click();
}

function setResumeMessage(id, message) {
  if (message) resumeMessages.set(id, message);
  else resumeMessages.delete(id);
  var item = getUploads().find(function (upload) {
    return upload.id === id;
  });
  if (item) syncRow(item);
}

async function resumePausedItem(item, file, fingerprint) {
  if (!item || !file) return;
  var row = rowIds.get(item.id);
  if (!row) {
    adoptItem(item);
    row = rowIds.get(item.id);
  }
  if (item.requiresPassword && !readUploadPassword()) {
    setResumeMessage(item.id, t(LOCALE, "upload.resume_password"));
    var password = document.querySelector("[data-upload-password]");
    if (password) password.focus();
    return;
  }
  if (!pausedFileMatches(item, file, fingerprint)) {
    setResumeMessage(item.id, t(LOCALE, "upload.resume_mismatch"));
    return;
  }
  resumeMessages.delete(item.id);
  var id = resumeUpload(item.id, file, {
    password: item.requiresPassword ? readUploadPassword() : ""
  }, fingerprint);
  if (!id) {
    setResumeMessage(item.id, item.requiresPassword ? t(LOCALE, "upload.resume_password") : t(LOCALE, "upload.resume_mismatch"));
    return;
  }
  playSound("loading", { emphasis: "subtle" });
}

async function resumeSelectedFile(id, file) {
  var item = getUploads().find(function (upload) {
    return upload.id === id;
  });
  if (!item || item.state !== "paused") return;
  var fingerprint = await fileFingerprint(file);
  if (!pausedFileMatches(item, file, fingerprint)) {
    setResumeMessage(id, t(LOCALE, "upload.resume_mismatch"));
    return;
  }
  await resumePausedItem(item, file, fingerprint);
}

function removeItem(item) {
  var id = item.getAttribute("data-upload-id");
  if (id) {
    cancelUpload(id);
    rowIds.delete(id);
    resumeMessages.delete(id);
  }
  item.classList.add("exiting");
  setTimeout(function () {
    item.remove();
    setEmpty(!listContentRef || !listContentRef.querySelector(".file-item:not(.exiting)"));
  }, 300);
}

function dismissRow(item) {
  var id = item.getAttribute("data-upload-id");
  if (id) removeUpload(id);
  removeItem(item);
}

function markRowProtected(row) {
  if (!row || row.querySelector(".file-protected")) return;
  var nameEl = row.querySelector(".file-name");
  if (!nameEl) return;
  var wrap = document.createElement("span");
  wrap.className = "file-name-row";
  nameEl.before(wrap);
  var badge = document.createElement("span");
  badge.className = "file-protected";
  badge.title = t(LOCALE, "files.protected");
  badge.setAttribute("aria-label", t(LOCALE, "files.protected"));
  badge.innerHTML = iconHTML("key", 24);
  wrap.append(badge, nameEl);
}
function wipeFailedReservation(item) {
  try {
    if (!item || item.dataset.wiped) return;
    item.dataset.wiped = "1";
    var fid = item.getAttribute("data-file-id") || "";
    var token = item.getAttribute("data-delete-token") || "";
    if (fid) {
      try {
        var stored = JSON.parse(localStorage.getItem("juicebox_uploads") || "[]");
        var next = stored.filter(function (e) { return e && e.id !== fid; });
        if (next.length !== stored.length) {
          localStorage.setItem("juicebox_uploads", JSON.stringify(next));
        }
      } catch (e) {}
    }
    if (fid && token) {
      fetch(publicFile(fid), {
        method: "DELETE",
        headers: { "X-Delete-Token": token },
      }).catch(function () {});
    }
  } catch (e) {}
}
function failItem(item, pill, fill, status, errorCode, rawMessage) {
  var uploadId = item.getAttribute("data-upload-id");
  if (uploadId) resumeMessages.delete(uploadId);
  clearPausedRow(item);
  item.classList.add("error");
  pill.classList.add("error");
  fill.classList.add("error");
  fill.classList.remove("finalizing");
  fill.classList.remove("indeterminate");
  var spinner = item.querySelector(".spinner");
  if (spinner) spinner.remove();
  var msg = vanityMsg(LOCALE, errorCode, rawMessage);
  status.textContent = msg;
  announce(t(LOCALE, "upload.announce_failed", { message: msg }));
  playSound("error");
  wipeFailedReservation(item);
}
function animateRowOut(item) {
  item.classList.add("exiting");
  setTimeout(function () {
    item.remove();
    if (listContentRef && !listContentRef.querySelector(".file-item:not(.exiting)")) {
      var et = listContentRef.querySelector(".empty-text");
      if (et) et.classList.add("visible");
    }
  }, 300);
}
function onDeleted(item) {
  var uploadId = item.getAttribute("data-upload-id");
  var fileId = item.getAttribute("data-file-id") || "";
  if (uploadId) {
    rowIds.delete(uploadId);
    removeUpload(uploadId);
  } else if (fileId) {
    var match = getUploads().find(function (u) {
      return (u.serverId || extractServerId(u.url || "")) === fileId;
    });
    if (match) removeUpload(match.id);
  }
  animateRowOut(item);
}
function getRowParts(row) {
  var parts = rowPartsCache.get(row);
  if (!parts) {
    parts = {
      fill: row.querySelector(".progress-fill"),
      status: row.querySelector(".status-text"),
      pill: row.querySelector(".status-pill"),
    };
    rowPartsCache.set(row, parts);
  }
  return parts;
}
function showQuickLink(item, row) {
  if (!item.reserveUrl || row.hasAttribute("data-quick-link")) return;
  if (item.state === "done") return;
  row.setAttribute("data-quick-link", "");
  var quickBar = makeCopyBar(item.reserveUrl, copyBarStrings(LOCALE));
  getRowParts(row).pill.after(quickBar);
  attachUrlVariantsAfter(
    quickBar,
    { preview: item.reserveUrl, raw: item.reserveRawUrl, download: item.reserveDownloadUrl },
    urlVariantLabels(LOCALE),
  );
}
function completeRow(item, row) {
  clearPausedRow(row);
  clearErrorRow(row);
  var parts = getRowParts(row);
  var fill = parts.fill;
  var status = parts.status;
  var pill = parts.pill;
  fill.classList.remove("finalizing");
  fill.style.setProperty("--progress", "100%");
  var progressDivider = fill.closest('[role="progressbar"]');
  if (progressDivider) progressDivider.setAttribute("aria-valuenow", "100");
  var iconContainer = row.querySelector(".file-icon");
  if (iconContainer && item.mimeType) {
    iconContainer.innerHTML = iconHTML(iconForMime(item.mimeType), 24);
  }
  row.classList.add("complete");
  var actionArea = row.querySelector(".file-action-area");
  if (actionArea) actionArea.classList.add("complete");
  if (item.protected) markRowProtected(row);
  status.textContent = t(LOCALE, "upload.complete");
  var spinner = pill.querySelector(".spinner");
  if (spinner) spinner.remove();
  var existingCopyBar = pill.nextElementSibling;
  var copyBar;
  if (existingCopyBar && existingCopyBar.classList.contains("copy-bar")) {
    existingCopyBar.replaceWith(makeCopyBar(item.url || "", copyBarStrings(LOCALE)));
  } else {
    pill.after(makeCopyBar(item.url || "", copyBarStrings(LOCALE)));
  }
  copyBar = pill.nextElementSibling;

  attachUrlVariantsAfter(
    copyBar,
    { preview: item.url, raw: item.rawUrl, download: item.downloadUrl },
    urlVariantLabels(LOCALE),
  );

  if (row.hasAttribute("data-quick-link")) {
    setTimeout(function () {
      var bar = pill.nextElementSibling;
      if (bar && bar.classList.contains("copy-bar") && row.isConnected) {
        pill.style.boxSizing = "border-box";
        pill.style.height = pill.offsetHeight + "px";
        pill.offsetHeight;
        pill.classList.add("sliding");
        bar.classList.add("copy-bar--rounded");
        // Two rAFs let the fixed pill height apply before it animates back to auto.
        requestAnimationFrame(function () {
          requestAnimationFrame(function () {
            pill.style.height = "";
          });
        });
      }
    }, 2000);
  } else {
    pill.style.display = "none";
    var bar2 = pill.nextElementSibling;
    if (bar2 && bar2.classList.contains("copy-bar")) {
      bar2.style.marginTop = "0.75rem";
    }
  }

  announce(t(LOCALE, "upload.announce_complete", { filename: item.filename }));
  playSound("success");
  autoCopyUrl(item.url || "", copyBar);

  var delBtn = row.querySelector(".delete-btn");
  var fileId = item.serverId || extractServerId(item.url || "") || item.id;
  if (delBtn && fileId && item.deleteToken) {
    delBtn.replaceWith(
      createDeleteButton({
        fileId: fileId,
        deleteToken: item.deleteToken,
        onDeleted: onDeleted,
        immediate: true,
      }),
    );
  }
}
function syncRow(item) {
  var row = rowIds.get(item.id);
  if (!row || !row.isConnected) return;
  var parts = getRowParts(row);
  var fill = parts.fill;
  var status = parts.status;
  var pill = parts.pill;

  if (item.state !== "paused") {
    clearPausedRow(row);
    resumeMessages.delete(item.id);
  }
  if (item.state === "queued" || item.state === "compressing" || item.state === "uploading" || item.state === "finalizing")
    clearErrorRow(row);

  showQuickLink(item, row);

  switch (item.state) {
    case "paused": {
      clearErrorRow(row);
      row.classList.add("paused");
      fill.classList.remove("finalizing");
      fill.classList.remove("indeterminate");
      fill.classList.add("paused");
      fill.style.setProperty("--progress", (Number(item.progress) || 0) + "%");
      var pausedDivider = fill.closest('[role="progressbar"]');
      if (pausedDivider) {
        pausedDivider.setAttribute("aria-valuenow", String(Number(item.progress || 0).toFixed(1)));
      }
      pill.classList.add("paused");
      status.textContent = resumeMessages.get(item.id) || t(LOCALE, "upload.interrupted");
      ensureResumeButton(row);
      break;
    }
    case "queued":
    case "compressing":
      status.textContent =
        item.method === "tus"
          ? t(LOCALE, "upload.initializing_tus")
          : t(LOCALE, "upload.initializing");
      break;
    case "uploading": {
      fill.style.setProperty("--progress", item.progress + "%");
      var progressDivider = fill.closest('[role="progressbar"]');
      if (progressDivider) {
        progressDivider.setAttribute("aria-valuenow", String(Number(item.progress).toFixed(1)));
      }
      status.textContent = t(LOCALE, "upload.uploading", {
        percent: Number(item.progress).toFixed(1),
      });
      break;
    }
    case "finalizing":
      fill.classList.add("finalizing");
      status.textContent = t(LOCALE, "upload.finalizing");
      break;
    case "done":
      if (!row.classList.contains("complete")) completeRow(item, row);
      break;
    case "error":
      if (!row.classList.contains("error")) {
        failItem(row, pill, fill, status, item.errorCode, item.errorMessage);
      }
      break;
    case "cancelled":
      if (!row.classList.contains("error")) {
        row.classList.add("error");
        var spinner = row.querySelector(".spinner");
        if (spinner) spinner.remove();
        status.textContent = t(LOCALE, "upload.cancelled");
      }
      break;
  }
}

function setCobaltBarProgress(fill, percent) {
  fill.classList.remove("indeterminate");
  fill.style.setProperty("--progress", percent + "%");
  var progressDivider = fill.closest('[role="progressbar"]');
  if (progressDivider) progressDivider.setAttribute("aria-valuenow", String(percent));
}

function setCobaltBarIndeterminate(fill) {
  fill.classList.add("indeterminate");
  fill.style.removeProperty("--progress");
  var progressDivider = fill.closest('[role="progressbar"]');
  if (progressDivider) progressDivider.removeAttribute("aria-valuenow");
}

function startCobaltFetch(url) {
  setEmpty(false);
  var item = document.createElement("div");
  item.className = "file-item";
  item.setAttribute("role", "listitem");
  item.innerHTML =
    '<div class="file-item-header">' +
    '<div class="file-card-icon">' + iconHTML("video", 24) + "</div>" +
    '<div class="file-info-left">' +
    '<span class="file-name"></span>' +
    '<span class="file-size"></span>' +
    "</div>" +
    '<button type="button" class="delete-btn" aria-label="' + t(LOCALE, "upload.remove_from_queue") + '">' + iconHTML("trash", 24) + "</button>" +
    "</div>" +
    '<div class="file-action-area">' +
    '<div class="progress-divider" role="progressbar" aria-valuenow="0" aria-valuemin="0" aria-valuemax="100" aria-label="' + t(LOCALE, "upload.progress_aria") + '">' +
    '<div class="progress-fill" style="--progress:0%"></div>' +
    "</div>" +
    '<div class="status-pill" role="status">' +
    '<div class="status-content">' +
    '<img src="/static/assets/loading.webp" alt="" class="spinner" aria-hidden="true" width="16" height="16" />' +
    '<span class="status-text">' + t(LOCALE, "upload.cobalt_queued") + "</span>" +
    "</div></div></div>";
  var nameEl = item.querySelector(".file-name");
  if (nameEl) nameEl.textContent = url.length > 48 ? url.slice(0, 45) + "..." : url;
  item.querySelector(".delete-btn").addEventListener("click", function () {
    dismissRow(item);
  });
  listContentRef.prepend(item);
  var fill = item.querySelector(".progress-fill");
  var status = item.querySelector(".status-text");
  var pill = item.querySelector(".status-pill");

  var audioOnly = !!(document.querySelector("[data-cobalt-audio-only]") || {}).checked;
  var qualityEl = document.querySelector("[data-cobalt-quality]");
  var containerEl = document.querySelector("[data-cobalt-container]");
  var formatEl = document.querySelector("[data-cobalt-format]");
  var betterEl = document.querySelector("[data-cobalt-better-audio-mode]");
  var videoQuality = qualityEl ? qualityEl.value : undefined;
  var videoContainer = containerEl ? containerEl.value : undefined;
  var audioFormat = formatEl ? formatEl.value : undefined;
  var betterAudio = betterEl ? betterEl.value === "enhanced" : false;

  var payload = { url: url, audio_only: audioOnly };
  var _pw = readUploadPassword();
  if (_pw) payload.password = _pw;
  if (audioOnly) {
    payload.audio_format = audioFormat;
    payload.better_audio = betterAudio;
  } else {
    payload.video_quality = videoQuality;
    payload.video_container = videoContainer;
  }

  fetch(UPLOAD_URL + apiFetch, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  })
    .then(function (res) {
      if (!res.ok) {
        return res.json().then(
          function (err) {
            failItem(item, pill, fill, status, err.error, err.message);
          },
          function () {
            failItem(item, pill, fill, status, undefined, undefined);
          },
        );
      }
      return res.json().then(function (start) {
        if (!start.job_id) {
          failItem(item, pill, fill, status, "BAD_RESPONSE");
          return;
        }
        setCobaltBarIndeterminate(fill);
        status.textContent = t(LOCALE, "upload.cobalt_contacting");
        pollCobaltJob(start.job_id, item, fill, status, pill);
      });
    })
    .catch(function () {
      failItem(item, pill, fill, status, "NETWORK_ERROR");
    });
}

/** Polls every 1 s (5 s while hidden) for up to 900 attempts; a 404 or removed row stops the loop, and poll errors are retried. */
function pollCobaltJob(jobId, item, fill, status, pill) {
  var INTERVAL = 1000;
  var HIDDEN_INTERVAL = 5000;
  var MAX_ATTEMPTS = 900;
  var attempts = 0;
  function nextDelay() {
    return typeof document !== "undefined" && document.hidden ? HIDDEN_INTERVAL : INTERVAL;
  }
  function poll() {
    if (attempts++ >= MAX_ATTEMPTS || !item.isConnected) return;
    // Cache-bust: job status is polled and some proxies cache GETs.
    fetch(UPLOAD_URL + apiFetchJob(jobId) + "?t=" + Date.now())
      .then(function (res) {
        if (res.status === 404) {
          failItem(item, pill, fill, status, undefined, t(LOCALE, "upload.cobalt_gone"));
          return null;
        }
        if (!res.ok) {
          setTimeout(poll, nextDelay());
          return null;
        }
        return res.json();
      })
      .then(function (data) {
        if (!data) return;
        if (data.status === "done" && data.file) {
          completeCobaltRow(data.file, item, fill, status, pill);
          return;
        }
        if (data.status === "failed") {
          failItem(item, pill, fill, status, undefined, data.error || t(LOCALE, "error.unknown"));
          return;
        }
        if ((data.stage || "").startsWith("retry-")) {
          var n = data.stage.split("-")[1];
          setCobaltBarIndeterminate(fill);
          status.textContent = t(LOCALE, "upload.cobalt_retry", { n: n });
        } else if (data.stage === "converting") {
          setCobaltBarIndeterminate(fill);
          status.textContent = t(LOCALE, "upload.cobalt_converting");
        } else if (
          typeof data.total_bytes === "number" &&
          data.total_bytes > 0
        ) {
          var received =
            typeof data.bytes_received === "number" ? data.bytes_received : 0;
          var percent = Math.max(
            0,
            Math.min(99, Math.floor((received / data.total_bytes) * 100)),
          );
          setCobaltBarProgress(fill, percent);
          status.textContent = t(LOCALE, "upload.cobalt_downloading", {
            percent: percent,
          });
        } else if (data.stage === "transfer" || data.stage === "transcode") {
          setCobaltBarIndeterminate(fill);
          status.textContent = t(LOCALE, "upload.cobalt_fetching");
        } else if (data.stage === "session-fallback") {
          setCobaltBarIndeterminate(fill);
          status.textContent = t(LOCALE, "upload.cobalt_relay");
        } else if (data.status !== "pending") {
          setCobaltBarIndeterminate(fill);
          status.textContent = t(LOCALE, "upload.cobalt_fetching");
        }
        setTimeout(poll, nextDelay());
      })
      .catch(function () {
        setTimeout(poll, nextDelay());
      });
  }
  setTimeout(poll, nextDelay());
}

function completeCobaltRow(file, item, fill, status, pill) {
  var nameEl = item.querySelector(".file-name");
  if (nameEl) nameEl.textContent = file.filename;
  var sizeEl = item.querySelector(".file-size");
  if (sizeEl) sizeEl.textContent = formatSize(file.size_bytes);
  var iconContainer = item.querySelector(".file-card-icon");
  if (iconContainer && file.mime_type) {
    iconContainer.innerHTML = iconHTML(iconForMime(file.mime_type), 24);
  }
  fill.classList.remove("finalizing");
  fill.classList.remove("indeterminate");
  fill.style.setProperty("--progress", "100%");
  var progressDivider = fill.closest('[role="progressbar"]');
  if (progressDivider) progressDivider.setAttribute("aria-valuenow", "100");
  item.classList.add("complete");
  item.setAttribute("data-file-id", file.id);
  if (file.protected) markRowProtected(item);
  var actionArea = item.querySelector(".file-action-area");
  if (actionArea) actionArea.classList.add("complete");
  status.textContent = t(LOCALE, "upload.complete");
  var spinner = pill.querySelector(".spinner");
  if (spinner) spinner.remove();
  pill.style.display = "none";
  var cobaltBar = makeCopyBar(file.url, copyBarStrings(LOCALE));
  pill.after(cobaltBar);
  attachUrlVariantsAfter(
    cobaltBar,
    { preview: file.url, raw: file.raw_url, download: file.download_url },
    urlVariantLabels(LOCALE),
  );
  var copyBar = pill.nextElementSibling;
  if (copyBar && copyBar.classList.contains("copy-bar")) {
    copyBar.style.marginTop = "0.75rem";
  }
  var delBtn = item.querySelector(".delete-btn");
  if (delBtn && file.id && file.delete_token) {
    delBtn.replaceWith(
      createDeleteButton({
        fileId: file.id,
        deleteToken: file.delete_token,
        onDeleted: onDeleted,
        immediate: true,
      }),
    );
  }
  upsertLocalFile({
    id: file.id,
    filename: file.filename,
    mime_type: file.mime_type,
    size_bytes: file.size_bytes,
    uploaded_at: Math.floor(Date.now() / 1000),
    url: file.url,
    raw_url: file.raw_url,
    download_url: file.download_url,
    expires_at: file.expires_at,
    delete_token: file.delete_token,
  });
  announce(t(LOCALE, "upload.announce_complete", { filename: file.filename }));
  playSound("success");
  autoCopyUrl(file.url, copyBar);
}

function buildRow(filename, size, icon) {
  var item = document.createElement("div");
  item.className = "file-item";
  item.setAttribute("role", "listitem");
  item.innerHTML =
    '<div class="file-item-header">' +
    '<div class="file-card-icon">' + iconHTML(icon, 24) + "</div>" +
    '<div class="file-info-left">' +
    '<span class="file-name"></span>' +
    '<span class="file-size"></span>' +
    "</div>" +
    '<button type="button" class="delete-btn" aria-label="' + t(LOCALE, "upload.remove_from_queue") + '">' + iconHTML("trash", 24) + "</button>" +
    "</div>" +
    '<div class="file-action-area">' +
    '<div class="progress-divider" role="progressbar" aria-valuenow="0" aria-valuemin="0" aria-valuemax="100" aria-label="' + t(LOCALE, "upload.progress_aria") + '">' +
    '<div class="progress-fill" style="--progress:0%"></div>' +
    "</div>" +
    '<div class="status-pill" role="status">' +
    '<div class="status-content">' +
    '<img src="/static/assets/loading.webp" alt="" class="spinner" aria-hidden="true" width="16" height="16" />' +
    '<span class="status-text">' + t(LOCALE, "upload.initializing") + "</span>" +
    "</div></div></div>";
  item.querySelector(".file-name").textContent = filename;
  item.querySelector(".file-size").textContent = size;
  return item;
}

function adoptItem(data) {
  if (rowIds.has(data.id)) return;
  setEmpty(false);
  var item = buildRow(data.filename || "upload", formatSize(data.size || 0), iconForMime(data.mimeType || ""));
  if (data.state === "paused") item.classList.add("paused");
  item.querySelector(".delete-btn").addEventListener("click", function () {
    dismissRow(item);
  });
  listContentRef.prepend(item);
  item.setAttribute("data-upload-id", data.id);
  rowIds.set(data.id, item);
  syncRow(data);
}

function setRowDeleteHandler(item, handler) {
  var button = item.querySelector(".delete-btn");
  if (!button) return;
  var replacement = button.cloneNode(true);
  button.replaceWith(replacement);
  replacement.addEventListener("click", handler);
}

async function startItem(file) {
  setEmpty(false);
  var fingerprint = await fileFingerprint(file);
  var paused = findPausedUpload(file, fingerprint);
  if (paused) {
    await resumePausedItem(paused, file, fingerprint);
    return;
  }
  var item = buildRow(file.name, formatSize(file.size), extIcon(file.name));
  setRowDeleteHandler(item, function () {
    dismissRow(item);
  });
  listContentRef.prepend(item);
  uploadItem(file, item, fingerprint);
}

async function uploadItem(file, item, fingerprint) {
  var fill = item.querySelector(".progress-fill");
  var status = item.querySelector(".status-text");
  var pill = item.querySelector(".status-pill");
  if (!fingerprint)
    fingerprint = await fileFingerprint(file);

  if (file.size > readMaxFileSize()) {
    failItem(item, pill, fill, status, "FILE_TOO_LARGE", t(LOCALE, "error.file_too_large"));
    return;
  }
  var validation = validateFileClient(file.name, readDangerLevel());
  if (!validation.allowed) {
    failItem(
      item,
      pill,
      fill,
      status,
      "FILE_BLOCKED",
      t(LOCALE, "error.file_blocked", {
        reason: t(LOCALE, validation.reason || "error.unknown"),
      }),
    );
    return;
  }
  // `enqueueUpload` publishes synchronously; suppress adoption so this row is not duplicated.
  suppressAdopt++;
  var id;
  try {
    id = enqueueUpload({
      file: file,
      ttlHours: ttlHours(),
      customHost: readSelectedHost() || await resolveUploadHost(),
      quickLink: readQuickLinkEnabled(),
      safeUpload: readSafeUploadEnabled(),
      password: readUploadPassword(),
      fingerprint: fingerprint,
    });
  } finally {
    suppressAdopt--;
  }
  item.setAttribute("data-upload-id", id);
  rowIds.set(id, item);
  playSound("loading", { emphasis: "subtle" });
  status.textContent =
    file.size > TUS_THRESHOLD
      ? t(LOCALE, "upload.initializing_tus")
      : t(LOCALE, "upload.initializing");
}

function startFolderZip(dirs, looseEntries) {
  var name = dirs.length === 1 ? folderZip.zipName(dirs[0].name) : "folders.zip";
  setEmpty(false);
  var placeholder = buildRow(name, "", extIcon(name));
  placeholder.querySelector(".file-size").textContent = "";
  placeholder.querySelector(".status-text").textContent = t(LOCALE, "upload.zipping");
  listContentRef.prepend(placeholder);
  announce(t(LOCALE, "upload.zipping"));

  var cancelled = false;
  setRowDeleteHandler(placeholder, function () {
    cancelled = true;
    removeItem(placeholder);
  });

  Promise.all(
    (looseEntries || []).map(function (entry) {
      return folderZip.resolveDroppedFile(entry).catch(function () {
        return null;
      });
    }),
  ).then(function (loose) {
    loose
      .filter(function (file) {
        return file && file.size > 0;
      })
      .forEach(startItem);
  });

  folderZip
    .buildFolderZip(dirs)
    .then(function (zip) {
      if (cancelled) return;
      if (!zip) {
        showFolderZipError(placeholder, name, t(LOCALE, "error.folder_empty"));
        return;
      }
      uploadFolderZip(placeholder, zip);
    })
    .catch(function (err) {
      if (cancelled) return;
      if (window.DEBUG_LOGS) console.error("[folder-zip]", err);
      showFolderZipError(
        placeholder,
        name,
        err && err.code === "ZIP_TOO_LARGE"
          ? t(LOCALE, "error.file_too_large")
          : t(LOCALE, "error.upload_failed"),
      );
    });

  function uploadFolderZip(item, zip) {
    item.querySelector(".file-name").textContent = zip.name;
    item.querySelector(".file-size").textContent = formatSize(zip.size);
    var iconContainer = item.querySelector(".file-card-icon");
    if (iconContainer) {
      iconContainer.innerHTML = iconHTML(extIcon(zip.name), 24);
    }
    setRowDeleteHandler(item, function () {
      dismissRow(item);
    });
    uploadItem(zip, item);
  }

  function showFolderZipError(item, zipFileName, message) {
    item.querySelector(".file-name").textContent = zipFileName;
    item.querySelector(".file-size").textContent = "";
    setRowDeleteHandler(item, function () {
      dismissRow(item);
    });
    failItem(
      item,
      item.querySelector(".status-pill"),
      item.querySelector(".progress-fill"),
      item.querySelector(".status-text"),
      "FOLDER_ZIP",
      message,
    );
  }
}

/** The `?uploaded=` payload is attacker-controllable, so clamp every string and coerce numbers before it reaches the DOM. */
function sanitizeUploaded(raw) {
  function str(v, max) {
    if (typeof v !== "string") return String(v == null ? "" : v).slice(0, max);
    return v.length > max ? v.slice(0, max) : v;
  }
  function num(v) {
    return typeof v === "number" && Number.isFinite(v) ? v : 0;
  }
  return {
    id: str(raw.id, 50),
    filename: str(raw.filename, 500),
    mime_type: str(raw.mime_type, 200),
    size_bytes: num(raw.size_bytes),
    url: str(raw.url, 2048),
    delete_token: str(raw.delete_token, 200),
  };
}

function addUploadedItem(data) {
  setEmpty(false);
  var item = document.createElement("div");
  item.className = "file-item complete";
  item.setAttribute("role", "listitem");
  var icon = iconForMime(data.mime_type || "");
  item.innerHTML =
    '<div class="file-item-header">' +
    '<div class="file-card-icon">' + iconHTML(icon, 24) + "</div>" +
    '<div class="file-info-left">' +
    '<span class="file-name"></span>' +
    '<span class="file-size"></span>' +
    "</div></div>" +
    '<div class="file-action-area complete"></div>';
  item.querySelector(".file-name").textContent = data.filename;
  item.querySelector(".file-size").textContent = formatSize(data.size_bytes);
  if (data.id) item.setAttribute("data-file-id", data.id);
  var actionArea = item.querySelector(".file-action-area");
  var uploadedBar = makeCopyBar(data.url, copyBarStrings(LOCALE));
  actionArea.appendChild(uploadedBar);
  attachUrlVariantsAfter(
    uploadedBar,
    { preview: data.url, raw: data.raw_url, download: data.download_url },
    urlVariantLabels(LOCALE),
  );
  if (data.id && data.delete_token) {
    actionArea.appendChild(
      createDeleteButton({
        fileId: data.id,
        deleteToken: data.delete_token,
        onDeleted: onDeleted,
        immediate: true,
      }),
    );
  }
  listContentRef.prepend(item);
}

function enhanceServerFiles() {
  var rendered = listContentRef ? listContentRef.querySelectorAll("[data-server-rendered]") : [];
  rendered.forEach(function (el) {
    var id = el.getAttribute("data-file-id") || "";
    var token = el.getAttribute("data-delete-token") || "";
    var url = el.getAttribute("data-url") || "";
    var link = el.querySelector("a.copy-bar");
    if (link) {
      var serverBar = makeCopyBar(url, copyBarStrings(LOCALE));
      link.replaceWith(serverBar);
      attachUrlVariantsAfter(
        serverBar,
        {
          preview: url,
          raw: el.getAttribute("data-raw-url") || "",
          download: el.getAttribute("data-download-url") || "",
        },
        urlVariantLabels(LOCALE),
      );
    }
    var noscript = el.querySelector("noscript");
    if (noscript) {
      noscript.replaceWith(
        createDeleteButton({
          fileId: id,
          deleteToken: token,
          onDeleted: onDeleted,
        }),
      );
    }
  });
}

function activateTextMode() {
  var form = getFormRef();
  var textarea = getTextareaRef();
  if (form) form.classList.add("text-mode");
  if (textarea) textarea.focus();
}
function deactivateTextMode() {
  var form = getFormRef();
  var textarea = getTextareaRef();
  if (form) form.classList.remove("text-mode");
  if (textarea) textarea.value = "";
  var fileRadio = document.getElementById("mode-file");
  if (fileRadio) fileRadio.checked = true;
}

function onPaste(e) {
  if (document.documentElement.hasAttribute("data-backend-offline")) return;
  var target = e.target;
  if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)) return;
  var form = getFormRef();
  if (form && form.classList.contains("text-mode")) return;
  var files = e.clipboardData ? e.clipboardData.files : null;
  if (files && files.length > 0) {
    e.preventDefault();
    Array.from(files).forEach(startItem);
    return;
  }
  var text = e.clipboardData ? e.clipboardData.getData("text") : "";
  if (!text || !text.trim()) return;
  var trimmed = text.trim();
  if (/^https?:\/\/\S+$/i.test(trimmed) && isCobaltEnabled() && urlMatchesCobaltService(trimmed)) {
    e.preventDefault();
    var cobaltRadio = document.getElementById("mode-cobalt");
    var cobaltTextarea = getCobaltTextareaRef();
    if (cobaltRadio && cobaltTextarea) {
      cobaltTextarea.value = trimmed;
      cobaltRadio.checked = true;
      cobaltTextarea.focus();
      return;
    }
  }
  e.preventDefault();
  var textarea = getTextareaRef();
  if (textarea) textarea.value = text;
  activateTextMode();
}

export function initUploadCard() {
  var card = getCardRef();
  if (!card || card.hasAttribute("data-enhanced")) return;
  card.setAttribute("data-enhanced", "");
  listContentRef = card.querySelector(".file-list-content");
  emptyTextRef = card.querySelector(".empty-text");

  subscribe(function () {
    var uploads = getUploads();
    var present = new Map(uploads.map(function (u) {
      return [u.id, u];
    }));
    var removed = Array.from(prevSeen.entries())
      .filter(function (entry) {
        return !present.has(entry[0]) && wasRemoved(entry[0]);
      })
      .map(function (entry) {
        return entry[1];
      });
    if (removed.length && listContentRef) {
      listContentRef.querySelectorAll(".file-item:not(.exiting)").forEach(function (row) {
        var uid = row.getAttribute("data-upload-id");
        var fid = row.getAttribute("data-file-id") || "";
        var hit = removed.some(function (it) {
          return (uid && it.id === uid) || (fid && (it.serverId || extractServerId(it.url || "")) === fid);
        });
        if (hit) {
          if (uid) rowIds.delete(uid);
          animateRowOut(row);
        }
      });
    }
    for (var i = 0; i < uploads.length; i++) {
      if (!rowIds.has(uploads[i].id) && !suppressAdopt) adoptItem(uploads[i]);
      else syncRow(uploads[i]);
    }
    prevSeen = present;
    try {
      var sampleDone = 0;
      var sampleActive = 0;
      for (var k = 0; k < uploads.length; k++) {
        var u = uploads[k];
        if (u.state === "uploading" || u.state === "finalizing") {
          sampleDone += ((u.size || 0) * (u.progress || 0)) / 100;
          sampleActive++;
        }
      }
      window.dispatchEvent(
        new CustomEvent("jb-net-sample", {
          detail: {
            done: sampleDone,
            active: sampleActive,
            items: uploads.slice(0, 8).map(function (x) {
              return {
                id: x.id,
                n: x.filename,
                s: x.state,
                p: x.progress,
                m: x.method,
                z: x.size,
                d: x.dbg,
                o: {
                  mode: x.uploadMode,
                  quick: x.quickLink,
                  ttl: x.ttlHours,
                  host: x.customHost || "",
                },
              };
            }),
          },
        }),
      );
    } catch {}
  });

  try {
    connectUploads();
  } catch {}
  var preexisting = getUploads();
  for (var j = 0; j < preexisting.length; j++) adoptItem(preexisting[j]);

  var retentionDestroy;
  retentionDestroy = enhanceRetention(card, LOCALE);
  if (retentionDestroy && retentionDestroy.destroy) retentionDestroy = retentionDestroy.destroy;
  enhanceHostSelector(LOCALE);
  initHostSelector();
  connectPresence();
  initHostGlow();

  window.addEventListener("juicehost-config-updated", function () {
    retentionDestroy = rebuildRetention(card, LOCALE, retentionDestroy);
    if (retentionDestroy && retentionDestroy.destroy) retentionDestroy = retentionDestroy.destroy;
  });

  try {
    var params = new URLSearchParams(location.search);
    var uploaded = params.get("uploaded");
    if (uploaded) {
      // base64url payload from the server-side upload redirect; strip it from the URL after use.
      var json = JSON.parse(atob(uploaded.replace(/-/g, "+").replace(/_/g, "/")));
      addUploadedItem(sanitizeUploaded(json));
      history.replaceState(null, "", location.pathname);
    }
  } catch {}

  var form = getFormRef();
  var input = getInputRef();
  var dropZone = getDropZoneRef();
  if (form) form.addEventListener("submit", function (e) {
    // Until the file input is wired, let the form submit natively as the no-JS path.
    if (!document.documentElement.hasAttribute("data-upload-ready")) return;
    e.preventDefault();
  });
  enhanceServerFiles();

  if (input) {
    input.addEventListener("change", function () {
      if (!input.files) return;
      Array.from(input.files).forEach(startItem);
      input.value = "";
    });
    // Tells `boot.js` the real picker is wired so the no-JS fallback is not armed.
    document.documentElement.setAttribute("data-upload-ready", "");
    document.documentElement.removeAttribute("data-upload-fallback");
  }
  if (dropZone) dropZone.removeAttribute("for");

  if (dropZone) {
    dropZone.addEventListener(
      "click",
      function (e) {
        var target = e.target;
        if (target.tagName === "INPUT" || (target.closest && target.closest("input"))) return;
        e.preventDefault();
        e.stopPropagation();
        var inp = getInputRef();
        if (inp) {
          inp.value = "";
          inp.click();
        }
      },
      true,
    );
  }

  if (dropZone) {
    ["dragenter", "dragover"].forEach(function (n) {
      dropZone.addEventListener(n, function (e) {
        e.preventDefault();
        dropZone.classList.add("dragover");
      });
    });
    ["dragleave", "drop"].forEach(function (n) {
      dropZone.addEventListener(n, function (e) {
        e.preventDefault();
        dropZone.classList.remove("dragover");
      });
    });
    dropZone.addEventListener("drop", function (e) {
      if (!e.dataTransfer) return;
      var entries = folderZip.collectDropEntries(e.dataTransfer);
      if (entries.dirs.length) {
        e.preventDefault();
        e.stopPropagation();
        startFolderZip(entries.dirs, entries.files);
        return;
      }
      if (!e.dataTransfer.files) return;
      Array.from(e.dataTransfer.files).forEach(startItem);
    });
  }

  var textCancel = card.querySelector("[data-text-cancel]");
  if (textCancel) textCancel.addEventListener("click", deactivateTextMode);
  var textUpload = card.querySelector("[data-text-upload]");
  if (textUpload) {
    textUpload.addEventListener("click", function () {
      var textarea = getTextareaRef();
      var text = textarea ? textarea.value : "";
      if (!text || !text.trim()) return;
      startItem(new File([text], "paste.txt", { type: "text/plain" }));
      deactivateTextMode();
    });
  }

  var audioToggle = document.querySelector("[data-cobalt-audio-only]");
  var videoRows = [document.querySelector("[data-cobalt-quality-row]"), document.querySelector("[data-cobalt-container-row]")];
  var audioRows = [document.querySelector("[data-cobalt-better-audio-row]"), document.querySelector("[data-cobalt-format-row]")];
  if (audioToggle) {
    audioToggle.addEventListener("change", function () {
      for (var i = 0; i < videoRows.length; i++) {
        if (videoRows[i]) videoRows[i].hidden = audioToggle.checked;
      }
      for (var j = 0; j < audioRows.length; j++) {
        if (audioRows[j]) audioRows[j].hidden = !audioToggle.checked;
      }
    });
  }
  var cobaltCancel = card.querySelector("[data-cobalt-cancel]");
  if (cobaltCancel) {
    cobaltCancel.addEventListener("click", function () {
      var textarea = getCobaltTextareaRef();
      if (textarea) textarea.value = "";
      var fileRadio = document.getElementById("mode-file");
      if (fileRadio) fileRadio.checked = true;
    });
  }
  var cobaltFetch = card.querySelector("[data-cobalt-fetch]");
  if (cobaltFetch) {
    cobaltFetch.addEventListener("click", function () {
      cobaltFetchUrls();
    });
  }
  var cobaltTextarea = getCobaltTextareaRef();
  if (cobaltTextarea) {
    cobaltTextarea.addEventListener("keydown", handleCobaltKeydown);
  }

  document.addEventListener("paste", onPaste);
}

if (document.getElementById("upload-card")) {
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", initUploadCard);
  } else {
    initUploadCard();
  }
}
