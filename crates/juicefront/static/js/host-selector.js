const PING_TIMEOUT_MS = 6000;
function str(root, key, fallback) {
  return root.getAttribute(`data-str-${key}`) || fallback;
}
function readSavedHost() {
  try {
    const ls = localStorage.getItem("juicebox_host") || "";
    if (ls.trim())
      return ls.trim();
  } catch {}
  try {
    const m = document.cookie.split("; ").find((c) => c.startsWith("juicebox_host="));
    if (m)
      return decodeURIComponent(m.split("=")[1] || "").trim();
  } catch {}
  return "";
}
function saveHost(url) {
  try {
    localStorage.setItem("juicebox_host", url);
  } catch {}
  try {
    document.cookie = `juicebox_host=${encodeURIComponent(url)}; path=/; ` + `max-age=${30 * 86400}; SameSite=Lax`;
  } catch {}
  const input = document.querySelector("[data-host-input]");
  if (input)
    input.value = url;
}
function syncConfigBits(cfg) {
  const serverCfg = document.getElementById("server-config");
  if (serverCfg) {
    if (typeof cfg.max_file_size_bytes === "number")
      serverCfg.setAttribute("data-max-file-size-bytes", String(cfg.max_file_size_bytes));
    if (Array.isArray(cfg.allowed_ttl_hours))
      serverCfg.setAttribute("data-allowed-ttl-hours", JSON.stringify(cfg.allowed_ttl_hours));
    if (typeof cfg.default_ttl_hours === "number")
      serverCfg.setAttribute("data-default-ttl-hours", String(cfg.default_ttl_hours));
    if (cfg.danger_level)
      serverCfg.setAttribute("data-danger-level", String(cfg.danger_level));
    if (typeof cfg.quick_link === "boolean")
      serverCfg.setAttribute("data-quick-link", String(cfg.quick_link));
    if (typeof cfg.quic === "boolean")
      serverCfg.setAttribute("data-quic", String(cfg.quic));
  }
  if (cfg.danger_level) {
    try {
      localStorage.setItem("juicebox_danger_level", String(cfg.danger_level));
    } catch {}
  }
  if (typeof cfg.ultrafast === "boolean") {
    try {
      localStorage.setItem("juicebox_ultrafast_supported", String(cfg.ultrafast));
    } catch {}
  }
}
function capsOf(cfg) {
  const caps = [];
  if (cfg.quic === true)
    caps.push("QUIC");
  if (cfg.cobalt === true)
    caps.push("Cobalt");
  if (cfg.quick_link === true)
    caps.push("Quick link");
  if (cfg.ultrafast === true)
    caps.push("Ultrafast");
  return caps;
}
function versionParts(v) {
  return String(v || "").split(".").map((p) => parseInt(p, 10));
}
function isLegacyVersion(cfg, current) {
  const theirs = typeof cfg.version === "string" ? cfg.version : "";
  if (!theirs || !current)
    return false;
  const a = versionParts(theirs);
  const b = versionParts(current);
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const x = Number.isFinite(a[i]) ? a[i] : 0;
    const y = Number.isFinite(b[i]) ? b[i] : 0;
    if (x !== y)
      return x < y;
  }
  return false;
}
export function initHostSelector() {
  const rootEl = document.getElementById("host-selector-modal");
  if (!rootEl || rootEl.dataset.hostselInit === "1")
    return;
  const root = rootEl;
  root.dataset.hostselInit = "1";
  const rows = Array.from(root.querySelectorAll("[data-hostsel-row]")).map((row) => ({
    row,
    url: row.getAttribute("data-url") || "",
    name: row.getAttribute("data-name") || "",
    state: "unknown",
    latencyMs: null,
    cfg: null
  }));
  const statusLine = root.querySelector("[data-hostsel-statusline]");
  const search = root.querySelector("[data-hostsel-search]");
  const onlineOnly = root.querySelector("[data-hostsel-online-only]");
  const sortSel = root.querySelector("[data-hostsel-sort]");
  const pingAllBtn = root.querySelector("[data-hostsel-ping-all]");
  const setStatusLine = (msg) => {
    if (statusLine)
      statusLine.textContent = msg;
  };
  function renderRow(s) {
    const dot = s.row.querySelector("[data-hostsel-dot]");
    const latency = s.row.querySelector("[data-hostsel-latency]");
    const status = s.row.querySelector("[data-hostsel-status]");
    const badges = s.row.querySelector("[data-hostsel-badges]");
    const pingBtn = s.row.querySelector("[data-hostsel-ping]");
    if (dot)
      dot.setAttribute("data-state", s.state);
    if (latency)
      latency.textContent = s.latencyMs === null ? "-" : `${Math.round(s.latencyMs)} ms`;
    if (status)
      status.textContent = s.state === "online" ? str(root, "online", "Online") : s.state === "offline" ? str(root, "offline", "Offline") : s.state === "checking" ? str(root, "checking", "Pinging...") : "";
    if (badges) {
      badges.innerHTML = "";
      if (s.state === "online" && s.cfg) {
        for (const cap of capsOf(s.cfg)) {
          const b = document.createElement("span");
          b.className = "hostsel-badge";
          b.textContent = cap;
          badges.appendChild(b);
        }
        if (isLegacyVersion(s.cfg, root.getAttribute("data-current-version") || "")) {
          const b = document.createElement("span");
          b.className = "hostsel-badge hostsel-badge--legacy";
          b.textContent = "Legacy";
          badges.appendChild(b);
        }
      }
    }
    if (pingBtn) {
      pingBtn.disabled = s.state === "checking";
      const label = pingBtn.querySelector("[data-hostsel-ping-label]");
      if (label)
        label.textContent = s.state === "checking" ? str(root, "pinging", "...") : str(root, "ping", "Ping");
    }
  }
  function markSelected(url) {
    for (const s of rows) {
      const current = s.row.querySelector("[data-hostsel-current]");
      const useBtn = s.row.querySelector("[data-hostsel-use]");
      const active = s.url === url;
      s.row.toggleAttribute("data-selected", active);
      if (current)
        current.hidden = !active;
      if (useBtn) {
        useBtn.disabled = active;
        const label = useBtn.querySelector("[data-hostsel-use-label]");
        if (label)
          label.textContent = active ? str(root, "selected", "In use") : str(root, "select", "Use");
      }
    }
  }
  async function pingRow(s) {
    if (s.state === "checking")
      return;
    if (s.url.startsWith("http://") && window.location.protocol === "https:") {
      s.state = "offline";
      s.latencyMs = null;
      renderRow(s);
      return;
    }
    s.state = "checking";
    renderRow(s);
    const t0 = performance.now();
    try {
      const res = await fetch(`${s.url}/api/config`, {
        signal: AbortSignal.timeout(PING_TIMEOUT_MS),
        headers: { Accept: "application/json" }
      });
      if (!res.ok)
        throw new Error(`status ${res.status}`);
      const cfg = await res.json();
      if (cfg === null || typeof cfg !== "object" || Array.isArray(cfg) || !("max_file_size_bytes" in cfg)) {
        throw new Error("not a JuiceHost node");
      }
      s.cfg = cfg;
      s.state = "online";
      s.latencyMs = performance.now() - t0;
    } catch {
      s.state = "offline";
      s.latencyMs = null;
      s.cfg = null;
    }
    renderRow(s);
    if (pendingSelect === s) {
      pendingSelect = null;
      if (s.state === "online")
        selectRow(s);
    }
    applyFilters();
  }
  function pingAll() {
    if (typeof document !== "undefined" && document.hidden)
      return;
    setStatusLine(str(root, "checking", "Pinging..."));
    Promise.allSettled(rows.map((s) => pingRow(s))).then(() => {
      const online = rows.filter((s) => s.state === "online").length;
      setStatusLine(`${online}/${rows.length} ${str(root, "online", "Online").toLowerCase()}`);
    });
  }
  function applyFilters() {
    const q = (search?.value || "").trim().toLowerCase();
    const onlyOnline = !!onlineOnly?.checked;
    const mode = sortSel?.value || "latency";
    for (const section of root.querySelectorAll("[data-hostsel-section]")) {
      const list = section.querySelector("[data-hostsel-list]");
      const empty = section.querySelector("[data-hostsel-empty]");
      const sectionRows = rows.filter((s) => list?.contains(s.row));
      let visible = 0;
      for (const s of sectionRows) {
        const hay = `${s.name} ${s.url}`.toLowerCase();
        const ok = (!q || hay.includes(q)) && (!onlyOnline || s.state === "online");
        s.row.hidden = !ok;
        if (ok)
          visible++;
      }
      if (empty)
        empty.hidden = visible > 0;
      if (list) {
        const ordered = sectionRows.filter((s) => !s.row.hidden).sort((a, b) => {
          if (mode === "name")
            return a.name.localeCompare(b.name);
          const al = a.latencyMs ?? Number.POSITIVE_INFINITY;
          const bl = b.latencyMs ?? Number.POSITIVE_INFINITY;
          return al - bl;
        });
        for (const s of ordered)
          list.appendChild(s.row);
      }
    }
  }
  function selectRow(s) {
    if (s.state !== "online" || !s.cfg)
      return;
    saveHost(s.url);
    syncConfigBits(s.cfg);
    window.dispatchEvent(new CustomEvent("juicehost-config-updated", { detail: s.cfg }));
    markSelected(s.url);
    setStatusLine(str(root, "applied", "Host applied"));
    location.hash = "#settings-modal";
  }
  let pendingSelect = null;
  function activateRow(s) {
    if (s.state === "online") {
      selectRow(s);
      return;
    }
    pendingSelect = s;
    pingRow(s);
  }
  root.addEventListener("click", (e) => {
    const t = e.target;
    if (!t || (t.closest && t.closest("a, button, input, select, textarea")))
      return;
    const row = t.closest ? t.closest("[data-hostsel-row]") : null;
    if (!row)
      return;
    const s = rows.find((r) => r.row === row);
    if (s)
      activateRow(s);
  });
  root.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ")
      return;
    const target = e.target;
    const row = target && target.closest ? target.closest("[data-hostsel-row]") : null;
    if (!row)
      return;
    e.preventDefault();
    const s = rows.find((r) => r.row === row);
    if (s)
      activateRow(s);
  });
  search?.addEventListener("input", applyFilters);
  onlineOnly?.addEventListener("change", applyFilters);
  sortSel?.addEventListener("change", applyFilters);
  pingAllBtn?.addEventListener("click", pingAll);
  markSelected(readSavedHost());
  rows.forEach(renderRow);
  applyFilters();
  let pinged = false;
  const maybePing = () => {
    if (location.hash === "#host-selector-modal" && !pinged) {
      pinged = true;
      pingAll();
    }
  };
  window.addEventListener("hashchange", maybePing);
  document.addEventListener("DOMContentLoaded", () => {
    pinged = false;
    maybePing();
  });
  maybePing();
}
document.addEventListener("DOMContentLoaded", initHostSelector);
initHostSelector();
