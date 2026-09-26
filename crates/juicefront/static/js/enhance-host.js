import { formatMaxSize } from "./util.js";
import { clearHostGlow, shouldGlowHost } from "./presence.js";
export function readSelectedHost() {
  const input = document.querySelector("[data-host-input]");
  if (input && input.value.trim())
    return input.value.trim();
  return "";
}
export function readSelectedUploadMode() {
  return localStorage.getItem("juicebox_upload_mode") || "standard";
}
export function readQuickLinkEnabled() {
  return localStorage.getItem("juicebox_quick_link") === "true";
}
export function readDangerLevel() {
  return localStorage.getItem("juicebox_danger_level") || "high";
}
export function readUltraFastEnabled() {
  return localStorage.getItem("juicebox_ultrafast_enabled") === "true";
}
export function readUltraFastSupported() {
  return localStorage.getItem("juicebox_ultrafast_supported") === "true";
}
export function initHostGlow() {
  const btn = document.querySelector("[data-host-btn]");
  if (!btn)
    return;
  const observer = new MutationObserver(() => {
    if (location.hash === "#host-modal" || location.hash === "#host-selector-modal") {
      clearHostGlow();
    }
  });
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["hash"] });
  btn.addEventListener("click", clearHostGlow);
  if (shouldGlowHost()) {
    import("./presence.js").then(({ isAppMode }) => {
      if (isAppMode()) {
        btn.classList.add("host-btn--glow");
      }
    });
  }
  window.addEventListener("juicebox-app-mode", (e) => {
    if (e.detail.connected && shouldGlowHost()) {
      btn.classList.add("host-btn--glow");
    } else {
      btn.classList.remove("host-btn--glow");
    }
  });
}
const BLOCKED_TYPES_I18N = {
  en: {
    none: "All file types allowed.",
    low: "Blocks executable files, installers, DLLs, and disk images.",
    medium: "Blocks executable files, installers, DLLs, disk images, shell scripts, batch files, and PowerShell scripts.",
    high: "Blocks executable files, installers, DLLs, disk images, shell scripts, batch files, PowerShell scripts, JavaScript, HTML/SVG, PHP, and Python files."
  },
  es: {
    none: "Todos los tipos de archivo están permitidos.",
    low: "Bloquea archivos ejecutables, instaladores, DLL e imágenes de disco.",
    medium: "Bloquea archivos ejecutables, instaladores, DLL, imágenes de disco, scripts de shell, scripts por lotes y scripts de PowerShell.",
    high: "Bloquea archivos ejecutables, instaladores, DLL, imágenes de disco, scripts de shell, scripts por lotes, scripts de PowerShell, JavaScript, HTML/SVG, PHP y Python."
  },
  fr: {
    none: "Tous les types de fichiers sont autorisés.",
    low: "Bloque les fichiers exécutables, les installateurs, les DLL et les images disque.",
    medium: "Bloque les fichiers exécutables, les installateurs, les DLL, les images disque, les scripts shell, les scripts batch et les scripts PowerShell.",
    high: "Bloque les fichiers exécutables, les installateurs, les DLL, les images disque, les scripts shell, les scripts batch, les scripts PowerShell, JavaScript, HTML/SVG, PHP et Python."
  },
  ru: {
    none: "Все типы файлов разрешены.",
    low: "Блокирует исполняемые файлы, установщики, DLL и образы дисков.",
    medium: "Блокирует исполняемые файлы, установщики, DLL, образы дисков, shell-скрипты, bat-файлы и PowerShell-скрипты.",
    high: "Блокирует исполняемые файлы, установщики, DLL, образы дисков, shell-скрипты, bat-файлы, PowerShell-скрипты, JavaScript, HTML/SVG, PHP и Python."
  }
};
function updateDangerLevelDisplay(level, locale) {
  const el = document.querySelector("[data-blocked-types-text]");
  if (!el)
    return;
  const lang = locale && BLOCKED_TYPES_I18N[locale] ? locale : "en";
  const text = BLOCKED_TYPES_I18N[lang][level] || BLOCKED_TYPES_I18N[lang].high;
  el.textContent = text;
}
function updateDropMaxSize(bytes) {
  const el = document.querySelector("[data-drop-max-template]");
  if (!el)
    return;
  const tpl = el.getAttribute("data-drop-max-template") || "Max upload size: {size}";
  const size = document.createElement("span");
  size.setAttribute("data-drop-max-size", "");
  size.textContent = formatMaxSize(bytes);
  const [before, after] = tpl.split("{size}");
  el.textContent = "";
  el.append(before ?? "", size, after ?? "");
}
const HOST_CHECK_FAILURES = {
  empty: "[X] Enter an address",
  invalid: "[X] Invalid address",
  scheme: "[X] Use http(s) only",
  mixed: "[X] HTTPS page, HTTP server",
  status: "[X] Server error",
  notjuicebox: "[X] Not a juicebox server",
  timeout: "[X] Timed out",
  blocked: "[X] Blocked by browser (CORS)",
  unreachable: "[X] Unreachable"
};
function parseHostUrl(host) {
  let raw = host.trim();
  if (!raw)
    return "empty";
  if (!/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(raw))
    raw = `https://${raw}`;
  let url;
  try {
    url = new URL(raw);
  } catch {
    return "invalid";
  }
  if (url.protocol !== "http:" && url.protocol !== "https:")
    return "scheme";
  if (!url.hostname)
    return "invalid";
  return { base: url.origin, scheme: url.protocol };
}
async function fetchHostConfig(host) {
  const parsed = parseHostUrl(host);
  if (typeof parsed === "string") {
    return { ok: false, reason: parsed };
  }
  if (parsed.scheme === "http:" && window.location.protocol === "https:") {
    return { ok: false, reason: "mixed" };
  }
  try {
    const res = await fetch(`${parsed.base}/api/config`, {
      method: "GET",
      signal: AbortSignal.timeout(5000)
    });
    if (!res.ok)
      return { ok: false, reason: "status" };
    const cfg = await res.json();
    if (cfg === null || typeof cfg !== "object" || Array.isArray(cfg)) {
      return { ok: false, reason: "notjuicebox" };
    }
    const record = cfg;
    if (!("max_file_size_bytes" in record) || !("default_ttl_hours" in record)) {
      return { ok: false, reason: "notjuicebox" };
    }
    return { ok: true, cfg: record };
  } catch (err) {
    if (err instanceof DOMException && (err.name === "AbortError" || err.name === "TimeoutError")) {
      return { ok: false, reason: "timeout" };
    }
    if (err instanceof TypeError) {
      const reachable = await fetch(`${parsed.base}/api/config`, {
        method: "GET",
        mode: "no-cors",
        signal: AbortSignal.timeout(5000)
      }).then(() => true).catch(() => false);
      return { ok: false, reason: reachable ? "blocked" : "unreachable" };
    }
    return { ok: false, reason: "unreachable" };
  }
}
export function enhanceHostSelector(locale) {
  const input = document.querySelector("[data-host-input]");
  const checkBtn = document.querySelector("[data-host-check]");
  const applyBtn = document.querySelector("[data-host-apply]");
  const status = document.querySelector("[data-host-status]");
  if (!input || !checkBtn || !applyBtn || !status)
    return;
  const cookieHost = document.cookie.split("; ").find((c) => c.startsWith("juicebox_host="))?.split("=")[1];
  const saved = localStorage.getItem("juicebox_host") || (cookieHost ? decodeURIComponent(cookieHost) : "");
  if (saved)
    input.value = saved;
  const saveHost = () => {
    const val = input.value.trim();
    localStorage.setItem("juicebox_host", val);
    document.cookie = `juicebox_host=${encodeURIComponent(val)}; path=/; max-age=2592000; SameSite=Lax`;
  };
  input.addEventListener("change", saveHost);
  input.addEventListener("blur", saveHost);
  const quicToggle = document.querySelector("[data-quic-toggle]");
  const syncQuicFromUltrafast = () => {
    if (!quicToggle)
      return;
    const ufEnabled = localStorage.getItem("juicebox_ultrafast_enabled") === "true";
    const quicServerOk = document.getElementById("server-config")?.getAttribute("data-quic") !== "false";
    if (ufEnabled) {
      quicToggle.checked = false;
      quicToggle.disabled = true;
      localStorage.setItem("juicebox_upload_mode", "standard");
    } else {
      quicToggle.disabled = !quicServerOk;
    }
  };
  if (quicToggle) {
    const savedMode = localStorage.getItem("juicebox_upload_mode") || "standard";
    quicToggle.checked = savedMode === "quic";
    quicToggle.addEventListener("change", () => {
      localStorage.setItem("juicebox_upload_mode", quicToggle.checked ? "quic" : "standard");
    });
  }
  const quickLinkToggle = document.querySelector("[data-quick-link-toggle]");
  if (quickLinkToggle) {
    quickLinkToggle.checked = readQuickLinkEnabled();
    quickLinkToggle.addEventListener("change", () => {
      localStorage.setItem("juicebox_quick_link", quickLinkToggle.checked ? "true" : "false");
    });
  }
  const ultrafastToggle = document.querySelector("[data-ultrafast-toggle]");
  if (ultrafastToggle) {
    ultrafastToggle.checked = localStorage.getItem("juicebox_ultrafast_enabled") === "true";
    ultrafastToggle.addEventListener("change", () => {
      localStorage.setItem("juicebox_ultrafast_enabled", ultrafastToggle.checked ? "true" : "false");
      syncQuicFromUltrafast();
    });
    const updateUltrafastState = () => {
      const deviceConnected = !!window.__juiceboxAppMode;
      const serverSupported = readUltraFastSupported();
      const canUse = deviceConnected && serverSupported;
      ultrafastToggle.disabled = !canUse;
      if (!canUse && ultrafastToggle.dataset.initialized === "true") {
        ultrafastToggle.checked = false;
        syncQuicFromUltrafast();
      }
      ultrafastToggle.dataset.initialized = "true";
    };
    window.addEventListener("juicebox-app-mode", updateUltrafastState);
    syncQuicFromUltrafast();
    const serverSupported = readUltraFastSupported();
    ultrafastToggle.disabled = !serverSupported;
    setTimeout(() => {
      updateUltrafastState();
    }, 2000);
  }
  updateDangerLevelDisplay(readDangerLevel(), locale);
  const serverCfg = document.getElementById("server-config");
  if (serverCfg) {
    const ql = serverCfg.getAttribute("data-quick-link");
    const qc = serverCfg.getAttribute("data-quic");
    const dl = serverCfg.getAttribute("data-danger-level");
    if (ql === "false" && quickLinkToggle) {
      quickLinkToggle.disabled = true;
      quickLinkToggle.checked = false;
      localStorage.setItem("juicebox_quick_link", "false");
    } else if (ql === "true" && quickLinkToggle) {
      quickLinkToggle.disabled = false;
      if (localStorage.getItem("juicebox_quick_link") === null) {
        localStorage.setItem("juicebox_quick_link", "false");
      }
    }
    if (qc === "false" && quicToggle) {
      quicToggle.disabled = true;
      quicToggle.checked = false;
      localStorage.setItem("juicebox_upload_mode", "standard");
    }
    const uf = serverCfg.getAttribute("data-ultrafast");
    if (uf !== null) {
      localStorage.setItem("juicebox_ultrafast_supported", String(uf === "true"));
    }
    if (dl) {
      localStorage.setItem("juicebox_danger_level", dl);
      updateDangerLevelDisplay(dl, locale);
    }
  } else {
    fetch("/api/config", { signal: AbortSignal.timeout(5000) }).then((r) => r.json()).then((cfg) => {
      if (!cfg.quic && quicToggle) {
        quicToggle.disabled = true;
        quicToggle.checked = false;
        localStorage.setItem("juicebox_upload_mode", "standard");
      }
      if (!cfg.quick_link && quickLinkToggle) {
        quickLinkToggle.disabled = true;
        quickLinkToggle.checked = false;
        localStorage.setItem("juicebox_quick_link", "false");
      }
      if (typeof cfg.ultrafast === "boolean") {
        localStorage.setItem("juicebox_ultrafast_supported", String(cfg.ultrafast));
      }
      if (cfg.danger_level) {
        localStorage.setItem("juicebox_danger_level", cfg.danger_level);
        updateDangerLevelDisplay(cfg.danger_level, locale);
      }
    }).catch(() => {
      if (quicToggle) {
        quicToggle.disabled = true;
      }
      if (quickLinkToggle) {
        quickLinkToggle.disabled = true;
      }
      updateDangerLevelDisplay(readDangerLevel(), locale);
    });
  }
  const applyConfig = (cfg) => {
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
    if (typeof cfg.max_file_size_bytes === "number") {
      updateDropMaxSize(cfg.max_file_size_bytes);
    }
    if (cfg.danger_level) {
      const dl = String(cfg.danger_level);
      localStorage.setItem("juicebox_danger_level", dl);
      updateDangerLevelDisplay(dl, locale);
    }
    if (!cfg.quick_link && quickLinkToggle) {
      quickLinkToggle.disabled = true;
      quickLinkToggle.checked = false;
      localStorage.setItem("juicebox_quick_link", "false");
    } else if (cfg.quick_link && quickLinkToggle) {
      quickLinkToggle.disabled = false;
    }
    if (!cfg.quic && quicToggle) {
      quicToggle.disabled = true;
      quicToggle.checked = false;
      localStorage.setItem("juicebox_upload_mode", "standard");
    } else if (cfg.quic && quicToggle) {
      quicToggle.disabled = false;
    }
    if (typeof cfg.ultrafast === "boolean") {
      localStorage.setItem("juicebox_ultrafast_supported", String(cfg.ultrafast));
      if (ultrafastToggle) {
        const deviceConnected = !!window.__juiceboxAppMode;
        const canUse = deviceConnected && cfg.ultrafast;
        ultrafastToggle.disabled = !canUse;
        if (!canUse)
          ultrafastToggle.checked = false;
        syncQuicFromUltrafast();
      }
    }
    window.dispatchEvent(new CustomEvent("juicehost-config-updated", { detail: cfg }));
  };
  const validateHost = async () => {
    const host = input.value.trim();
    if (!host) {
      status.textContent = "empty";
      return null;
    }
    status.textContent = "checking...";
    const result = await fetchHostConfig(host);
    if (result.ok) {
      status.textContent = "[O] Valid";
      status.style.color = "var(--green-500)";
      return result.cfg;
    }
    status.textContent = HOST_CHECK_FAILURES[result.reason] ?? "[X] unreachable";
    status.style.color = "";
    return null;
  };
  checkBtn.addEventListener("click", async () => {
    checkBtn.disabled = true;
    try {
      await validateHost();
    } finally {
      checkBtn.disabled = false;
    }
  });
  applyBtn.addEventListener("click", async () => {
    applyBtn.disabled = true;
    try {
      const cfg = await validateHost();
      if (!cfg)
        return;
      saveHost();
      applyConfig(cfg);
      status.textContent = "[O] Applied";
      status.style.color = "var(--green-500)";
    } finally {
      applyBtn.disabled = false;
    }
  });
}
