import { t } from "./i18n.js";
import { announce, formatSize } from "./util.js";
import {
  iconHTML,
  iconForMime,
  extIcon,
  makeCopyBar,
  copyBarStrings,
  createDeleteButton,
  vanityMsg,
} from "./copy.js";
import {
  UPLOAD_URL,
  readMaxFileSize,
  TUS_THRESHOLD,
  ULTRAFAST_RESERVE_URL,
} from "./upload-config.js";
import { apiFetch, apiFetchJob, publicFileInfo } from "./api.js";
import { enhanceRetention, rebuildRetention } from "./enhance-retention.js";
import {
  enhanceHostSelector,
  readSelectedHost,
  readSelectedUploadMode,
  readQuickLinkEnabled,
  readDangerLevel,
  readUltraFastEnabled,
  readUltraFastSupported,
  initHostGlow,
} from "./enhance-host.js";
import { connectPresence, isAppMode } from "./presence.js";
import { validateFileClient } from "./file-validation.js";
import {
  enqueueUpload,
  cancelUpload,
  removeUpload,
  subscribe,
  getUploads,
  wasRemoved,
} from "./upload-client.js";
import { extractServerId } from "./upload-engine.js";
import { initHostSelector } from "./host-selector.js";

var LOCALE = (document.documentElement && document.documentElement.lang) || "en";
var rowIds = new Map();
var prevSeen = new Map();
var rowPartsCache = new WeakMap();
var suppressAdopt = 0;
var listContentRef = null;
var emptyTextRef = null;

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
function setEmpty(visible) {
  if (emptyTextRef) emptyTextRef.classList.toggle("visible", visible);
}
function removeItem(item) {
  var id = item.getAttribute("data-upload-id");
  if (id) {
    cancelUpload(id);
    rowIds.delete(id);
  }
  item.classList.add("exiting");
  setTimeout(function () {
    item.remove();
    setEmpty(!listContentRef || !listContentRef.querySelector(".file-item:not(.exiting)"));
  }, 300);
}
function failItem(item, pill, fill, status, errorCode, rawMessage) {
  item.classList.add("error");
  pill.classList.add("error");
  fill.classList.add("error");
  fill.classList.remove("finalizing");
  var spinner = item.querySelector(".spinner");
  if (spinner) spinner.remove();
  var msg = vanityMsg(LOCALE, errorCode, rawMessage);
  status.textContent = msg;
  announce(t(LOCALE, "upload.announce_failed", { message: msg }));
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
  getRowParts(row).pill.after(makeCopyBar(item.reserveUrl, copyBarStrings(LOCALE)));
}
function completeRow(item, row) {
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

  if (row.hasAttribute("data-quick-link")) {
    setTimeout(function () {
      var bar = pill.nextElementSibling;
      if (bar && bar.classList.contains("copy-bar") && row.isConnected) {
        pill.style.boxSizing = "border-box";
        pill.style.height = pill.offsetHeight + "px";
        pill.offsetHeight;
        pill.classList.add("sliding");
        bar.classList.add("copy-bar--rounded");
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
  autoCopyUrl(item.url || "", copyBar);

  var delBtn = row.querySelector(".delete-btn");
  var fileId = item.serverId || extractServerId(item.url || "") || item.id;
  if (delBtn && fileId && item.deleteToken) {
    delBtn.replaceWith(
      createDeleteButton({
        fileId: fileId,
        deleteToken: item.deleteToken,
        apiBase: UPLOAD_URL,
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

  showQuickLink(item, row);

  switch (item.state) {
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

async function ultrafastReserve(file, item, fill, status, pill) {
  status.textContent = t(LOCALE, "upload.delegating_to_app");
  try {
    var ttl = String(ttlHours());
    var reserveRes = await fetch(ULTRAFAST_RESERVE_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        filename: file.name,
        mime_type: file.type || "application/octet-stream",
        file_size: file.size,
        ttl_hours: Number(ttl),
      }),
    });
    if (!reserveRes.ok) {
      var code;
      var msg;
      try {
        var err = await reserveRes.json();
        if (err.error) code = err.error;
        if (err.message) msg = err.message;
      } catch {}
      failItem(item, pill, fill, status, code, msg);
      return;
    }
    var reserve = await reserveRes.json();
    var reserveUrl = reserve.url || "";
    try {
      var stored = JSON.parse(localStorage.getItem("juicebox_uploads") || "[]");
      stored.push({
        id: reserve.file_id || "",
        filename: file.name,
        mime_type: file.type || "application/octet-stream",
        size_bytes: file.size,
        uploaded_at: Math.floor(Date.now() / 1000),
        url: reserveUrl,
        expires_at: Math.floor(Date.now() / 1000) + Number(ttlHours()) * 3600,
        delete_token: reserve.delete_token || "",
      });
      localStorage.setItem("juicebox_uploads", JSON.stringify(stored));
    } catch {}
    if (reserveUrl) {
      item.setAttribute("data-quick-link", "");
      pill.after(makeCopyBar(reserveUrl, copyBarStrings(LOCALE)));
    }
    fill.style.setProperty("--progress", "0%");
    status.textContent = t(LOCALE, "upload.waiting_for_app");
    item.classList.add("delegated");
  } catch {
    failItem(item, pill, fill, status, "NETWORK_ERROR");
  }
}

function startUltraFastFromDropzone() {
  setEmpty(false);
  var item = document.createElement("div");
  item.className = "file-item";
  item.setAttribute("role", "listitem");
  item.innerHTML =
    '<div class="file-item-header">' +
    '<div class="file-card-icon">' + iconHTML("upload", 24) + "</div>" +
    '<div class="file-info-left">' +
    '<span class="file-name">Picking file on device...</span>' +
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
    '<span class="status-text">' + t(LOCALE, "upload.delegating_to_app") + "</span>" +
    "</div></div></div>";
  item.querySelector(".delete-btn").addEventListener("click", function () {
    removeItem(item);
  });
  listContentRef.prepend(item);
  var fill = item.querySelector(".progress-fill");
  var status = item.querySelector(".status-text");
  var pill = item.querySelector(".status-pill");
  ultrafastReservePlaceholder(item, fill, status, pill);
}

async function ultrafastReservePlaceholder(item, fill, status, pill) {
  status.textContent = t(LOCALE, "upload.delegating_to_app");
  try {
    var ttl = String(ttlHours());
    var reserveRes = await fetch(ULTRAFAST_RESERVE_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        filename: "upload",
        mime_type: "application/octet-stream",
        file_size: 0,
        ttl_hours: Number(ttl),
      }),
    });
    if (!reserveRes.ok) {
      var code;
      var msg;
      try {
        var err = await reserveRes.json();
        if (err.error) code = err.error;
        if (err.message) msg = err.message;
      } catch {}
      failItem(item, pill, fill, status, code, msg);
      return;
    }
    var reserve = await reserveRes.json();
    var reserveUrl = reserve.url || "";
    try {
      var stored = JSON.parse(localStorage.getItem("juicebox_uploads") || "[]");
      stored.push({
        id: reserve.file_id || "",
        filename: "upload",
        mime_type: "application/octet-stream",
        size_bytes: 0,
        uploaded_at: Math.floor(Date.now() / 1000),
        url: reserveUrl,
        expires_at: Math.floor(Date.now() / 1000) + Number(ttlHours()) * 3600,
        delete_token: reserve.delete_token || "",
      });
      localStorage.setItem("juicebox_uploads", JSON.stringify(stored));
    } catch {}
    if (reserveUrl) {
      item.setAttribute("data-quick-link", "");
      pill.after(makeCopyBar(reserveUrl, copyBarStrings(LOCALE)));
    }
    fill.style.setProperty("--progress", "0%");
    status.textContent = t(LOCALE, "upload.waiting_for_app");
    item.classList.add("delegated");
    if (reserve.file_id) {
      pollUltrafastStatus(reserve.file_id, item, fill, status, pill, reserve.delete_token || "");
    }
  } catch {
    failItem(item, pill, fill, status, "NETWORK_ERROR");
  }
}

function pollUltrafastStatus(fileId, item, fill, status, pill, deleteToken) {
  var INTERVAL = 1000;
  var HIDDEN_INTERVAL = 5000;
  var MAX_ATTEMPTS = 150;
  var attempts = 0;
  function nextDelay() {
    return typeof document !== "undefined" && document.hidden ? HIDDEN_INTERVAL : INTERVAL;
  }
  function poll() {
    if (attempts++ >= MAX_ATTEMPTS || !item.isConnected) return;
    fetch(UPLOAD_URL + publicFileInfo(fileId) + "?t=" + Date.now())
      .then(function (res) {
        if (!res.ok) {
          setTimeout(poll, nextDelay());
          return null;
        }
        return res.json();
      })
      .then(function (data) {
        if (!data) return;
        if (data.status === "ready") {
          var realName = data.filename || "upload";
          var realSize = data.size_bytes || 0;
          var realUrl = data.url || "";
          var realMime = data.mime_type || "";
          var nameEl = item.querySelector(".file-name");
          if (nameEl) nameEl.textContent = realName;
          var sizeEl = item.querySelector(".file-size");
          if (sizeEl) sizeEl.textContent = formatSize(realSize);
          var iconContainer = item.querySelector(".file-card-icon");
          if (iconContainer && realMime) {
            iconContainer.innerHTML = iconHTML(iconForMime(realMime), 24);
          }
          fill.classList.remove("finalizing");
          fill.style.setProperty("--progress", "100%");
          var progressDivider = fill.closest('[role="progressbar"]');
          if (progressDivider) progressDivider.setAttribute("aria-valuenow", "100");
          item.classList.add("complete");
          var actionArea = item.querySelector(".file-action-area");
          if (actionArea) actionArea.classList.add("complete");
          status.textContent = t(LOCALE, "upload.complete");
          var spinner = pill.querySelector(".spinner");
          if (spinner) spinner.remove();
          var existingCopyBar = pill.nextElementSibling;
          if (existingCopyBar && existingCopyBar.classList.contains("copy-bar")) {
            existingCopyBar.replaceWith(makeCopyBar(realUrl, copyBarStrings(LOCALE)));
          } else {
            pill.after(makeCopyBar(realUrl, copyBarStrings(LOCALE)));
          }
          pill.style.display = "none";
          var copyBar = pill.nextElementSibling;
          if (copyBar && copyBar.classList.contains("copy-bar")) {
            copyBar.style.marginTop = "0.75rem";
          }
          var delBtn = item.querySelector(".delete-btn");
          if (delBtn && fileId && deleteToken) {
            delBtn.replaceWith(
              createDeleteButton({
                fileId: fileId,
                deleteToken: deleteToken,
                apiBase: UPLOAD_URL,
                onDeleted: onDeleted,
                immediate: true,
              }),
            );
          }
          try {
            var stored = JSON.parse(localStorage.getItem("juicebox_uploads") || "[]");
            var idx = stored.findIndex(function (e) {
              return e.id === fileId;
            });
            if (idx >= 0) {
              stored[idx].filename = realName;
              stored[idx].mime_type = realMime;
              stored[idx].size_bytes = realSize;
              stored[idx].url = realUrl;
            } else {
              stored.push({
                id: fileId,
                filename: realName,
                mime_type: realMime,
                size_bytes: realSize,
                uploaded_at: Math.floor(Date.now() / 1000),
                url: realUrl,
                expires_at: data.expires_at || Math.floor(Date.now() / 1000) + ttlHours() * 3600,
                delete_token: deleteToken,
              });
            }
            localStorage.setItem("juicebox_uploads", JSON.stringify(stored));
          } catch {}
          announce(t(LOCALE, "upload.announce_complete", { filename: realName }));
          autoCopyUrl(realUrl, copyBar);
          return;
        }
        setTimeout(poll, nextDelay());
      })
      .catch(function () {
        setTimeout(poll, nextDelay());
      });
  }
  setTimeout(poll, nextDelay());
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
    removeItem(item);
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
        fill.classList.add("finalizing");
        status.textContent = t(LOCALE, "upload.cobalt_fetching");
        pollCobaltJob(start.job_id, item, fill, status, pill);
      });
    })
    .catch(function () {
      failItem(item, pill, fill, status, "NETWORK_ERROR");
    });
}

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
          status.textContent = t(LOCALE, "upload.cobalt_retry", { n: n });
        } else if (typeof data.bytes_received === "number" && data.bytes_received > 0) {
          status.textContent = t(LOCALE, "upload.cobalt_fetching") + " " + formatSize(data.bytes_received);
        } else if (data.stage === "session-fallback") {
          status.textContent = t(LOCALE, "upload.cobalt_relay");
        } else if (data.status !== "pending") {
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
  fill.style.setProperty("--progress", "100%");
  var progressDivider = fill.closest('[role="progressbar"]');
  if (progressDivider) progressDivider.setAttribute("aria-valuenow", "100");
  item.classList.add("complete");
  item.setAttribute("data-file-id", file.id);
  var actionArea = item.querySelector(".file-action-area");
  if (actionArea) actionArea.classList.add("complete");
  status.textContent = t(LOCALE, "upload.complete");
  var spinner = pill.querySelector(".spinner");
  if (spinner) spinner.remove();
  pill.style.display = "none";
  pill.after(makeCopyBar(file.url, copyBarStrings(LOCALE)));
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
        apiBase: UPLOAD_URL,
        onDeleted: onDeleted,
        immediate: true,
      }),
    );
  }
  try {
    var stored = JSON.parse(localStorage.getItem("juicebox_uploads") || "[]");
    stored.push({
      id: file.id,
      filename: file.filename,
      mime_type: file.mime_type,
      size_bytes: file.size_bytes,
      uploaded_at: Math.floor(Date.now() / 1000),
      url: file.url,
      expires_at: file.expires_at,
      delete_token: file.delete_token,
    });
    localStorage.setItem("juicebox_uploads", JSON.stringify(stored));
  } catch {}
  announce(t(LOCALE, "upload.announce_complete", { filename: file.filename }));
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
  item.querySelector(".delete-btn").addEventListener("click", function () {
    removeItem(item);
  });
  listContentRef.prepend(item);
  item.setAttribute("data-upload-id", data.id);
  rowIds.set(data.id, item);
  syncRow(data);
}

