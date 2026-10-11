

(function () {
  var BLOCKED_GRACE_MS = 4000;
  var WATCH_MAX_MS = BLOCKED_GRACE_MS + 30000;

  var cascadeFailed = false;

  function readZones() {
    var zones = [];
    var seen = {};
    function push(zone) {
      if (!zone || !zone.holder || !zone.key) return;
      if (seen[zone.holder]) return;
      seen[zone.holder] = true;
      zones.push(zone);
    }
    
    try {
      var cfg = document.querySelector("[data-ad-zones]");
      if (cfg) {
        var raw = cfg.getAttribute("data-ad-zones");
        if (raw) {
          var parsed = JSON.parse(raw);
          if (Array.isArray(parsed)) {
            parsed.forEach(function (z) {
              if (!z) return;
              push({
                holder: z.holder || z.id,
                key: z.key,
                format: z.format || "iframe",
                width: Number(z.width) || Number(z.w) || 0,
                height: Number(z.height) || Number(z.h) || 0
              });
            });
          }
        }
      }
    } catch (e) {}
    
    try {
      document.querySelectorAll("[data-box-key]").forEach(function (el) {
        var key = el.getAttribute("data-box-key");
        if (!key) return;
        var w = Number(el.getAttribute("data-box-w")) || 0;
        var h = Number(el.getAttribute("data-box-h")) || 0;
        push({
          holder: el.id,
          key: key,
          format: el.getAttribute("data-box-format") || "iframe",
          width: w,
          height: h
        });
      });
    } catch (e) {}
    
    return zones.filter(function (z) {
      if (!z.holder || !document.getElementById(z.holder)) return false;
      return true;
    });
  }

  function adFlagFromCookie() {
    try {
      return document.cookie.split("; ").some(function (c) {
        var eq = c.indexOf("=");
        return (
          eq > 0 &&
          c.slice(0, eq) === "ADTESTING" &&
          c.slice(eq + 1).toUpperCase() === "TRUE"
        );
      });
    } catch (e) {
      return false;
    }
  }

  function adTestEligible() {
    return adFlagFromCookie();
  }

  function injectZone(zone, zones, done) {
    var holder = document.getElementById(zone.holder);
    if (!holder || holder.dataset.boxInjected) {
      if (done) done();
      return;
    }
    holder.dataset.boxInjected = "1";
    holder.dataset.boxAt = String(Date.now());
    var opts = document.createElement("script");
    opts.textContent =
      "atOptions = { 'key' : '" + zone.key +
      "', 'format' : '" + zone.format +
      "', 'height' : " + zone.height +
      ", 'width' : " + zone.width +
      ", 'params' : {} };";
    var invoke = document.createElement("script");
    invoke.src = "https://www.highperformanceformat.com/" + zone.key + "/invoke.js";
    invoke.async = true;
    if (done) {
      invoke.onload = done;
      invoke.onerror = function () {
        
        cascadeFailed = true;
        markAllBlocked(zones);
        done();
      };
    }
    holder.appendChild(opts);
    holder.appendChild(invoke);
  }

  function markAllBlocked(zones) {
    zones.forEach(function (zone) {
      var holder = document.getElementById(zone.holder);
      if (!holder) return;
      if (holder.dataset.boxOk || adInHolder(holder)) return;
      holder.setAttribute("data-box-blocked", "");
      holder.removeAttribute("data-box-loading");
    });
  }

  function adInHolder(holder) {
    var iframe = holder.querySelector("iframe");
    if (iframe && iframe.offsetWidth > 0 && iframe.offsetHeight > 0) {
      return true;
    }
    var children = holder.children;
    for (var i = 0; i < children.length; i++) {
      var child = children[i];
      if (child.tagName === "SCRIPT") continue;
      if (
        child.classList &&
        (child.classList.contains("box-note") ||
          child.classList.contains("box-shimmer"))
      )
        continue;
      var rect = child.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) return true;
    }
    return false;
  }

  function markHolderLoaded(holder) {
    if (holder.dataset.boxOk) return;
    holder.dataset.boxOk = "1";
    holder.removeAttribute("data-box-blocked");
    holder.removeAttribute("data-box-loading");
  }

  function armLoadWatcher(holder) {
    if (typeof MutationObserver === "undefined") return;
    var obs = new MutationObserver(function () {
      var iframe = holder.querySelector("iframe");
      if (!iframe || iframe.dataset.boxIwatched) return;
      iframe.dataset.boxIwatched = "1";
      obs.disconnect();
      iframe.addEventListener("load", function () {
        if (!iframe.src || iframe.src.indexOf("about:") !== 0) markHolderLoaded(holder);
      });
    });
    obs.observe(holder, { childList: true, subtree: true });
  }

  function watchZones(zones) {
    if (!zones.length) return;
    var tick = setInterval(function () {
      zones.forEach(function (zone) {
        var holder = document.getElementById(zone.holder);
        if (!holder) return;

        if (holder.dataset.boxInjected && !holder.dataset.boxWatched) {
          holder.dataset.boxWatched = "1";
          holder.style.setProperty("--box-w", zone.width + "px");
          holder.style.setProperty("--box-h", zone.height + "px");
          holder.dataset.boxOrient =
            zone.width >= zone.height * 1.4 ? "wide" : "tall";
          holder.dataset.boxStarted =
            String(Number(holder.dataset.boxAt) || Date.now());
          armLoadWatcher(holder);
        }

        if (!holder.dataset.boxInjected || holder.dataset.boxOk) return;
        if (adInHolder(holder)) {
          markHolderLoaded(holder);
          return;
        }
        if (Date.now() - Number(holder.dataset.boxStarted) >= BLOCKED_GRACE_MS) {
          holder.setAttribute("data-box-blocked", "");
          holder.removeAttribute("data-box-loading");
        }
      });
    }, 1000);
    setTimeout(function () { clearInterval(tick); }, WATCH_MAX_MS + 5000);
  }

  function injectAds() {
    var zones = readZones();
    if (!zones.length) return;
    if (!adTestEligible()) return;
    cascadeFailed = false;
    var index = 0;
    function next() {
      if (cascadeFailed) return;
      if (index < zones.length) injectZone(zones[index++], zones, next);
    }
    next();

    watchZones(zones);
  }

  document.addEventListener("DOMContentLoaded", injectAds);
  injectAds();
})();
