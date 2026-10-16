  (function () {
    "use strict";
    var textview = document.getElementById("textview");
    if (textview) {
      var raw = textview.getAttribute("data-raw") || "";
      var stride = __TEXT_MAX__;
      var loaded = parseInt(textview.getAttribute("data-loaded") || "0", 10) || 0;
      var total = null;
      var loading = false;
      var done = false;
      var retriedPlain = false;
      var truncated = false;
      var decoder = new TextDecoder();
      var sentinel = document.createElement("div");
      sentinel.setAttribute("aria-hidden", "true");
      textview.after(sentinel);
      function parseTotal(res) {
        var cr = res.headers.get("Content-Range");
        if (cr) {
          var m = /\/(\d+)\s*$/.exec(cr);
          if (m) total = parseInt(m[1], 10);
        }
      }
      function fail() {
        if (loaded === 0) textview.textContent = "Could not load a text preview.";
        done = true;
      }
      async function loadPlain() {
        if (total !== null && total > 4 * 1024 * 1024) { fail(); return; }
        try {
          var res = await fetch(raw);
          if (!res.ok) throw new Error("bad status");
          var buf = new Uint8Array(await res.arrayBuffer());
          if (buf.length === 0) throw new Error("empty");
          textview.textContent = "";
          textview.appendChild(document.createTextNode(decoder.decode(buf)));
          loaded = buf.length;
          total = buf.length;
          done = true;
        } catch (e) {
          fail();
        } finally {
          loading = false;
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
          if (buf.length === 0) {
            loading = false;
            if (loaded === 0 && !retriedPlain) {
              retriedPlain = true;
              await loadPlain();
            } else {
              fail();
            }
            return;
          }
          var last = total !== null && loaded + buf.length >= total;
          var text = decoder.decode(buf, last ? undefined : { stream: true });
          if (loaded === 0) textview.textContent = "";
          textview.appendChild(document.createTextNode(text));
          loaded += buf.length;
          if (!truncated && loaded >= stride && total !== null && total > stride) {
            truncated = true;
            textview.appendChild(document.createTextNode("\n... (truncated preview, download for the full file)"));
            done = true;
          }
          if (last || (total !== null && loaded >= total)) done = true;
        } catch (e) {
          loading = false;
          if (loaded === 0 && !retriedPlain) {
            retriedPlain = true;
            await loadPlain();
          } else {
            fail();
          }
          return;
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
