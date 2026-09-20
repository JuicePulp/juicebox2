/**
 * Host selector modal logic (main page only).
 *
 * Pings every listed node via its `/api/config`, shows STATUS + LATENCY,
 * filters/sorts the two lists, and persists the chosen node to the same
 * `juicebox_host` cookie + localStorage keys the settings modal uses.
 * Selecting dispatches `juicehost-config-updated` so UploadCard rebuilds
 * retention options without a reload.
 */

interface NodeFeatures {
  quic: boolean;
  cobalt: boolean;
}

type NodeState = "unknown" | "checking" | "online" | "offline";

interface RowState {
  row: HTMLElement;
  url: string;
  name: string;
  state: NodeState;
  latencyMs: number | null;
  cfg: Record<string, unknown> | null;
}

const PING_TIMEOUT_MS = 6000;

function str(root: HTMLElement, key: string, fallback: string): string {
  return root.getAttribute(`data-str-${key}`) || fallback;
}

function readSavedHost(): string {
  try {
    const ls = localStorage.getItem("juicebox_host") || "";
    if (ls.trim()) return ls.trim();
  } catch {}
  try {
    const m = document.cookie
      .split("; ")
      .find((c) => c.startsWith("juicebox_host="));
    if (m) return decodeURIComponent(m.split("=")[1] || "").trim();
  } catch {}
  return "";
}

function saveHost(url: string): void {
  try {
    localStorage.setItem("juicebox_host", url);
  } catch {}
  try {
    document.cookie =
      `juicebox_host=${encodeURIComponent(url)}; path=/; ` +
      `max-age=${30 * 86400}; SameSite=Lax`;
  } catch {}
  const input = document.querySelector(
    "[data-host-input]",
  ) as HTMLInputElement | null;
  if (input) input.value = url;
}

/** Mirror the persisted config bits the settings modal keeps in sync. */
function syncConfigBits(cfg: Record<string, unknown>): void {
  const serverCfg = document.getElementById("server-config");
  if (serverCfg) {
    if (typeof cfg.max_file_size_bytes === "number")
      serverCfg.setAttribute(
        "data-max-file-size-bytes",
        String(cfg.max_file_size_bytes),
      );
    if (Array.isArray(cfg.allowed_ttl_hours))
      serverCfg.setAttribute(
        "data-allowed-ttl-hours",
        JSON.stringify(cfg.allowed_ttl_hours),
      );
    if (typeof cfg.default_ttl_hours === "number")
      serverCfg.setAttribute(
        "data-default-ttl-hours",
        String(cfg.default_ttl_hours),
      );
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
      localStorage.setItem(
        "juicebox_ultrafast_supported",
        String(cfg.ultrafast),
      );
    } catch {}
  }
}

function featuresOf(cfg: Record<string, unknown>): NodeFeatures {
  return { quic: cfg.quic === true, cobalt: cfg.cobalt === true };
}

