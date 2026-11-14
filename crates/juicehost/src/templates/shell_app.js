  (function () {
    "use strict";
    var CFG = {
      id: "__FILE_ID__",
      gateway: "__GATEWAY_ORIGIN__",
      cipherUrl: "__CIPHERTEXT_URL__?inline=1",
      contentUrl: "__CONTENT_URL__",
      mode: "__MODE__",
      keyVersion: "__KEY_VERSION__",
      filename: "__FILENAME__",
      mime: "__MIME__",
      dlIcon: '__DL_ICON_SVG__',
      checkIcon: '__CHECK_ICON_SVG__'
    };
    var PLAIN_CHUNK = 65536, NONCE = 12, TAG = 16, HEADER = 13;
    var STORED_FULL = NONCE + PLAIN_CHUNK + TAG;
    var TEXT_PREVIEW_MAX = 262144;

    var statusEl = document.getElementById("status");
    var barEl = document.getElementById("bar");
    var progressEl = document.getElementById("progress");
    var btn = document.getElementById("unlockbtn");
    var pwInput = document.getElementById("password");
    var stage = document.getElementById("stage");
    var card = document.getElementById("unlockcard");
    var topbar = document.getElementById("topbar");
    var topdl = document.getElementById("topdl");
    var topraw = document.getElementById("topraw");

    
    
    var held = null;

    function enableTopbar() {
      if (topdl) topdl.disabled = false;
      if (topraw) topraw.disabled = false;
    }
    if (topdl) topdl.addEventListener("click", function () {
      if (!held) return;
      say("Downloading...");
      downloadHeld(held).catch(function (e) { say(e.message, true); });
    });
    if (topraw) topraw.addEventListener("click", function () {
      if (!held) return;
      openHeld(held).catch(function (e) { say(e.message, true); });
    });

    function say(msg, err) {
      statusEl.textContent = msg;
      statusEl.classList.toggle("err", !!err);
    }
    function progress(frac) {
      progressEl.hidden = false;
      barEl.style.width = Math.round(frac * 100) + "%";
    }
    function unreachable(e) {
      return "Server unreachable. The file service may be down, or the browser blocked the request (the storage host origin must be in the backend CORS list).";
    }
    function b64ToBytes(b64) {
      var bin = atob(b64);
      var out = new Uint8Array(bin.length);
      for (var i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
      return out;
    }
    // The storage host caps a single range response (max_range_response_mb),
    // so large ciphertext reads must be assembled from several requests.
    var RANGE_CHUNK_MAX = 8 * 1024 * 1024;
    function cacheBusted(url) {
      var sep = url.indexOf("?") >= 0 ? "&" : "?";
      return url + sep + "_=" + Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
    }
    async function fetchRange(url, start, len, extraHeaders) {
      if (len === 0) return new Uint8Array(0);
      var out = new Uint8Array(len);
      var filled = 0;
      while (filled < len) {
        var chunkStart = start + filled;
        var chunkEnd = Math.min(start + len - 1, chunkStart + RANGE_CHUNK_MAX - 1);
        var headers = {};
        if (extraHeaders) {
          for (var k in extraHeaders) {
            if (Object.prototype.hasOwnProperty.call(extraHeaders, k)) headers[k] = extraHeaders[k];
          }
        }
        headers["Range"] = "bytes=" + chunkStart + "-" + chunkEnd;
        var res = await fetch(cacheBusted(url), { headers: headers, cache: "no-store" });
        if (res.status === 206) {
          var bytes = new Uint8Array(await res.arrayBuffer());
          if (bytes.length === 0) throw new Error("ciphertext truncated");
          var take = Math.min(bytes.length, len - filled);
          out.set(bytes.subarray(0, take), filled);
          filled += take;
          continue;
        }
        if (res.status === 200 && chunkStart === 0) {
          var whole = new Uint8Array(await res.arrayBuffer());
          var takeWhole = Math.min(whole.length, len);
          out.set(whole.subarray(0, takeWhole), 0);
          filled = takeWhole;
          continue;
        }
        throw new Error("range request failed: " + res.status);
      }
      return out;
    }
    function parseHeader(bytes) {
      if (bytes.length < HEADER || bytes[0] !== 0x4A || bytes[1] !== 0x42 || bytes[2] !== 0x43 || bytes[3] !== 0x31 || bytes[4] !== 16) {
        throw new Error("not a Juicebox encrypted file");
      }
      var view = new DataView(bytes.buffer, bytes.byteOffset + 5, 8);
      var lo = view.getUint32(0, true), hi = view.getUint32(4, true);
      return hi * 4294967296 + lo;
    }
    function cipherRangeForPlain(start, end, plainLen) {
      if (plainLen === 0) return [HEADER, NONCE + TAG];
      if (start === end) return [HEADER, 0];
      var first = Math.floor(start / PLAIN_CHUNK);
      var last = Math.floor((end - 1) / PLAIN_CHUNK);
      var cipherStart = HEADER + first * STORED_FULL;
      var lastPlain = Math.min(plainLen - last * PLAIN_CHUNK, PLAIN_CHUNK);
      var cipherLen = (last - first) * STORED_FULL + (NONCE + lastPlain + TAG);
      return [cipherStart, cipherLen];
    }
    async function decryptChunk(key, stored) {
      var iv = stored.slice(0, NONCE);
      var data = stored.slice(NONCE);
      var plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv: iv }, key, data);
      return new Uint8Array(plain);
    }
    
    
    
    async function decryptRange(key, plainStart, plainEnd, plainLen, onChunk) {
      var range = cipherRangeForPlain(plainStart, plainEnd, plainLen);
      var cipherBytes = await fetchRange(CFG.cipherUrl, range[0], range[1]);
      var firstIdx = Math.floor(plainStart / PLAIN_CHUNK);
      var plainOff = firstIdx * PLAIN_CHUNK;
      var cursor = 0;
      while (plainOff < plainEnd) {
        var want = Math.min(plainLen - plainOff, PLAIN_CHUNK);
        var take = NONCE + want + TAG;
        var stored = cipherBytes.slice(cursor, cursor + take);
        cursor += take;
        if (stored.length !== take) throw new Error("ciphertext truncated");
        var plain = await decryptChunk(key, stored);
        var from = Math.max(0, plainStart - plainOff);
        var to = Math.min(want, plainEnd - plainOff);
        if (from < to) await onChunk(plain.slice(from, to), plainOff + from);
        plainOff += want;
      }
    }
    async function importDek(b64) {
      return crypto.subtle.importKey("raw", b64ToBytes(b64), { name: "AES-GCM" }, false, ["decrypt"]);
    }
    var TRACKER_EXTENSIONS = " mod xm s3m it mptm mtm stm med 669 far okt ult mo3 amf dsm dbm umx mt2 psm j2b gdm imf stx ptm sfx wow ";
    var playerModule = null;
    function extensionOf(name) {
      var dot = (name || "").lastIndexOf(".");
      return dot >= 0 ? name.slice(dot + 1).toLowerCase() : "";
    }
    function loadJuicePlayer() {
      if (window.customElements && customElements.get("juice-player")) return Promise.resolve();
      if (!playerModule) {
        playerModule = import("/api/player/index.js?v=20261114c").catch(function (err) {
          playerModule = null;
          throw err;
        });
      }
      return playerModule;
    }
    function mountNativeBlob(blob, kind) {
      var url = URL.createObjectURL(blob);
      if (kind === "image" || kind === "animation") {
        var img = document.createElement("img");
        img.id = "media"; img.src = url; img.alt = CFG.filename;
        stage.appendChild(img);
      } else if (kind === "video") {
        var video = document.createElement("video");
        video.id = "media"; video.controls = true; video.preload = "metadata"; video.src = url;
        stage.appendChild(video);
      } else if (kind === "audio") {
        var audio = document.createElement("audio");
        audio.id = "media"; audio.controls = true; audio.preload = "metadata"; audio.src = url;
        stage.appendChild(audio);
      } else if (kind === "pdf") {
        var frame = document.createElement("iframe");
        frame.id = "media"; frame.className = "pdf-frame"; frame.src = url; frame.title = CFG.filename;
        stage.appendChild(frame);
      }
    }
    async function showBlob(blob, kind) {
      card.hidden = true;
      if (kind === "video" || kind === "audio" || kind === "flash" || kind === "midi" || kind === "tracker" || kind === "animation") {
        var player = null;
        var nativeMounted = false;
        var mountNative = function () {
          if (nativeMounted) return;
          nativeMounted = true;
          if (player && player.isConnected) player.remove();
          mountNativeBlob(blob, kind);
        };
        try {
          await Promise.race([
            loadJuicePlayer(),
            new Promise(function (_, reject) {
              setTimeout(function () { reject(new Error("JuicePlayer did not load.")); }, 15000);
            })
          ]);
          player = document.createElement("juice-player");
          player.id = "media";
          player.className = "juice-player";
          player.setAttribute("worker-url", "/api/player/decoder.worker.js?v=20261114c");
          player.setAttribute("soundfont-url", "/api/player/gm.sf2");
          player.setAttribute("ruffle-url", "/api/player/ruffle/ruffle.js");
          player.setAttribute("hide-empty", "");
          player.hideEmpty = true;
          player.addEventListener("error", function (event) {
            var detail = event && event.detail;
            if (detail && detail.fatal === false) return;
            if (kind === "flash" || kind === "midi" || kind === "tracker") return;
            mountNative();
          });
          stage.appendChild(player);
          var file = new File([blob], CFG.filename || "file", { type: blob.type || CFG.mime });
          await player.loadFile(file);
          return;
        } catch (e) {
          if (player && player.isConnected) player.remove();
          if (kind === "flash" || kind === "midi" || kind === "tracker") throw e;
          mountNative();
          return;
        }
      }
      mountNativeBlob(blob, kind);
    }
    function showText(text) {
      card.hidden = true;
      var pre = document.createElement("pre");
      pre.className = "text-view";
      pre.textContent = text;
      stage.appendChild(pre);
    }
    function showRawBlob(blob) {
      window.location.replace(URL.createObjectURL(blob));
    }
    function kindOf(mime, filename) {
      var ext = extensionOf(filename || CFG.filename || "");
      if (ext === "swf") return "flash";
      if (ext === "mid" || ext === "midi" || ext === "rmi" || ext === "kar") return "midi";
      if (TRACKER_EXTENSIONS.indexOf(" " + ext + " ") >= 0) return "tracker";
      if (ext === "gif" || ext === "apng" || ext === "webp" || ext === "avif") return "animation";
      var base = (mime || "").split(";")[0].trim();
      if (base.indexOf("video/") === 0) return "video";
      if (base.indexOf("audio/") === 0) return "audio";
      if (base.indexOf("image/") === 0) return "image";
      if (base === "application/pdf") return "pdf";
      if (base.indexOf("text/") === 0 || base === "application/json") return "text";
      return "download";
    }
    async function saveDownload(getStream) {
      if (window.showSaveFilePicker) {
        try {
          var handle = await window.showSaveFilePicker({ suggestedName: CFG.filename });
          var writable = await handle.createWritable();
          await getStream(writable);
          await writable.close();
          return true;
        } catch (e) {
          if (e && e.name === "AbortError") { say("Save cancelled."); return false; }
          
        }
      }
      var parts = [];
      await getStream({ write: async function (chunk) { parts.push(chunk); }, close: async function () {} });
      var blob = new Blob(parts, { type: CFG.mime || "application/octet-stream" });
      var a = document.createElement("a");
      a.href = URL.createObjectURL(blob);
      a.download = CFG.filename;
      document.body.appendChild(a);
      a.click();
      setTimeout(function () { URL.revokeObjectURL(a.href); a.remove(); }, 4000);
      return true;
    }

    async function v1Unlock(password) {
      var paramsRes;
      try {
        paramsRes = await fetch(CFG.gateway + "/api/gateway/params/" + encodeURIComponent(CFG.id), { cache: "no-store" });
      } catch (e) { throw new Error(unreachable(e)); }
      if (paramsRes.status === 404) throw new Error("File not found or no longer protected.");
      if (!paramsRes.ok) throw new Error("Could not load file parameters (" + paramsRes.status + ").");
      var params = await paramsRes.json();
      if (params.key_version !== "1") throw new Error("Unexpected file version.");
      var res;
      try {
        res = await fetch(CFG.gateway + "/api/gateway/unlock", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ id: CFG.id, password: password }),
          cache: "no-store"
        });
      } catch (e) { throw new Error(unreachable(e)); }
      if (res.status === 403) throw new Error("Wrong password.");
      if (res.status === 429) throw new Error("Too many attempts. Wait and try again.");
      if (!res.ok) throw new Error("Unlock failed (" + res.status + ").");
      var body = await res.json();
      var key = await importDek(body.dek);
      return { key: key, plainLen: body.plain_len, filename: body.filename || CFG.filename, mime: body.mime_type || CFG.mime };
    }

    function beginDownload() {
      if (CFG.mode !== "download" || !card) return;
      card.hidden = false;
      card.classList.remove("done");
      card.classList.add("is-downloading");
      say("Downloading...");
    }
    function cancelDownload() {
      if (CFG.mode !== "download" || !card) return;
      card.classList.remove("is-downloading");
    }
    function downloadComplete() {
      if (CFG.mode !== "download") { say("Download complete."); return; }
      if (!card) return;
      card.hidden = false;
      card.classList.remove("is-downloading");
      card.classList.add("done");
      card.innerHTML = '<div class="done-check" role="status" aria-label="Download complete">' + CFG.checkIcon + '</div>';
    }

    async function v1Download(key, plainLen) {
      beginDownload();
      var totalChunks = Math.max(1, Math.ceil(plainLen / PLAIN_CHUNK));
      var done = 0;
      var saved = await saveDownload(async function (writable) {
        if (plainLen === 0) {
          var stored = await fetchRange(CFG.cipherUrl, HEADER, NONCE + TAG);
          await decryptChunk(key, stored);
          progress(1);
        }
        for (var i = 0; i < totalChunks && plainLen !== 0; i++) {
          var chunkStart = i * PLAIN_CHUNK;
          var chunkEnd = Math.min(plainLen, chunkStart + PLAIN_CHUNK);
          var range = cipherRangeForPlain(chunkStart, chunkEnd, plainLen);
          var stored = await fetchRange(CFG.cipherUrl, range[0], range[1]);
          var plain = await decryptChunk(key, stored);
          await writable.write(plain.slice(0, chunkEnd - chunkStart));
          done++;
          progress(done / totalChunks);
        }
      });
      if (!saved) { cancelDownload(); return; }
      downloadComplete();
    }

    
    
    async function decryptFull(key, plainLen) {
      var parts = [];
      var done = 0;
      var totalChunks = Math.max(1, Math.ceil(plainLen / PLAIN_CHUNK));
      await decryptRange(key, 0, plainLen, plainLen, async function (bytes) {
        parts.push(bytes);
        done++;
        progress(done / totalChunks);
      });
      return new Blob(parts, { type: CFG.mime || "application/octet-stream" });
    }

    function fmtSize(bytes) {
      if (bytes >= 1099511627776) return (bytes / 1099511627776).toFixed(1) + " TB";
      if (bytes >= 1073741824) return (bytes / 1073741824).toFixed(1) + " GB";
      if (bytes >= 1048576) return (bytes / 1048576).toFixed(1) + " MB";
      if (bytes >= 1024) return (bytes / 1024).toFixed(1) + " KB";
      return bytes + " B";
    }

    
    
    function renderDlHero(mimeLabel, sizeLabel, onDownload) {
      card.hidden = true;
      var hero = document.createElement("div");
      hero.className = "dl-hero";
      var iconWrap = document.createElement("div");
      iconWrap.className = "dl-icon";
      iconWrap.setAttribute("aria-hidden", "true");
      try { iconWrap.innerHTML = CFG.dlIcon; } catch (e) {}
      var name = document.createElement("p");
      name.className = "dl-name";
      name.textContent = CFG.filename;
      var meta = document.createElement("p");
      meta.className = "dl-meta";
      meta.textContent = mimeLabel + " - " + sizeLabel;
      var dlBtn = document.createElement("a");
      dlBtn.className = "dl-btn";
      dlBtn.href = "#";
      dlBtn.textContent = "Download";
      dlBtn.addEventListener("click", function (e) {
        e.preventDefault();
        say("Downloading...");
        onDownload().catch(function (err) { say(err.message, true); });
      });
      hero.appendChild(iconWrap);
      hero.appendChild(name);
      hero.appendChild(meta);
      hero.appendChild(dlBtn);
      stage.appendChild(hero);
      say("");
    }

    
    
    async function v1View(h) {
      if (h.plainLen > 512 * 1024 * 1024) {
        say("File is too large to open in the browser. Download it instead.", true);
        await v1Download(h.key, h.plainLen);
        return;
      }
      say("Decrypting...");
      showRawBlob(await decryptFull(h.key, h.plainLen));
    }

    
    
    
    
    
    
    async function renderNative(blob, kind, withDownload) {
      card.hidden = true;
      if (topbar && !withDownload) topbar.style.display = "none";
      document.title = CFG.filename + " - Juicebox²";
      stage.style.padding = "0";
      if (kind === "text") {
        blob.text().then(function (text) {
          var pre = document.createElement("pre");
          pre.className = "text-view";
          pre.style.height = "100%";
          pre.textContent = text;
          stage.appendChild(pre);
          if (withDownload) addFloatDl(withDownload);
          say("");
        }).catch(function () { say("Could not render the file.", true); });
        return;
      }
      await showBlob(blob, kind);
      var media = document.getElementById("media");
      if (media) { media.style.width = "100%"; media.style.height = "100%"; }
      if (withDownload) addFloatDl(withDownload);
      say("");
    }

    function addFloatDl(onClick) {
      var old = document.getElementById("floatdl");
      if (old) old.remove();
      var b = document.createElement("button");
      b.id = "floatdl";
      b.className = "float-dl";
      b.textContent = "Download";
      b.addEventListener("click", function () {
        say("Downloading...");
        onClick().catch(function (e) { say(e.message, true); });
      });
      document.body.appendChild(b);
    }

    
    async function downloadHeld(h) {
      if (h.v1) return v1Download(h.key, h.plainLen);
      beginDownload();
      var res = await fetch(CFG.contentUrl, { headers: { "X-File-Password": h.password }, cache: "no-store" });
      if (res.status === 403) throw new Error("Wrong password.");
      if (!res.ok) throw new Error("Download failed (" + res.status + ").");
      var blob = await res.blob();
      var a = document.createElement("a");
      a.href = URL.createObjectURL(blob);
      a.download = CFG.filename;
      document.body.appendChild(a);
      a.click();
      setTimeout(function () { URL.revokeObjectURL(a.href); a.remove(); }, 4000);
      downloadComplete();
    }

    async function openHeld(h) {
      if (h.v1) {
        if (h.plainLen > 512 * 1024 * 1024) throw new Error("File is too large to open in the browser.");
        say("Decrypting...");
        await renderNative(await decryptFull(h.key, h.plainLen), kindOf(CFG.mime, CFG.filename), false);
        return;
      }
      say("Opening...");
      var res = await fetch(CFG.contentUrl, { headers: { "X-File-Password": h.password }, cache: "no-store" });
      if (res.status === 403) throw new Error("Wrong password.");
      if (!res.ok) throw new Error("Open failed (" + res.status + ").");
      await renderNative(await res.blob(), kindOf(CFG.mime, CFG.filename), false);
    }

    async function v1Preview(key, plainLen) {
      var kind = kindOf(CFG.mime, CFG.filename);
      if (kind === "text") {
        var end = Math.min(plainLen, TEXT_PREVIEW_MAX);
        if (end === 0) { showText(""); say(""); return; }
        var chunks = [];
        await decryptRange(key, 0, end, plainLen, async function (bytes) { chunks.push(bytes); });
        var total = chunks.reduce(function (n, c) { return n + c.length; }, 0);
        var merged = new Uint8Array(total);
        var off = 0;
        chunks.forEach(function (c) { merged.set(c, off); off += c.length; });
        var text = new TextDecoder().decode(merged);
        if (plainLen > end) text += "\n... (truncated preview, download for the full file)";
        showText(text);
        say("");
        return;
      }
      if (kind === "download") {
        renderDlHero(CFG.mime || "application/octet-stream", fmtSize(plainLen), function () {
          return v1Download(key, plainLen);
        });
        return;
      }
      if (plainLen > 512 * 1024 * 1024 && (kind === "video" || kind === "audio")) {
        renderDlHero(CFG.mime || "application/octet-stream", fmtSize(plainLen), function () {
          return v1Download(key, plainLen);
        });
        return;
      }
      say("Decrypting...");
      await showBlob(await decryptFull(key, plainLen), kind);
      say("");
    }

    async function legacyFlow(password) {
      var headers = { "X-File-Password": password };
      if (CFG.mode === "download") {
        beginDownload();
        say("Downloading...");
        var res;
        try {
          res = await fetch(CFG.contentUrl, { headers: headers, cache: "no-store" });
        } catch (e) { throw new Error(unreachable(e)); }
        if (res.status === 403) throw new Error("Wrong password.");
        if (!res.ok) throw new Error("Download failed (" + res.status + ").");
        var blob = await res.blob();
        var a = document.createElement("a");
        a.href = URL.createObjectURL(blob);
        a.download = CFG.filename;
        document.body.appendChild(a);
        a.click();
        setTimeout(function () { URL.revokeObjectURL(a.href); a.remove(); }, 4000);
        downloadComplete();
        return;
      }
      var kind = kindOf(CFG.mime, CFG.filename);
      if (CFG.mode === "view") {
        say("Opening...");
        var r0 = await fetch(CFG.contentUrl, { headers: headers, cache: "no-store" });
        if (r0.status === 403) throw new Error("Wrong password.");
        if (!r0.ok) throw new Error("Open failed (" + r0.status + ").");
        var blob0 = await r0.blob();
        if (blob0.size > 512 * 1024 * 1024) {
          say("File is too large to open in the browser. Download it instead.", true);
          renderDlHero(CFG.mime || "application/octet-stream", fmtSize(blob0.size), function () {
            return downloadHeld(held);
          });
          return;
        }
        showRawBlob(blob0);
        return;
      }
      if (kind === "text") {
        var r = await fetch(CFG.contentUrl, { headers: Object.assign({ "Range": "bytes=0-" + (TEXT_PREVIEW_MAX - 1) }, headers), cache: "no-store" });
        if (r.status === 403) throw new Error("Wrong password.");
        if (!r.ok && r.status !== 206) throw new Error("Preview failed (" + r.status + ").");
        showText(await r.text());
        say("");
        return;
      }
      if (kind === "download") {
        say("This file type cannot be previewed. Opening the download instead.");
      }
      var r2 = await fetch(CFG.contentUrl, { headers: headers, cache: "no-store" });
      if (r2.status === 403) throw new Error("Wrong password.");
      if (!r2.ok) throw new Error("Preview failed (" + r2.status + ").");
      var blob2 = await r2.blob();
      if (kind === "download") {
        renderDlHero(CFG.mime || "application/octet-stream", fmtSize(blob2.size), function () {
          return downloadHeld(held);
        });
      } else {
        await showBlob(blob2, kind);
        say("");
      }
    }

    function revealName(filename, mime) {
      if (!filename) return;
      CFG.filename = filename;
      if (mime) CFG.mime = mime;
      document.title = filename + " - Juicebox²";
      var fname = document.querySelector(".topbar .fname");
      if (fname) { fname.textContent = filename; fname.setAttribute("title", filename); }
    }
    async function go() {
      var password = pwInput.value;
      if (!password) { say("Enter the password first.", true); return; }
      btn.disabled = true;
      say("Unlocking...");
      try {
        if (CFG.keyVersion === "1") {
          var unlocked = await v1Unlock(password);
          pwInput.value = "";
          held = { v1: true, key: unlocked.key, plainLen: unlocked.plainLen };
          enableTopbar();
          revealName(unlocked.filename, unlocked.mime);
          if (CFG.mode === "download") {
            await v1Download(unlocked.key, unlocked.plainLen);
          } else if (CFG.mode === "view") {
            await v1View(held);
          } else {
            await v1Preview(unlocked.key, unlocked.plainLen);
          }
        } else {
          held = { v1: false, password: password };
          enableTopbar();
          try {
            var metaRes = await fetch(CFG.gateway + "/api/gateway/unlock", {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ id: CFG.id, password: password }),
              cache: "no-store"
            });
            if (metaRes.ok) {
              var meta = await metaRes.json();
              revealName(meta.filename, meta.mime_type);
            }
          } catch (e) {}
          await legacyFlow(password);
          pwInput.value = "";
        }
      } catch (e) {
        if (card) card.classList.remove("is-downloading");
        say(e.message || "Unlock failed.", true);
      } finally {
        btn.disabled = false;
      }
    }
    var unlockForm = document.getElementById("unlockform");
    if (unlockForm) unlockForm.addEventListener("submit", function (e) {
      e.preventDefault();
      go();
    });
    pwInput.focus();
  })();
