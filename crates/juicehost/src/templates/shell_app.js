  (function () {
    "use strict";
    var CFG = {
      id: "__FILE_ID__",
      gateway: "__GATEWAY_ORIGIN__",
      cipherUrl: "__CIPHERTEXT_URL__",
      contentUrl: "__CONTENT_URL__",
      mode: "__MODE__",
      keyVersion: "__KEY_VERSION__",
      filename: "__FILENAME__",
      mime: "__MIME__"
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
    async function fetchRange(url, start, len, extraHeaders) {
      var headers = extraHeaders || {};
      headers["Range"] = "bytes=" + start + "-" + (start + len - 1);
      var res = await fetch(url, { headers: headers, cache: "no-store" });
      if (res.status !== 206 && !(start === 0 && res.status === 200)) {
        throw new Error("range request failed: " + res.status);
      }
      return new Uint8Array(await res.arrayBuffer());
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
    // Decrypt plaintext [plainStart, plainEnd) by fetching exactly the
    // overlapping stored chunks, then slicing. Chunk-aligned sequential
    // calls stream without ever holding the whole file.
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
    function showBlob(blob, kind) {
      var url = URL.createObjectURL(blob);
      card.hidden = true;
      if (kind === "image") {
        var img = document.createElement("img");
        img.id = "media"; img.src = url; img.alt = CFG.filename;
        stage.appendChild(img);
      } else if (kind === "video") {
        var video = document.createElement("video");
        video.id = "media"; video.controls = true; video.src = url;
        stage.appendChild(video);
      } else if (kind === "audio") {
        var audio = document.createElement("audio");
        audio.id = "media"; audio.controls = true; audio.src = url;
        stage.appendChild(audio);
      } else if (kind === "pdf") {
        var frame = document.createElement("iframe");
        frame.id = "media"; frame.className = "pdf-frame"; frame.src = url; frame.title = CFG.filename;
        stage.appendChild(frame);
      }
    }
    function showText(text) {
      card.hidden = true;
      var pre = document.createElement("pre");
      pre.className = "text-view";
      pre.textContent = text;
      stage.appendChild(pre);
    }
    function kindOf(mime) {
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
          return;
        } catch (e) {
          if (e && e.name === "AbortError") { say("Save cancelled."); return; }
          // Fall through to blob fallback below.
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
      return { key: key, plainLen: body.plain_len };
    }

    async function v1Download(key, plainLen) {
      var totalChunks = Math.max(1, Math.ceil(plainLen / PLAIN_CHUNK));
      var done = 0;
      await saveDownload(async function (writable) {
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
      say("Download complete.");
    }

    // Full decrypt into one Blob (view/open + media preview). Holds the
    // whole file in memory: callers cap absurd sizes first.
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

    // Post-unlock actions for preview mode. Both reuse the held key only:
    // the password is never resent, stored, or placed in a URL.
    function offerActions(key, plainLen) {
      var bar = document.createElement("div");
      bar.setAttribute("style", "display:flex;gap:.5rem;flex:none;");
      var dl = document.createElement("button");
      dl.textContent = "Download decrypted";
      dl.setAttribute("style", "padding:.45rem .9rem;border:1px solid #4a3d33;border-radius:.5rem;background:#241d17;color:#e8ded4;font-weight:600;cursor:pointer;");
      dl.addEventListener("click", function () {
        say("Downloading...");
        v1Download(key, plainLen).catch(function (e) { say(e.message, true); });
      });
      var open = document.createElement("button");
      open.textContent = "Open as file";
      open.setAttribute("style", "padding:.45rem .9rem;border:1px solid #4a3d33;border-radius:.5rem;background:#241d17;color:#e8ded4;font-weight:600;cursor:pointer;");
      open.addEventListener("click", function () {
        v1View(key, plainLen).catch(function (e) { say(e.message, true); });
      });
      bar.appendChild(dl);
      bar.appendChild(open);
      stage.appendChild(bar);
    }

    // View mode (/f/): the decrypted bytes become the document itself via a
    // blob URL, so the browser renders the file natively (image as image,
    // PDF as PDF, text as text) instead of embedding it in this page.
    // Blob URLs are origin-scoped unguessable tokens; nothing new is sent
    // anywhere and the key stays in memory.
    async function v1View(key, plainLen) {
      if (plainLen > 512 * 1024 * 1024) {
        say("File is too large to open in the browser. Download it instead.", true);
        btn.textContent = "Download";
        btn.onclick = function () { v1Download(key, plainLen).catch(function (e) { say(e.message, true); }); };
        return;
      }
      say("Decrypting...");
      var blob = await decryptFull(key, plainLen);
      location.href = URL.createObjectURL(blob);
    }

    async function v1Preview(key, plainLen) {
      var kind = kindOf(CFG.mime);
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
        offerActions(key, plainLen);
        return;
      }
      if (kind === "download") {
        say("This file type cannot be previewed. Press Unlock again to download it.");
        btn.textContent = "Download";
        btn.onclick = function () { v1Download(key, plainLen).catch(function (e) { say(e.message, true); }); };
        return;
      }
      if (plainLen > 512 * 1024 * 1024 && (kind === "video" || kind === "audio")) {
        say("File is too large to preview in the browser. Press Unlock again to download it.", true);
        btn.textContent = "Download";
        btn.onclick = function () { v1Download(key, plainLen).catch(function (e) { say(e.message, true); }); };
        return;
      }
      say("Decrypting...");
      showBlob(await decryptFull(key, plainLen), kind);
      say("");
      offerActions(key, plainLen);
    }

    async function legacyFlow(password) {
      var headers = { "X-File-Password": password };
      if (CFG.mode === "download") {        say("Downloading...");
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
        say("Download complete.");
        return;
      }
      var kind = kindOf(CFG.mime);
      if (CFG.mode === "view") {
        say("Opening...");
        var r0 = await fetch(CFG.contentUrl, { headers: headers, cache: "no-store" });
        if (r0.status === 403) throw new Error("Wrong password.");
        if (!r0.ok) throw new Error("Open failed (" + r0.status + ").");
        location.href = URL.createObjectURL(await r0.blob());
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
        var a2 = document.createElement("a");
        a2.href = URL.createObjectURL(blob2);
        a2.download = CFG.filename;
        document.body.appendChild(a2);
        a2.click();
        setTimeout(function () { URL.revokeObjectURL(a2.href); a2.remove(); }, 4000);
        say("Download complete.");
      } else {
        showBlob(blob2, kind);
        say("");
      }
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
          if (CFG.mode === "download") {
            await v1Download(unlocked.key, unlocked.plainLen);
          } else if (CFG.mode === "view") {
            await v1View(unlocked.key, unlocked.plainLen);
          } else {
            await v1Preview(unlocked.key, unlocked.plainLen);
          }
        } else {
          await legacyFlow(password);
          pwInput.value = "";
        }
      } catch (e) {
        say(e.message || "Unlock failed.", true);
      } finally {
        btn.disabled = false;
      }
    }
    btn.addEventListener("click", go);
    pwInput.addEventListener("keydown", function (e) { if (e.key === "Enter") go(); });
    pwInput.focus();
  })();