function startItem(file) {
  setEmpty(false);
  var item = buildRow(file.name, formatSize(file.size), extIcon(file.name));
  item.querySelector(".delete-btn").addEventListener("click", function () {
    removeItem(item);
  });
  listContentRef.prepend(item);
  var fill = item.querySelector(".progress-fill");
  var status = item.querySelector(".status-text");
  var pill = item.querySelector(".status-pill");

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
  if (isAppMode() && readUltraFastEnabled() && readUltraFastSupported()) {
    ultrafastReserve(file, item, fill, status, pill);
    return;
  }
  suppressAdopt++;
  var id;
  try {
    id = enqueueUpload({
      file: file,
      ttlHours: ttlHours(),
      customHost: readSelectedHost(),
      uploadMode: readSelectedUploadMode(),
      quickLink: readQuickLinkEnabled(),
    });
  } finally {
    suppressAdopt--;
  }
  item.setAttribute("data-upload-id", id);
  rowIds.set(id, item);
  status.textContent =
    file.size > TUS_THRESHOLD
      ? t(LOCALE, "upload.initializing_tus")
      : t(LOCALE, "upload.initializing");
}

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
  actionArea.appendChild(makeCopyBar(data.url, copyBarStrings(LOCALE)));
  if (data.id && data.delete_token) {
    actionArea.appendChild(
      createDeleteButton({
        fileId: data.id,
        deleteToken: data.delete_token,
        apiBase: UPLOAD_URL,
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
    if (link) link.replaceWith(makeCopyBar(url, copyBarStrings(LOCALE)));
    var noscript = el.querySelector("noscript");
    if (noscript) {
      noscript.replaceWith(
        createDeleteButton({
          fileId: id,
          deleteToken: token,
          apiBase: UPLOAD_URL,
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
      var json = JSON.parse(atob(uploaded.replace(/-/g, "+").replace(/_/g, "/")));
      addUploadedItem(sanitizeUploaded(json));
      history.replaceState(null, "", location.pathname);
    }
  } catch {}

  var form = getFormRef();
  var input = getInputRef();
  var dropZone = getDropZoneRef();
  if (form) form.addEventListener("submit", function (e) {
    e.preventDefault();
  });
  enhanceServerFiles();

  if (input) {
    input.addEventListener("change", function () {
      if (!input.files) return;
      Array.from(input.files).forEach(startItem);
      input.value = "";
    });
  }
  if (dropZone) dropZone.removeAttribute("for");

  if (input) {
    input.addEventListener(
      "click",
      function (e) {
        if (isAppMode() && readUltraFastEnabled() && readUltraFastSupported()) {
          e.preventDefault();
          e.stopPropagation();
          startUltraFastFromDropzone();
        }
      },
      true,
    );
  }
  if (dropZone) {
    dropZone.addEventListener(
      "click",
      function (e) {
        var target = e.target;
        if (target.tagName === "INPUT" || (target.closest && target.closest("input"))) return;
        e.preventDefault();
        e.stopPropagation();
        if (isAppMode() && readUltraFastEnabled() && readUltraFastSupported()) {
          startUltraFastFromDropzone();
        } else {
          var inp = getInputRef();
          if (inp) {
            inp.value = "";
            inp.click();
          }
        }
      },
      true,
    );
  }

  function syncDropzoneAppMode() {
    var full = isAppMode() && readUltraFastEnabled() && readUltraFastSupported();
    if (dropZone) dropZone.classList.toggle("drop-zone--app-mode", full);
  }
  syncDropzoneAppMode();
  window.addEventListener("juicebox-app-mode", syncDropzoneAppMode);
  window.addEventListener("juicehost-config-updated", syncDropzoneAppMode);
  var ufToggle = document.querySelector("[data-ultrafast-toggle]");
  if (ufToggle) ufToggle.addEventListener("change", syncDropzoneAppMode);

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
      if (!e.dataTransfer || !e.dataTransfer.files) return;
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
  var audioRows = [document.querySelector("[data-cobalt-format-row]"), document.querySelector("[data-cobalt-better-audio-row]")];
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
      var cobaltTextarea = getCobaltTextareaRef();
      var url = cobaltTextarea ? cobaltTextarea.value.trim() : "";
      if (!url) return;
      startCobaltFetch(url);
      if (cobaltTextarea) cobaltTextarea.value = "";
    });
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
