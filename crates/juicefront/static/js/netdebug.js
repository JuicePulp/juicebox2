import {
  detectNetTier,
  streamsForRate,
  TIER_DEFAULT_BPS,
  TIER_CHUNK,
  PARALLEL_STREAMS,
  TUS_MIN_PART,
  TUS_MAX_PARTS,
  TUNER_INTERVAL_MS,
  TUS_THRESHOLD,
  TUS_TARGET_CHUNK_SECS,
  LAST_SPEED_KEY
} from "./upload-config.js";
import { apiHealth } from "./api.js";
const MB = 1024 * 1024;
const fmtRate = (bps) => bps >= MB ? `${(bps / MB).toFixed(2)} MB/s` : `${(bps / 1024).toFixed(0)} KB/s`;
const trunc = (s, n) => s.length > n ? s.slice(0, n - 1) + "..." : s;
let open = false;
let rateEma = 0;
let peakRate = 0;
let lastSample = { t: 0, done: 0 };
let activeCount = 0;
let prevDone = 0;
let prevTime = 0;
let probeRtt = null;
let startedAt = 0;
let batteryTxt = "";
let liveItems = [];
let lastTuner = null;
const SAMPLE_MS = 500;
const HIST = 110;
const hist = [];
const elText = () => document.getElementById("jb-net-text");
const elCanvas = () => document.getElementById("jb-net-canvas");
async function probe() {
  const t0 = performance.now();
  try {
    const r = await fetch(apiHealth, { cache: "no-store" });
    if (r.ok)
      probeRtt = Math.round(performance.now() - t0);
  } catch {}
}
async function readBattery() {
  try {
    const nav = navigator;
    if (!nav.getBattery)
      return;
    const b = await nav.getBattery();
    batteryTxt = `${Math.round(b.level * 100)}%${b.charging ? " charging" : ""}`;
  } catch {}
}
function sec(label, lines) {
  return [label, ...lines.map((l) => "  " + l)].join(`
`);
}
const RECORD_MS = 1000;
const MAX_SAMPLES = 7200;
const recorded = [];
setInterval(() => {
  const t = lastTuner;
  recorded.push({
    t: Math.floor(Date.now() / 1000),
    up_s: startedAt ? Math.round((performance.now() - startedAt) / 1000) : null,
    now_bps: Math.round(rateEma),
    peak_bps: Math.round(peakRate),
    done_mb: +(lastSample.done / MB).toFixed(2),
    active: activeCount,
    api_rtt_ms: probeRtt,
    workers: t ? t.w : null,
    target_workers: t ? t.t : null,
    saturated: t ? t.sat : null,
    tuner_bps: t ? Math.round(t.r) : null,
    parts_done: t ? t.c : null,
    parts_total: t ? t.n : null,
    chunk_mb: t && t.k ? +(t.k / MB).toFixed(2) : null,
    gzip: t?.g == null ? null : !!t.g,
    files: liveItems.map((i) => ({
      n: trunc(i.n, 32),
      s: i.s,
      p: Math.round(i.p * 10) / 10,
      m: i.m,
      z: i.z
    }))
  });
  if (recorded.length > MAX_SAMPLES)
    recorded.shift();
}, RECORD_MS);
function exportStamp() {
  const d = new Date;
  const p = (x) => String(x).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}
