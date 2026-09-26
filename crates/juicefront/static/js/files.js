import { announce, esc, formatSize } from "./util.js";
import { iconHTML, iconForMime } from "./copy.js";
import { onFileDeleted, emitFileDeleted, removeLocalFile } from "./file-events.js";
import { t } from "./i18n.js";

function loc() {
  return document.documentElement.lang || "en";
}

function readDefaultHost() {
  try {
    return document.getElementById("server-config")?.getAttribute("data-public-base-url") || "";
  } catch {
    return "";
  }
}

function customIdEnabled() {
  try {
    return document.getElementById("server-config")?.getAttribute("data-custom-id") !== "false";
  } catch {
    return true;
  }
}

function stripHost(host) {
  return String(host || "").replace(/^https?:\/\//, "").replace(/\/$/, "");
}

function isDefaultHost(host, def) {
  if (!host) return true;
  const s = stripHost(host);
  if (s === "localhost:6402" || s === "127.0.0.1:6402" || s === "localhost:6400" || s === "127.0.0.1:6400") return true;
  if (def) return s === stripHost(def);
  return false;
}

/** Percentage of TTL remaining (0-100). */
function pctRemaining(expiresAt, uploadedAt) {
  const total = (expiresAt - uploadedAt) * 1000;
  if (total <= 0) return 0;
  return Math.max(0, Math.min(100, ((expiresAt * 1000 - Date.now()) / total) * 100));
}

function remainingLabel(expiresAt, locale) {
  if (!expiresAt || !Number.isFinite(expiresAt)) return "???";
  const left = expiresAt * 1000 - Date.now();
  if (left <= 0) return t(locale, "files.expired");
  const secs = Math.floor(left / 1000);
  const mins = Math.floor(secs / 60);
  const hours = Math.floor(mins / 60);
  const days = Math.floor(hours / 24);
  if (mins < 1) return t(locale, "files.expires", { time: secs + "s" });
  if (mins < 60) return t(locale, "files.expires", { time: mins + "m" });
  if (hours < 24) return t(locale, "files.expires", { time: hours + "h" });
  return t(locale, "files.expires", { time: days + "d" });
}

function normalizeFile(f) {
  let expires_at = f.expires_at;
  if (expires_at == null) {
    const old = f.expiresAt;
    expires_at = old ? (old > 1e11 ? Math.floor(old / 1000) : old) : 0;
  }
  let uploaded_at = f.uploaded_at;
  if (uploaded_at == null) {
    const old = f.uploadedAt;
    uploaded_at = old ? (old > 1e11 ? Math.floor(old / 1000) : old) : 0;
  }
  return {
    id: f.id,
    filename: f.filename ?? f.name ?? "",
    mime_type: f.mime_type ?? "",
    size_bytes: f.size_bytes ?? f.size ?? 0,
    uploaded_at,
    expires_at,
    url: f.url ?? "",
    delete_token: f.delete_token ?? "",
    storage_host: f.storage_host ?? "",
  };
}

function getLocalFiles() {
  try {
    const raw = localStorage.getItem("juicebox_uploads");
    if (raw) {
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed)) return parsed.map(normalizeFile);
    }
  } catch {}
  return [];
}

function trashIconHtml() {
  return iconHTML("trash", 24);
}

function fileIconHtml(mime) {
  return iconHTML(iconForMime(mime || ""), 24);
}

function editIconHtml() {
  return iconHTML("edit", 24);
}

function extractIdFromUrl(url, fallbackId) {
  if (!url) return fallbackId || "";
  try {
    const segment = new URL(url).pathname.split("/").pop() || "";
    const dot = segment.lastIndexOf(".");
    return (dot > 0 ? segment.slice(0, dot) : segment) || fallbackId || "";
  } catch {
    const segment = url.split("/").pop() || "";
    const dot = segment.lastIndexOf(".");
    return (dot > 0 ? segment.slice(0, dot) : segment) || fallbackId || "";
  }
}

