/* Archive browser: folder navigation + image lightbox, all client-side.
 * Rows carry data-path / data-dir / data-kind; the table carries the
 * archive id. No requests except lightbox image loads. */
(function () {
  "use strict";
  var table = document.getElementById("zipTable");
  if (!table) return;
  var archive = table.getAttribute("data-archive") || "";
  var rows = Array.prototype.slice.call(table.querySelectorAll("tbody tr"));
  var crumbs = document.getElementById("zipCrumbs");
  var prefix = "";

  function esc(s) {
    return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;");
  }

  function childOf(path) {
    if (path.indexOf(prefix) !== 0) return false;
    var rest = path.slice(prefix.length);
    if (!rest) return false;
    if (rest.charAt(rest.length - 1) === "/") rest = rest.slice(0, -1);
    return rest !== "" && rest.indexOf("/") === -1;
  }

  function render() {
    var shown = 0;
    rows.forEach(function (r) {
      var show = childOf(r.getAttribute("data-path") || "");
      r.style.display = show ? "" : "none";
      if (show) shown += 1;
    });
    if (crumbs) {
      var html = '<button type="button" data-prefix="">root</button>';
      var acc = "";
      prefix.split("/").filter(Boolean).forEach(function (seg) {
        acc += seg + "/";
        html += ' / <button type="button" data-prefix="' + esc(acc) + '">' + esc(seg) + "</button>";
      });
      crumbs.innerHTML = html + ' <span class="zip-count">(' + shown + ")</span>";
    }
  }

  if (crumbs) {
    crumbs.addEventListener("click", function (e) {
      var btn = e.target.closest("button[data-prefix]");
      if (!btn) return;
      prefix = btn.getAttribute("data-prefix") || "";
      render();
    });
  }

  var lightbox = document.getElementById("zipLightbox");
  var lightboxImg = document.getElementById("zipLightboxImg");
  var lightboxErr = document.getElementById("zipLightboxErr");
  function closeLightbox() {
    if (!lightbox) return;
    lightbox.hidden = true;
    if (lightboxImg) lightboxImg.removeAttribute("src");
  }
  if (lightboxImg) {
    lightboxImg.addEventListener("error", function () {
      if (lightboxErr) {
        lightboxErr.textContent = "Could not load that image (unsupported or locked entry).";
        lightboxErr.hidden = false;
      }
    });
  }
  if (lightbox) {
    lightbox.addEventListener("click", function (e) {
      if (e.target === lightbox || e.target.hasAttribute("data-close")) closeLightbox();
    });
  }
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") closeLightbox();
  });

  table.querySelector("tbody").addEventListener("click", function (e) {
    var tr = e.target.closest("tr");
    if (!tr) return;
    if (tr.getAttribute("data-dir") === "1") {
      prefix = tr.getAttribute("data-path") || "";
      render();
      return;
    }
    if (tr.getAttribute("data-kind") === "image" && lightbox && lightboxImg && archive) {
      if (lightboxErr) lightboxErr.hidden = true;
      lightboxImg.src =
        "/api/zipentry/" + encodeURIComponent(archive) + "?path=" + encodeURIComponent(tr.getAttribute("data-path") || "");
      lightbox.hidden = false;
    }
  });

  render();
})();
