  (function () {
    "use strict";
    var textview = document.getElementById("textview");
    if (textview) {
      var raw = textview.getAttribute("data-raw") || "";
      var stride = __TEXT_MAX__;
      var loaded = 0;
      var total = null;
      var loading = false;
      var done = false;
      var decoder = new TextDecoder();
      var sentinel = document.createElement("div");
      sentinel.setAttribute("aria-hidden", "true");
      textview.after(sentin);
      function parseTotal(res) {
        var cr = res.headers.get("Content-Range");
        if (cr) {
          var m = /\/(\d+)\s*$/.exec(cr);
          if (m) total = parseInt(m[1], 10);
        }
      }
      async function loadMore() {
        if (loading || done) return;
        if (total !== null && loaded >= total) { done = true; return; }
        loading = true;
        try {
          var res = await fetch(raw, { headers: { "Range": "bytes=" + loaded + "-" + (loaded + stride - 1) } });
          if (res.status === 416) { done = true; return; }
          if (!res.ok && res.status !== 206) throw new Error("bad status");
          parseTotal(res);
          var buf = new Uint8Array(await res.arrayBuffer());
          if (buf.length === 0) { done = true; return; }
          var last = total !== null && loaded + buf.length >= total;
          var text = decoder.decode(buf, last ? undefined : { stream: true });
          if (loaded === 0) textview.textContent = "";
          textview.appendChild(document.createTextNode(text));
          loaded += buf.length;
          if (last || (total !== null && loaded >= total)) done = true;
        } catch (e) {
          if (loaded === 0) textview.textContent = "Could not load a text preview.";
          done = true;
        } finally {
          loading = false;
        }
      }
      if ("IntersectionObserver" in window) {
        new IntersectionObserver(function (entries) {
          if (entries.some(function (en) { return en.isIntersecting; })) loadMore();
        }, { rootMargin: "1200px" }).observe(sentinel);
      } else {
        var scroller = textview;
        scroller.addEventListener("scroll", function () {
          if (scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 1200) loadMore();
        });
      }
      loadMore();
    }
  })();