function renameModalHtml(f, locale) {
  const currentId = extractIdFromUrl(f.url, f.id || "");
  return (
    '<div id="rename-' + esc(f.id) + '" class="mdloverlay mdl-target rename-modal" role="dialog" aria-modal="true" aria-labelledby="rename-title-' + esc(f.id) + '">' +
    '<a href="#!" class="mdl-target__backdrop" aria-label="' + esc(t(locale, "modal.close")) + '"></a>' +
    '<div class="mdlcontent"><div class="mdlheader"><h2 id="rename-title-' + esc(f.id) + '" class="mdltitle">' + esc(t(locale, "files.rename")) + '</h2>' +
    '<a href="#!" class="mdlclose" aria-label="' + esc(t(locale, "modal.close")) + '">×</a></div>' +
    '<form class="rename-form" method="POST" action="/file/' + esc(f.id) + '/rename"><div class="mdlbody">' +
    '<input type="hidden" name="token" value="' + esc(f.delete_token) + '">' +
    '<div class="form-field"><label class="form-label" for="rename-input-' + esc(f.id) + '">' + esc(t(locale, "files.rename_placeholder")) + '</label>' +
    '<div class="rename-input-group"><span class="rename-input-prefix">/f/</span>' +
    '<input id="rename-input-' + esc(f.id) + '" name="custom_id" class="form-input rename-input" value="' + esc(currentId) + '" maxlength="32" minlength="3" pattern="[-A-Za-z0-9_]+" autocomplete="off" spellcheck="false"></div>' +
    '<p class="rename-error" hidden></p></div></div>' +
    '<div class="mdlfooter rename-actions"><a href="#!" class="mdlbtn mdlbtn--secondary">' + esc(t(locale, "files.rename_cancel")) + '</a>' +
    '<button type="submit" class="mdlbtn mdlbtn--primary">' + esc(t(locale, "files.rename_save")) + '</button></div>' +
    "</form></div></div>"
  );
}

function makeCopyBar(url, locale) {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "copy-bar";
  btn.title = t(locale, "upload.copy_bar_title");
  btn.setAttribute("aria-label", t(locale, "upload.copy_bar_aria"));
  const wrap = document.createElement("div");
  wrap.className = "copy-bar__text-wrapper";
  const copied = document.createElement("span");
  copied.className = "copy-bar__copied-text";
  copied.textContent = t(locale, "upload.copy_bar_copied");
  const urlEl = document.createElement("span");
  urlEl.className = "copy-bar__url";
  urlEl.textContent = url;
  wrap.append(copied, urlEl);
  btn.append(wrap);
  btn.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(urlEl.textContent.trim() || url);
    } catch {}
    btn.classList.add("copy-bar--copied");
    setTimeout(() => btn.classList.remove("copy-bar--copied"), 2000);
    announce(t(locale, "upload.copy_bar_announce"));
  });
  return btn;
}

function updateEmptyState(card) {
  const grid = card.querySelector("[data-files-grid]");
  const empty = card.querySelector("[data-files-empty]");
  if (!grid) return;
  const hasCards = grid.querySelectorAll(".file-card").length > 0;
  if (empty) empty.hidden = hasCards;
  grid.hidden = !hasCards;
}

function flipRemove(el, card) {
  const container = el.parentElement;
  if (!container) {
    el.remove();
    updateEmptyState(card);
    return;
  }
  const siblings = Array.from(container.querySelectorAll(".file-card:not(.exiting)"));
  const first = siblings.map((s) => s.getBoundingClientRect());
  el.remove();
  requestAnimationFrame(() => {
    const last = siblings.map((s) => s.getBoundingClientRect());
    siblings.forEach((s, i) => {
      const dx = first[i].left - last[i].left;
      const dy = first[i].top - last[i].top;
      if (dx === 0 && dy === 0) return;
      s.style.transition = "none";
      s.style.transform = "translate(" + dx + "px, " + dy + "px)";
      requestAnimationFrame(() => {
        s.style.transition = "transform 0.25s cubic-bezier(0.4, 0, 0.2, 1)";
        s.style.transform = "";
        s.addEventListener("transitionend", () => { s.style.transition = ""; }, { once: true });
      });
    });
    updateEmptyState(card);
  });
}

