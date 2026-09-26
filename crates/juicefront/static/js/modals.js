import { initPairModal } from "./pair.js";

(function () {
  var cfg = document.querySelector("[data-i18n-config]");
  if (!cfg || !cfg.dataset) return;
  var LOCALE_CODES = [];
  try {
    LOCALE_CODES = JSON.parse(cfg.dataset.localeCodes || "[]");
    if (!Array.isArray(LOCALE_CODES)) LOCALE_CODES = [];
  } catch (e) {
    return;
  }
  if (!LOCALE_CODES.length) return;
  var defaultLocale = cfg.dataset.defaultLocale || "en";
  var nonDefault = LOCALE_CODES.filter(function (c) { return c !== defaultLocale; });
  if (!nonDefault.length) return;
  function escapeRegExp(s) {
    return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  }
  var LOCALE_RE = new RegExp("^\\/(" + nonDefault.map(escapeRegExp).join("|") + ")(\\/|$)");
  var VALID = new Set(LOCALE_CODES);
  var lastFocus = null;

  var chordEl = document.getElementById("chord-indicator");
  var chordKeysEl = chordEl ? chordEl.querySelector(".chord-indicator__keys") : null;

  function localePath(path) {
    var locale = currentUrlLocale();
    return locale === "en" ? path : "/" + locale + path;
  }

  function navigateTo(path) {
    var a = document.createElement("a");
    a.href = localePath(path);
    document.body.appendChild(a);
    a.click();
    a.remove();
  }

  function adTestingOn() {
    try {
      return document.cookie.split("; ").some(function (c) {
        var eq = c.indexOf("=");
        return eq > 0 && c.slice(0, eq) === "ADTESTING" && c.slice(eq + 1).toUpperCase() === "TRUE";
      });
    } catch (e) {}
    return false;
  }

  function toggleAdTesting() {
    var on = adTestingOn();
    var next = on ? "false" : "true";
    try { document.cookie = "ADTESTING=" + next + "; path=/; max-age=31536000; samesite=lax"; } catch (e) {}
    try { localStorage.setItem("ADTESTING", next); } catch (e) {}
  }

  var CHORDS = {
    "g": {
      "h": function () { navigateTo("/"); },
      "f": function () { navigateTo("/files"); },
      "r": function () { navigateTo("/report"); },
      "d": function () { navigateTo("/docs"); },
      "s": function () { location.hash = "#share-modal"; },
      "l": function () { location.hash = "#language-modal"; },
      "t": function () { location.hash = "#host-modal"; },
      "a": {
        "d": {
          "e": function () { toggleAdTesting(); window.location.reload(); }
        }
      }
    }
  };

  function resolveChord(path) {
    var node = CHORDS;
    for (var i = 0; i < path.length; i++) {
      if (!node) return undefined;
      node = node[path[i]];
    }
    return node;
  }

  var chordPending = null;
  var chordTimer = null;
  var CHORD_TIMEOUT = 4000;

  var chordHideTimer = null;

  function showChordIndicator(keys) {
    if (!chordEl || !chordKeysEl) return;
    if (chordHideTimer) { clearTimeout(chordHideTimer); chordHideTimer = null; }
    chordKeysEl.innerHTML = "";
    keys.forEach(function (k) {
      var span = document.createElement("span");
      span.textContent = k;
      chordKeysEl.appendChild(span);
    });
    chordEl.hidden = false;
    chordEl.classList.add("chord-indicator--active");
  }

  function hideChordIndicator() {
    if (!chordEl) return;
    chordEl.classList.remove("chord-indicator--active");
    if (chordHideTimer) clearTimeout(chordHideTimer);
    chordHideTimer = setTimeout(function () {
      chordHideTimer = null;
      chordEl.hidden = true;
    }, 200);
  }

  function clearChord() {
    chordPending = null;
    if (chordTimer) { clearTimeout(chordTimer); chordTimer = null; }
    hideChordIndicator();
  }

  function currentUrlLocale() {
    var m = location.pathname.match(LOCALE_RE);
    return m ? m[1] : "en";
  }

  function stripLocale(p) {
    return p.replace(LOCALE_RE, "/") || "/";
  }

  function redirectToSavedLocale() {
    var saved = null;
    try { saved = localStorage.getItem("language"); } catch (e) {}
    if (!saved || !VALID.has(saved)) return;
    var urlLocale = currentUrlLocale();
    if (saved === urlLocale) return;
    var base = stripLocale(location.pathname);
    location.href = saved === "en" ? base : "/" + saved + (base === "/" ? "" : base);
  }

  function saveLanguageOnNav() {
    document.querySelectorAll("[data-lang-link]").forEach(function (a) {
      a.addEventListener("click", function () {
        try { localStorage.setItem("language", a.dataset.langLink); } catch (e) {}
      });
    });
  }

  function initShareCopy() {
    document.querySelectorAll("[data-share-copy]").forEach(function (btn) {
      btn.addEventListener("click", function () {
        if (!navigator.clipboard || !navigator.clipboard.writeText) return;
        navigator.clipboard.writeText(location.href).then(function () {
          var span = btn.querySelector("span");
          if (span) {
            var orig = span.textContent;
            span.textContent = "Copied!";
            setTimeout(function () { span.textContent = orig; }, 2000);
          }
        }).catch(function () {});
      });
    });
  }

  function initShareNative() {
    document.querySelectorAll("[data-share-native]").forEach(function (btn) {
      btn.addEventListener("click", function () {
        if (navigator.share) navigator.share({ title: "Juicebox", url: location.href }).catch(function () {});
      });
    });
  }

  function trapFocus(e, modal) {
    if (e.key !== "Tab") return;
    var focusable = modal.querySelectorAll(
      'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])'
    );
    if (!focusable.length) return;
    var first = focusable[0], last = focusable[focusable.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  }

  function updateFocus() {
    var hash = location.hash;
    var modal = hash.endsWith("-modal") ? document.getElementById(hash.slice(1)) : null;
    if (modal) {
      if (lastFocus !== document.activeElement) lastFocus = document.activeElement;
      var first = modal.querySelector(
        'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])'
      );
      if (first) first.focus();
      var handler = function (e) { trapFocus(e, modal); };
      // Store the handler on the modal so it can be removed deterministically
      // when the modal closes, even if no CSS transition runs.
      modal._focusTrapHandler = handler;
      document.addEventListener("keydown", handler);
      modal.addEventListener("transitionend", function () {
        if (modal._focusTrapHandler) {
          document.removeEventListener("keydown", modal._focusTrapHandler);
          modal._focusTrapHandler = null;
        }
      }, { once: true });
    } else {
      document.querySelectorAll(".mdloverlay[id$='-modal']").forEach(function (m) {
        if (m._focusTrapHandler) {
          document.removeEventListener("keydown", m._focusTrapHandler);
          m._focusTrapHandler = null;
        }
      });
      if (lastFocus) {
        lastFocus.focus();
        lastFocus = null;
      }
    }
  }

  function isTyping() {
    var t = document.activeElement;
    return t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || (t && t.isContentEditable);
  }

  function onKeydown(e) {
    if (e.key === "Escape") {
      if (chordPending) { clearChord(); return; }
      if (location.hash.endsWith("-modal")) { location.hash = "#!"; return; }
    }

    if (isTyping()) { clearChord(); return; }

    if (e.ctrlKey || e.metaKey) {
      if (e.key === "u" || e.key === "U") {
        e.preventDefault();
        var unavailable = document.documentElement.hasAttribute("data-banned") ||
          document.documentElement.hasAttribute("data-backend-offline");
        if (unavailable) {
          var dropzone = document.querySelector("[data-drop-zone]");
          if (dropzone) {
            dropzone.classList.remove("drop-zone--shake");
            void dropzone.offsetWidth;
            dropzone.classList.add("drop-zone--shake");
            dropzone.addEventListener("animationend", function () {
              dropzone.classList.remove("drop-zone--shake");
            }, { once: true });
          }
          return;
        }
        var input = document.getElementById("dropzone-input");
        if (input) {
          if (window.__juiceboxAppMode) {
            var appDropzone = document.querySelector("[data-drop-zone]");
            if (appDropzone) {
              appDropzone.click();
            }
          } else {
            input.value = "";
            input.click();
          }
        }
        return;
      }
      if (e.key === "c" || e.key === "C") {
        var sel = window.getSelection ? (window.getSelection() || "").toString() : "";
        if (!sel) {
          var copyBars = document.querySelectorAll(".copy-bar .copy-bar__url");
          var lastUrlEl = copyBars.length ? copyBars[copyBars.length - 1] : null;
          var fileUrl = lastUrlEl ? lastUrlEl.textContent : null;
          e.preventDefault();
          if (!navigator.clipboard || !navigator.clipboard.writeText) return;
          navigator.clipboard.writeText(fileUrl || location.href).then(function () {
            if (fileUrl) {
              var copyBar = lastUrlEl.closest(".copy-bar");
              if (copyBar) {
                copyBar.classList.add("copy-bar--copied");
                setTimeout(function () { copyBar.classList.remove("copy-bar--copied"); }, 2000);
              }
              showChordIndicator(["Copied!"]);
              setTimeout(hideChordIndicator, 1500);
            }
          }).catch(function () {});
        }
        return;
      }
    }

    var key = typeof e.key === "string" ? e.key.toLowerCase() : "";

    if (chordPending) {
      var path = chordPending.concat([key]);
      if (chordTimer) { clearTimeout(chordTimer); chordTimer = null; }
      var node = resolveChord(path);
      if (typeof node === "function") {
        chordPending = null;
        hideChordIndicator();
        e.preventDefault();
        node();
        return;
      }
      if (node) {
        chordPending = path;
        showChordIndicator(path.map(function (k) { return k.toUpperCase(); }));
        chordTimer = setTimeout(clearChord, CHORD_TIMEOUT);
        return;
      }
      clearChord();
      return;
    }

    var first = CHORDS[key];
    if (first && typeof first === "object") {
      chordPending = [key];
      showChordIndicator([key.toUpperCase()]);
      chordTimer = setTimeout(clearChord, CHORD_TIMEOUT);
      return;
    }

    if (e.key === "?") {
      location.hash = "#shortcuts-modal";
    }
  }

  function onClick(e) {
    var target = e.target;
    if (!target || !target.closest) return;
    if (target.closest("[data-select-backdrop]")) {
      var details = target.closest("[data-select-details]");
      if (details) details.removeAttribute("open");
      return;
    }
    document.querySelectorAll("details.select-details[open]").forEach(function (d) {
      if (!d.contains(target)) d.removeAttribute("open");
    });
  }

  function init() {
    chordEl = document.getElementById("chord-indicator");
    chordKeysEl = chordEl ? chordEl.querySelector(".chord-indicator__keys") : null;
    redirectToSavedLocale();
    saveLanguageOnNav();
    initShareCopy();
    initShareNative();
    updateFocus();
  }

  document.addEventListener("keydown", onKeydown);
  document.addEventListener("click", onClick);
  window.addEventListener("hashchange", updateFocus);
  document.addEventListener("DOMContentLoaded", init);
  document.addEventListener("click", function (e) {
    var t = e.target && e.target.closest ? e.target.closest("[data-share-select]") : null;
    if (t && typeof t.select === "function") {
      try {
        t.select();
      } catch (err) {}
    }
  });
  init();
})();

function initPairOnReady() {
  try {
    initPairModal();
  } catch (e) {}
}

document.addEventListener("DOMContentLoaded", initPairOnReady);
initPairOnReady();