function downloadText(name, mime, text) {
  const url = URL.createObjectURL(new Blob([text], { type: mime }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 2000);
}
function metaBlock() {
  return {
    exported_at: new Date().toISOString(),
    ua: navigator.userAgent,
    battery: batteryTxt || null,
    network: netLines(),
    device: deviceLines(),
    settings: settingLines(),
    sample_interval_s: RECORD_MS / 1000,
    max_samples: MAX_SAMPLES,
    sample_count: recorded.length
  };
}
function exportJson() {
  downloadText(`juicebox-netdebug-${exportStamp()}.json`, "application/json", JSON.stringify({ meta: metaBlock(), samples: recorded.slice() }, null, 1));
}
function exportCsv() {
  const esc = (v) => {
    const s = v == null ? "" : String(v);
    return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
  };
  const header = [
    "t_iso",
    "up_s",
    "now_bps",
    "peak_bps",
    "done_mb",
    "active",
    "api_rtt_ms",
    "workers",
    "target_workers",
    "saturated",
    "tuner_bps",
    "parts_done",
    "parts_total",
    "chunk_mb",
    "gzip",
    "files"
  ];
  const rows = recorded.map((r) => [
    new Date(r.t * 1000).toISOString(),
    r.up_s,
    r.now_bps,
    r.peak_bps,
    r.done_mb,
    r.active,
    r.api_rtt_ms,
    r.workers,
    r.target_workers,
    r.saturated,
    r.tuner_bps,
    r.parts_done,
    r.parts_total,
    r.chunk_mb,
    r.gzip,
    r.files.map((f) => `${f.n}:${f.s}:${f.p}%:${f.m}:${f.z}`).join(" | ")
  ]);
  const csv = [header, ...rows].map((row) => row.map(esc).join(",")).join(`
`);
  downloadText(`juicebox-netdebug-${exportStamp()}.csv`, "text/csv", csv);
}
document.getElementById("jb-net-export-json")?.addEventListener("click", exportJson);
document.getElementById("jb-net-export-csv")?.addEventListener("click", exportCsv);
function netLines() {
  const c = navigator.connection;
  if (!c)
    return [`api: none | health probe rtt=${probeRtt ?? "?"}ms`];
  return [
    `type=${c.type ?? "?"} eff=${c.effectiveType ?? "?"} down=${c.downlink ?? "?"}Mb api_rtt=${c.rtt ?? "?"}ms`,
    `saveData=${!!c.saveData} probe_rtt=${probeRtt ?? "?"}ms`
  ];
}
function deviceLines() {
  const nav = navigator;
  const perf = performance;
  const plat = nav.userAgentData?.platform || (/Android/i.test(navigator.userAgent) ? "Android" : /iPhone|iPad/i.test(navigator.userAgent) ? "iOS" : /Mac/i.test(navigator.platform || "") ? "macOS" : /Win/i.test(navigator.platform || "") ? "Windows" : "?");
  let ram = "n/a";
  if (nav.deviceMemory)
    ram = `${nav.deviceMemory}GB`;
  else if (perf.memory?.jsHeapSizeLimit)
    ram = `~${(perf.memory.jsHeapSizeLimit / 1024 ** 3).toFixed(1)}GB heap`;
  const motion = window.__jbMotion;
  const motionTxt = motion ? ` motion=${motion.evs}ev src=${motion.src} mag=${motion.lastMag ?? "-"}` : "";
  return [
    `${plat}, ${nav.hardwareConcurrency ?? "?"} cores, ram=${ram}`,
    `touch=${nav.maxTouchPoints ?? 0} online=${navigator.onLine}${batteryTxt ? ` battery=${batteryTxt}` : ""}`,
    `ua=${trunc(navigator.userAgent, 42)}${motionTxt}`
  ];
}
function settingLines() {
  const tier = detectNetTier();
  const stored = Number(localStorage.getItem(LAST_SPEED_KEY));
  const hint = Number.isFinite(stored) && stored > 0 ? stored : 0;
  const est = hint || TIER_DEFAULT_BPS[tier];
  return [
    `tier=${tier}${hint ? " (learned)" : ""} est=${fmtRate(est)}`,
    `streams: start ${streamsForRate(est)}, cap ${PARALLEL_STREAMS}, parts >=${TUS_MIN_PART / MB}MB up to ${TUS_MAX_PARTS}`,
    `chunk: start ${TIER_CHUNK[tier].start / MB}MB max ${TIER_CHUNK[tier].max / MB}MB target ${TUS_TARGET_CHUNK_SECS}s`,
    `mode: tus> ${TUS_THRESHOLD / MB}MB | gzip: text-like ext/mime | tuner every ${TUNER_INTERVAL_MS / 1000}s`
  ];
}
function tunerLines() {
  if (!lastTuner)
    return [`no tus session yet (direct < ${TUS_THRESHOLD / MB}MB skips tuning)`];
  const t = lastTuner;
  const chunk = t.k ? t.k >= MB ? `${(t.k / MB).toFixed(1)}MB` : `${Math.round(t.k / 1024)}KB` : "-";
  return [
    `chose ${t.w} streams (target ${t.t}, cap ${PARALLEL_STREAMS})`,
    `parts ${t.c}/${t.n} | chunk ${chunk} | gzip=${t.g == null ? "probe" : t.g ? "on" : "off"}`,
    `state: ${activeCount > 0 ? t.sat ? "SATURATED" : "growing" : "idle"} | window rate ${fmtRate(t.r)}`
  ];
}
function uploadLines() {
  const it = liveItems.find((i) => i.o || i.m === "tus");
  if (!it)
    return ["no active upload"];
  const o = it.o;
  return [
    `method=${it.m}${o ? ` mode=${o.mode} quick=${o.quick} ttl=${o.ttl}h` : ""}`,
    o ? `host=${o.host || "(default)"}` : ""
  ].filter(Boolean);
}
function speedLines() {
  const remaining = liveItems.reduce((a, i) => a + Math.max(0, i.z * (100 - i.p) / 100), 0);
  const eta = rateEma > 1 && remaining > 0 ? `${String(Math.floor(remaining / rateEma / 60)).padStart(2, "0")}:${String(Math.floor(remaining / rateEma % 60)).padStart(2, "0")}` : "--:--";
  return [
    `now ${fmtRate(rateEma)} peak ${fmtRate(peakRate)} | sent ${(lastSample.done / MB).toFixed(1)}MB`,
    `eta ${eta} | elapsed ${startedAt ? ((performance.now() - startedAt) / 1000).toFixed(0) : "0"}s | active ${activeCount}`
  ];
}
function drawGraph() {
  const cv = elCanvas();
  if (!cv)
    return;
  const dpr = window.devicePixelRatio || 1;
  const w = cv.clientWidth || 440;
  const h = cv.clientHeight || 88;
  if (cv.width !== Math.round(w * dpr) || cv.height !== Math.round(h * dpr)) {
    cv.width = Math.round(w * dpr);
    cv.height = Math.round(h * dpr);
  }
  const ctx = cv.getContext("2d");
  if (!ctx)
    return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  const pad = 2;
  const max = Math.max(...hist, 512 * 1024);
  ctx.font = "9px ui-monospace, monospace";
  ctx.fillStyle = "rgba(255,255,255,.55)";
  ctx.strokeStyle = "rgba(255,255,255,.14)";
  ctx.lineWidth = 1;
  for (let q = 0;q <= 4; q++) {
    const y = pad + (h - pad * 2) * q / 4;
    ctx.beginPath();
    ctx.moveTo(pad, Math.round(y) + 0.5);
    ctx.lineTo(w - pad, Math.round(y) + 0.5);
    ctx.stroke();
    const val = max * (4 - q) / 4;
    ctx.fillText(val >= MB ? `${(val / MB).toFixed(0)}M` : `${Math.round(val / 1024)}K`, w - 24, y + 8 >= h ? h - 2 : y + 8);
  }
  if (hist.length < 2) {
    ctx.fillStyle = "rgba(255,255,255,.6)";
    ctx.fillText("waiting for samples", pad + 2, h - 6);
    return;
  }
  const stepX = (w - pad * 2 - 26) / (HIST - 1);
  const x0 = pad;
  const yOf = (v) => pad + (h - pad * 2) * (1 - v / max);
  ctx.beginPath();
  ctx.moveTo(x0, h - pad);
  hist.forEach((v, i) => ctx.lineTo(x0 + i * stepX, yOf(v)));
  ctx.lineTo(x0 + (hist.length - 1) * stepX, h - pad);
  ctx.closePath();
  ctx.fillStyle = "rgba(255,255,255,.09)";
  ctx.fill();
  ctx.beginPath();
  hist.forEach((v, i) => {
    const x = x0 + i * stepX;
    const y = yOf(v);
    i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y);
  });
  ctx.strokeStyle = "#fff";
  ctx.lineWidth = 1.6;
  ctx.stroke();
  const hx = x0 + (hist.length - 1) * stepX;
  const hy = yOf(hist[hist.length - 1]);
  ctx.beginPath();
  ctx.arc(hx, hy, 2.4, 0, Math.PI * 2);
  ctx.fillStyle = "#fff";
  ctx.fill();
}
function render() {
  const txt = elText();
  if (!txt || !open)
    return;
  const now = performance.now();
  if (activeCount > 0 && prevTime > 0) {
    const rate = Math.max(0, lastSample.done - prevDone) / Math.max(1, now - prevTime) * 1000;
    rateEma = rateEma ? rateEma * 0.6 + rate * 0.4 : rate;
    peakRate = Math.max(peakRate, rateEma);
  }
  prevDone = lastSample.done;
  prevTime = now;
  const fresh = liveItems.find((i) => i.d && typeof i.d.g !== "undefined" && (i.s === "uploading" || i.s === "finalizing"))?.d ?? liveItems.find((i) => i.d && typeof i.d.g !== "undefined")?.d;
  if (fresh)
    lastTuner = fresh;
  txt.textContent = [
    "-- NET DEBUG --",
    sec("[NETWORK]", netLines()),
    sec("[DEVICE]", deviceLines()),
    sec("[SETTINGS]", settingLines()),
    sec("[UPLOAD]", uploadLines()),
    sec("[TUNER]", tunerLines()),
    sec("[SPEED]", speedLines()),
    liveItems.length ? sec("[FILES]", liveItems.map((i) => `${trunc(i.n, 18)} ${i.s}:${i.m} ${Math.round(i.p)}% (${(i.z * i.p / 100 / MB).toFixed(1)}MB)`)) : ""
  ].filter(Boolean).join(`
`);
  drawGraph();
}
setInterval(() => {
  hist.push(activeCount > 0 ? rateEma : 0);
  if (hist.length > HIST)
    hist.shift();
}, SAMPLE_MS);
window.addEventListener("jb-net-sample", (ev) => {
  const d = ev.detail;
  if (d.active > 0 && !startedAt)
    startedAt = performance.now();
  if (d.active === 0)
    startedAt = 0;
  lastSample = { t: performance.now(), done: d.done };
  activeCount = d.active;
  liveItems = d.items ?? [];
  if (open)
    render();
});
function toggle() {
  open = !open;
  const node = document.getElementById("jb-net-debug");
  if (!node)
    return;
  node.hidden = !open;
  if (open) {
    render();
    probe();
    readBattery();
  }
  try {
    navigator.vibrate?.(12);
  } catch {}
}
let spikes = 0;
let lastSpike = 0;
let lastMag = null;
const DOUBLE_SHAKE_MS = 1400;
const GESTURE_COOLDOWN_MS = 600;
let armed = false;
let armedAt = 0;
let suppressUntil = 0;
const magOf = (o) => o && o.x != null && o.y != null && o.z != null ? Math.hypot(o.x, o.y, o.z) : null;
window.__jbMotion = {
  evs: 0,
  lastMag: null,
  src: "-",
  state: "idle",
  gestures: 0
};
window.addEventListener("devicemotion", (e) => {
  if (!("ontouchstart" in window))
    return;
  const dbg = window.__jbMotion;
  dbg.evs++;
  const incl = magOf(e.accelerationIncludingGravity);
  const mag = incl ?? magOf(e.acceleration);
  dbg.lastMag = mag?.toFixed(1) ?? null;
  dbg.src = incl != null ? "g" : mag != null ? "a" : "-";
  if (mag == null)
    return;
  const now = Date.now();
  const delta = lastMag == null ? 0 : Math.abs(mag - lastMag);
  lastMag = mag;
  if (mag < 23 && delta < 17)
    return;
  if (now < suppressUntil)
    return;
  if (now - lastSpike > 80) {
    spikes++;
    lastSpike = now;
  }
  if (spikes >= 3) {
    spikes = 0;
    suppressUntil = now + GESTURE_COOLDOWN_MS;
    dbg.gestures++;
    if (armed && now - armedAt <= DOUBLE_SHAKE_MS) {
      armed = false;
      armedAt = 0;
      dbg.state = "toggle";
      try {
        navigator.vibrate?.([30, 40, 30]);
      } catch {}
      toggle();
    } else {
      armed = true;
      armedAt = now;
      dbg.state = "armed";
      try {
        navigator.vibrate?.(15);
      } catch {}
    }
  }
});
setInterval(() => {
  const t = Date.now();
  if (t - lastSpike > 900) {
    spikes = 0;
    lastMag = null;
  }
  if (armed && t - armedAt > DOUBLE_SHAKE_MS) {
    armed = false;
    armedAt = 0;
    const dbg = window.__jbMotion;
    if (dbg)
      dbg.state = "idle";
  }
}, 500);
window.addEventListener("touchend", () => {
  const dm = window.DeviceMotionEvent;
  dm?.requestPermission?.().catch(() => {});
}, { once: true, passive: true });
window.addEventListener("keydown", (e) => {
  if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "d") {
    e.preventDefault();
    toggle();
  }
});
setInterval(() => {
  if (!open || (typeof document !== "undefined" && document.hidden))
    return;
  probe();
  render();
}, 2000);
document.addEventListener("DOMContentLoaded", () => {
  const node = document.getElementById("jb-net-debug");
  if (node)
    node.hidden = !open;
});