function makeDeleteBtn(card, el, f, locale) {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "file-action-btn file-action-btn--danger";
  btn.title = t(locale, "files.delete");
  btn.setAttribute("aria-label", t(locale, "files.delete") + " file");
  btn.innerHTML = trashIconHtml() + '<span class="confirm-text"></span>';
  btn.addEventListener("click", async (e) => {
    if (e.shiftKey) {
      btn.dataset.confirming = "true";
      btn.classList.add("confirming");
    } else if (!btn.dataset.confirming) {
      btn.dataset.confirming = "true";
      btn.classList.add("confirming");
      const textSpan = btn.querySelector(".confirm-text");
      if (textSpan) textSpan.textContent = t(locale, "files.confirm");
      announce(t(locale, "files.confirm_sr"));
      return;
    }
    try {
      btn.innerHTML = '<img src="/static/assets/loading.webp" alt="" class="spinner-icon" width="18" height="18"><span class="confirm-text"></span>';
      btn.classList.add("spinning");
      if (f.delete_token) {
        const res = await fetch("/file/" + encodeURIComponent(f.id), {
          method: "DELETE",
          headers: { "X-Delete-Token": f.delete_token },
        });
        if (!res.ok) throw new Error("delete failed");
        removeLocalFile(f.id);
        emitFileDeleted(f.id);
      }
      el.classList.add("exiting");
      announce(t(locale, "sr.file_deleted"));
      setTimeout(() => flipRemove(el, card), 300);
    } catch {
      btn.dataset.confirming = "";
      btn.classList.remove("confirming", "spinning");
      btn.innerHTML = trashIconHtml() + '<span class="confirm-text"></span>';
      alert(t(locale, "files.delete_error"));
    }
  });
  return btn;
}

function wireRenameModal(card, modal, f, locale) {
  if (modal.dataset.wired) return;
  modal.dataset.wired = "1";
  const form = modal.querySelector(".rename-form");
  const input = modal.querySelector(".rename-input");
  let errorEl = modal.querySelector(".rename-error");
  if (!form || !input) return;
  if (!errorEl) {
    errorEl = document.createElement("p");
    errorEl.className = "rename-error";
    errorEl.hidden = true;
    input.closest(".form-field")?.append(errorEl);
  }
  const close = () => {
    modal.remove();
    if (location.hash.startsWith("#rename-")) history.replaceState(null, "", location.pathname + location.search);
  };
  modal.querySelector(".mdlbtn--secondary")?.addEventListener("click", (e) => { e.preventDefault(); close(); });
  modal.querySelector(".mdlclose")?.addEventListener("click", (e) => { e.preventDefault(); close(); });
  modal.querySelector(".mdl-target__backdrop")?.addEventListener("click", (e) => { e.preventDefault(); close(); });
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    errorEl.hidden = true;
    const customId = input.value.trim();
    try {
      const res = await fetch("/file/" + encodeURIComponent(f.id) + "/renew", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Delete-Token": f.delete_token },
        body: JSON.stringify({ custom_id: customId }),
      });
      if (!res.ok) {
        const data = await res.json().catch(() => ({}));
        errorEl.textContent = data.message || t(locale, "files.rename_invalid");
        errorEl.hidden = false;
        return;
      }
      const data = await res.json();
      f.id = data.id;
      f.url = data.url;
      const host = modal.closest(".file-card");
      if (host) {
        host.setAttribute("data-file-id", data.id);
        host.setAttribute("data-url", data.url);
        const urlEl = host.querySelector(".copy-bar .copy-bar__url");
        if (urlEl) urlEl.textContent = data.url;
        const link = host.querySelector(".copy-bar__text-wrapper")?.closest(".copy-bar");
        if (link) link.setAttribute("aria-label", t(locale, "upload.copy_bar_aria"));
        const editLink = host.querySelector(".copy-bar-edit");
        if (editLink) editLink.href = "#rename-" + data.id;
      }
      modal.remove();
      if (location.hash.startsWith("#rename-")) history.replaceState(null, "", location.pathname + location.search);
      announce(t(locale, "files.rename_success"));
    } catch {
      errorEl.textContent = t(locale, "files.rename_invalid");
      errorEl.hidden = false;
    }
  });
}

