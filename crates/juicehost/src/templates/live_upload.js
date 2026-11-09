(function () {
  "use strict";
  var badge = document.getElementById("uploadBadge");
  var line = document.getElementById("uploadLine");
  var fill = document.getElementById("uploadFill");
  var stage = document.getElementById("stage");
  if (!badge || !line || !fill || !stage) return;
  if (badge.hasAttribute("hidden")) return;

  var fileId = stage.getAttribute("data-file-id") || "";
  if (!fileId) return;
  var timer = null;
  var stopped = false;

  function setProgress(pct, label) {
    pct = Math.max(0, Math.min(100, Number(pct) || 0));
    fill.style.width = pct + "%";
    badge.textContent = label;
  }

  function showEvent(data) {
    try {
      window.dispatchEvent(new CustomEvent("juicebox-upload-progress", { detail: data }));
    } catch (_) {}
  }

  async function poll() {
    if (stopped) return;
    try {
      var res = await fetch("/api/upload-progress/" + encodeURIComponent(fileId), {
        cache: "no-store"
      });
      if (!res.ok) {
        stopped = true;
        return;
      }
      var data = await res.json();
      if (data.ready) {
        setProgress(100, "Upload complete");
        stopped = true;
        setTimeout(function () { location.reload(); }, 120);
        return;
      }
      if (!data.uploading) {
        setProgress(0, "Upload unavailable");
        stopped = true;
        return;
      }
      var pct = Number(data.percent) || 0;
      var label = data.complete ? "Finalizing upload" : "Uploading " + pct + "%";
      setProgress(pct, label);
      showEvent(data);
    } catch (_) {
    }
    timer = setTimeout(poll, 1500);
  }

  window.addEventListener("beforeunload", function () {
    stopped = true;
    if (timer) clearTimeout(timer);
  });
  poll();
})();