export function initHostSelector(): void {
  const rootEl = document.getElementById("host-selector-modal");
  if (!rootEl || rootEl.dataset.hostselInit === "1") return;
  // Bind a non-nullable alias: narrowing on `rootEl` is lost inside the
  // closures below, so the explicit type keeps astro check clean.
  const root: HTMLElement = rootEl;
  root.dataset.hostselInit = "1";

  const rows: RowState[] = Array.from(
    root.querySelectorAll<HTMLElement>("[data-hostsel-row]"),
  ).map((row) => ({
    row,
    url: row.getAttribute("data-url") || "",
    name: row.getAttribute("data-name") || "",
    state: "unknown" as NodeState,
    latencyMs: null,
    cfg: null,
  }));

  const statusLine = root.querySelector<HTMLElement>(
    "[data-hostsel-statusline]",
  );
  const search = root.querySelector<HTMLInputElement>(
    "[data-hostsel-search]",
  );
  const onlineOnly = root.querySelector<HTMLInputElement>(
    "[data-hostsel-online-only]",
  );
  const sortSel = root.querySelector<HTMLSelectElement>(
    "[data-hostsel-sort]",
  );
  const pingAllBtn = root.querySelector<HTMLButtonElement>(
    "[data-hostsel-ping-all]",
  );

  const setStatusLine = (msg: string) => {
    if (statusLine) statusLine.textContent = msg;
  };

  function renderRow(s: RowState): void {
    const dot = s.row.querySelector<HTMLElement>("[data-hostsel-dot]");
    const latency = s.row.querySelector<HTMLElement>(
      "[data-hostsel-latency]",
    );
    const status = s.row.querySelector<HTMLElement>("[data-hostsel-status]");
    const badges = s.row.querySelector<HTMLElement>(
      "[data-hostsel-badges]",
    );
    const pingBtn = s.row.querySelector<HTMLButtonElement>(
      "[data-hostsel-ping]",
    );
    if (dot) dot.setAttribute("data-state", s.state);
    if (latency)
      latency.textContent =
        s.latencyMs === null ? "—" : `${Math.round(s.latencyMs)} ms`;
    if (status)
      status.textContent =
        s.state === "online"
          ? str(root, "online", "Online")
          : s.state === "offline"
            ? str(root, "offline", "Offline")
            : s.state === "checking"
              ? str(root, "checking", "Pinging...")
              : "";
    if (badges) {
      badges.innerHTML = "";
      if (s.state === "online" && s.cfg) {
        const f = featuresOf(s.cfg);
        if (f.quic) {
          const b = document.createElement("span");
          b.className = "hostsel-badge";
          b.textContent = "QUIC";
          badges.appendChild(b);
        }
        if (f.cobalt) {
          const b = document.createElement("span");
          b.className = "hostsel-badge";
          b.textContent = "Cobalt";
          badges.appendChild(b);
        }
      }
    }
    if (pingBtn) {
      pingBtn.disabled = s.state === "checking";
      const label = pingBtn.querySelector("[data-hostsel-ping-label]");
      if (label)
        label.textContent =
          s.state === "checking"
            ? str(root, "pinging", "...")
            : str(root, "ping", "Ping");
    }
  }

  function markSelected(url: string): void {
    for (const s of rows) {
      const current = s.row.querySelector<HTMLElement>(
        "[data-hostsel-current]",
      );
      const useBtn = s.row.querySelector<HTMLButtonElement>(
        "[data-hostsel-use]",
      );
      const active = s.url === url;
      s.row.toggleAttribute("data-selected", active);
      if (current) current.hidden = !active;
      if (useBtn) {
        useBtn.disabled = active;
        const label = useBtn.querySelector("[data-hostsel-use-label]");
        if (label)
          label.textContent = active
            ? str(root, "selected", "In use")
            : str(root, "select", "Use");
      }
    }
  }

  async function pingRow(s: RowState): Promise<void> {
    if (s.state === "checking") return;
    // Mixed-content pages can't probe http:// nodes; don't spam console.
    if (
      s.url.startsWith("http://") &&
      window.location.protocol === "https:"
    ) {
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
        headers: { Accept: "application/json" },
      });
      if (!res.ok) throw new Error(`status ${res.status}`);
      const cfg: unknown = await res.json();
      if (
        cfg === null ||
        typeof cfg !== "object" ||
        Array.isArray(cfg) ||
        !("max_file_size_bytes" in (cfg as Record<string, unknown>))
      ) {
        throw new Error("not a juicebox node");
      }
      s.cfg = cfg as Record<string, unknown>;
      s.state = "online";
      s.latencyMs = performance.now() - t0;
    } catch {
      s.state = "offline";
      s.latencyMs = null;
      s.cfg = null;
    }
    renderRow(s);
    applyFilters();
  }

  function pingAll(): void {
    if (typeof document !== "undefined" && document.hidden) return;
    setStatusLine(str(root, "checking", "Pinging..."));
    void Promise.allSettled(rows.map((s) => pingRow(s))).then(() => {
      const online = rows.filter((s) => s.state === "online").length;
      setStatusLine(
        `${online}/${rows.length} ${str(root, "online", "Online").toLowerCase()}`,
      );
    });
  }

  function applyFilters(): void {
    const q = (search?.value || "").trim().toLowerCase();
    const onlyOnline = !!onlineOnly?.checked;
    const mode = sortSel?.value || "latency";

    for (const section of root.querySelectorAll<HTMLElement>(
      "[data-hostsel-section]",
    )) {
      const list = section.querySelector<HTMLElement>("[data-hostsel-list]");
      const empty = section.querySelector<HTMLElement>(
        "[data-hostsel-empty]",
      );
      const sectionRows = rows.filter((s) => list?.contains(s.row));
      let visible = 0;
      for (const s of sectionRows) {
        const hay = `${s.name} ${s.url}`.toLowerCase();
        const ok =
          (!q || hay.includes(q)) &&
          (!onlyOnline || s.state === "online");
        s.row.hidden = !ok;
        if (ok) visible++;
      }
      if (empty) empty.hidden = visible > 0;
      if (list) {
        const ordered = sectionRows
          .filter((s) => !s.row.hidden)
          .sort((a, b) => {
            if (mode === "name") return a.name.localeCompare(b.name);
            const al = a.latencyMs ?? Number.POSITIVE_INFINITY;
            const bl = b.latencyMs ?? Number.POSITIVE_INFINITY;
            return al - bl;
          });
        for (const s of ordered) list.appendChild(s.row);
      }
    }
  }

  function selectRow(s: RowState): void {
    if (s.state !== "online" || !s.cfg) return;
    saveHost(s.url);
    syncConfigBits(s.cfg);
    window.dispatchEvent(
      new CustomEvent("juicehost-config-updated", { detail: s.cfg }),
    );
    markSelected(s.url);
    setStatusLine(str(root, "applied", "Host applied"));
    location.hash = "#!";
  }

  root.addEventListener("click", (e) => {
    const t = e.target as HTMLElement | null;
    if (!t) return;
    const pingBtn = t.closest<HTMLElement>("[data-hostsel-ping]");
    if (pingBtn) {
      const row = pingBtn.closest<HTMLElement>("[data-hostsel-row]");
      const s = rows.find((r) => r.row === row);
      if (s) void pingRow(s);
      return;
    }
    const useBtn = t.closest<HTMLElement>("[data-hostsel-use]");
    if (useBtn) {
      const row = useBtn.closest<HTMLElement>("[data-hostsel-row]");
      const s = rows.find((r) => r.row === row);
      if (s) selectRow(s);
    }
  });

  search?.addEventListener("input", applyFilters);
  onlineOnly?.addEventListener("change", applyFilters);
  sortSel?.addEventListener("change", applyFilters);
  pingAllBtn?.addEventListener("click", pingAll);

  markSelected(readSavedHost());
  rows.forEach(renderRow);
  applyFilters();

  // Ping once when the modal first opens (and on init if already open).
  let pinged = false;
  const maybePing = () => {
    if (location.hash === "#host-selector-modal" && !pinged) {
      pinged = true;
      pingAll();
    }
  };
  window.addEventListener("hashchange", maybePing);
  document.addEventListener("astro:page-load", () => {
    pinged = false;
    maybePing();
  });
  maybePing();
}

document.addEventListener("astro:page-load", initHostSelector);
initHostSelector();