function showRenameModal(card, f, locale) {
  let modal = document.getElementById("rename-" + f.id);
  if (!modal) {
    const host = card.querySelector('[data-file-id="' + CSS.escape(f.id) + '"]');
    if (!host) return;
    host.insertAdjacentHTML("beforeend", renameModalHtml(f, locale));
    modal = document.getElementById("rename-" + f.id);
    if (modal) wireRenameModal(card, modal, f, locale);
  }
  location.hash = "#rename-" + f.id;
}

function enhanceCard(card, el, f, locale) {
  if (el.querySelector(".copy-bar")) return;
  const headerEl = el.querySelector(".file-card-header");
  const storageHost = el.getAttribute("data-storage-host") || f.storage_host || "";
  if (storageHost && !isDefaultHost(storageHost, readDefaultHost()) && headerEl && !headerEl.querySelector(".storage-host-tag")) {
    const tag = document.createElement("span");
    tag.className = "storage-host-tag";
    tag.textContent = t(locale, "files.stored_on", { host: stripHost(storageHost) });
    headerEl.append(tag);
  }
  const linkEl = el.querySelector(".file-card-link");
  const wrapper = el.querySelector(".copy-bar-wrapper");
  const copyBar = makeCopyBar(f.url || el.getAttribute("data-url") || "", locale);
  if (wrapper) {
    const existingLink = wrapper.querySelector(".file-card-link");
    if (existingLink) existingLink.replaceWith(copyBar);
    else wrapper.prepend(copyBar);
  } else if (linkEl) {
    const w = document.createElement("div");
    w.className = "copy-bar-wrapper";
    linkEl.replaceWith(w);
    w.append(copyBar);
  } else if (f.url || el.getAttribute("data-url")) {
    const w = document.createElement("div");
    w.className = "copy-bar-wrapper";
    w.append(copyBar);
    headerEl?.after(w);
  }
  if (customIdEnabled() && f.delete_token) {
    const w = el.querySelector(".copy-bar-wrapper");
    if (w && !w.querySelector(".copy-bar-edit")) {
      const a = document.createElement("a");
      a.href = "#rename-" + f.id;
      a.className = "copy-bar-edit";
      a.title = t(locale, "files.rename");
      a.setAttribute("aria-label", t(locale, "files.rename_aria"));
      a.innerHTML = "<span>" + editIconHtml() + "</span>";
      w.append(a);
    }
    const editBtn = w?.querySelector(".copy-bar-edit");
    if (editBtn && !editBtn.dataset.wired) {
      editBtn.dataset.wired = "1";
      editBtn.addEventListener("click", (e) => { e.preventDefault(); showRenameModal(card, f, locale); });
    }
    if (!el.querySelector("#rename-" + CSS.escape(f.id))) {
      el.insertAdjacentHTML("beforeend", renameModalHtml(f, locale));
    }
    const modal = el.querySelector("#rename-" + CSS.escape(f.id));
    if (modal) wireRenameModal(card, modal, f, locale);
  }
  const existingBtn = el.querySelector(".file-action-btn--danger");
  const fresh = makeDeleteBtn(card, el, f, locale);
  if (existingBtn) existingBtn.replaceWith(fresh);
  else el.querySelector(".file-card-buttons")?.append(fresh);
  const ttlLabel = el.querySelector(".file-card-ttl-label");
  const ttlEl = el.querySelector(".file-card-ttl");
  const fill = el.querySelector(".file-card-ttl-fill");
  const ttl = remainingLabel(f.expires_at, locale);
  if (ttlLabel) ttlLabel.textContent = ttl;
  if (fill) fill.style.width = pctRemaining(f.expires_at, f.uploaded_at || f.expires_at - 86400).toFixed(0) + "%";
  if (ttlEl) ttlEl.classList.toggle("expired", ttl === t(locale, "files.expired"));
}

