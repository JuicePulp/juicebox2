document.documentElement.classList.remove("no-js");
document.documentElement.classList.add("js");

(function () {
  if (!document.documentElement || document.documentElement.dataset.live !== "1") return;
  if (typeof EventSource === "undefined") return;
  var KEY = "juicefront_boot_id";
  var COUNT_KEY = "juicefront_reloads";
  function seen() {
    try {
      return sessionStorage.getItem(KEY);
    } catch {
      return null;
    }
  }
  function store(id) {
    try {
      sessionStorage.setItem(KEY, id);
    } catch {}
  }
  function reloads() {
    try {
      var raw = sessionStorage.getItem(COUNT_KEY);
      if (!raw) return [];
      var list = JSON.parse(raw);
      if (!Array.isArray(list)) return [];
      var now = Date.now();
      return list.filter(function (t) {
        return now - t < 60000;
      });
    } catch {
      return [];
    }
  }
  function noteReload() {
    try {
      var list = reloads();
      list.push(Date.now());
      sessionStorage.setItem(COUNT_KEY, JSON.stringify(list));
    } catch {}
  }
  function connect() {
    if (reloads().length >= 5) return;
    var src;
    try {
      src = new EventSource("/__live");
    } catch {
      return;
    }
    src.addEventListener("boot", function (e) {
      var id = (e.data || "").trim();
      if (!id) return;
      var prev = seen();
      store(id);
      if (prev && prev !== id) {
        noteReload();
        location.reload();
      }
    });
    src.onerror = function () {
      try {
        src.close();
      } catch {}
    };
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", connect);
  } else {
    connect();
  }
})();