function tickTtl(card, locale) {
  const grid = card.querySelector("[data-files-grid]");
  if (!grid || grid.hidden) return;
  grid.querySelectorAll(".file-card").forEach((el) => {
    const expires = Number(el.getAttribute("data-expires"));
    const uploaded = Number(el.getAttribute("data-uploaded")) || expires - 86400;
    if (!expires) return;
    const label = remainingLabel(expires, locale);
    const labelEl = el.querySelector(".file-card-ttl-label");
    const fill = el.querySelector(".file-card-ttl-fill");
    const ttlEl = el.querySelector(".file-card-ttl");
    if (labelEl) labelEl.textContent = label;
    if (fill) fill.style.width = pctRemaining(expires, uploaded).toFixed(0) + "%";
    if (ttlEl) ttlEl.classList.toggle("expired", label === t(locale, "files.expired"));
  });
}

function updateOffline(card) {
  const banner = card.querySelector("[data-files-offline]");
  if (banner) banner.hidden = !document.documentElement.hasAttribute("data-backend-offline");
}

const shiftState = { held: false, attached: false };
function attachShift(card) {
  if (shiftState.attached) return;
  shiftState.attached = true;
  const sync = () => card.classList.toggle("shift-delete", shiftState.held);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Shift" && !shiftState.held) { shiftState.held = true; sync(); }
  });
  document.addEventListener("keyup", (e) => {
    if (e.key === "Shift") { shiftState.held = false; sync(); }
  });
  window.addEventListener("blur", () => { shiftState.held = false; sync(); });
}

export function initFilesCard() {
  const card = document.querySelector("[data-files-card]");
  if (!card || card.hasAttribute("data-enhanced")) return;
  card.setAttribute("data-enhanced", "");
  const locale = loc();
  const grid = card.querySelector("[data-files-grid]");
  if (!grid) return;
  const localFiles = getLocalFiles();
  grid.querySelectorAll(".file-card").forEach((el) => {
    const id = el.getAttribute("data-file-id");
    if (!id) return;
    const match = localFiles.find((m) => m.id === id);
    const data = match || {
      id,
      delete_token: el.getAttribute("data-delete-token") || "",
      filename: el.getAttribute("data-filename") || "",
      size_bytes: Number(el.getAttribute("data-size")) || 0,
      uploaded_at: Number(el.getAttribute("data-uploaded")) || 0,
      expires_at: Number(el.getAttribute("data-expires")) || 0,
      url: el.getAttribute("data-url") || "",
      storage_host: el.getAttribute("data-storage-host") || "",
    };
    enhanceCard(card, el, data, locale);
  });
  updateEmptyState(card);
  updateOffline(card);
  tickTtl(card, locale);
  setInterval(() => {
    if (document.hidden) return;
    tickTtl(card, locale);
  }, 30000);
  attachShift(card);
  onFileDeleted((fileId) => {
    const el = grid.querySelector('[data-file-id="' + CSS.escape(fileId) + '"]');
    if (el) {
      el.remove();
      updateEmptyState(card);
    }
  });
  window.addEventListener("juicebox-app-mode", () => {});
}

function maybeInit() {
  if (document.querySelector("[data-files-card]")) initFilesCard();
}

if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", maybeInit);
else maybeInit();
