import { bind, play, setEnabled } from "./vendor/cuelume/index.js?v=2.1.32";

var STORE_KEY = "juicebox_sound";
var lastBurst = Object.create(null);
var BURST_GAP_MS = 600;

function storedEnabled() {
  try {
    var raw = localStorage.getItem(STORE_KEY);
    if (raw === "0") return false;
  } catch {}
  return true;
}

export function soundEnabled() {
  return storedEnabled();
}

function applyEnabled(on) {
  try {
    setEnabled(on);
  } catch {}
  try {
    localStorage.setItem(STORE_KEY, on ? "1" : "0");
  } catch {}
  try {
    document.documentElement.toggleAttribute("data-sound-off", !on);
  } catch {}
}

function sound(name, opts) {
  if (!storedEnabled()) return;
  try {
    var now = Date.now();
    if (now - (lastBurst[name] || 0) < BURST_GAP_MS) return;
    lastBurst[name] = now;
    play(name, opts);
  } catch {}
}

function toggle() {
  var next = !storedEnabled();
  applyEnabled(next);
  if (next) sound("toggle");
  return next;
}

try {
  document.addEventListener(
    "pointerdown",
    function warmup() {
      document.removeEventListener("pointerdown", warmup, true);
      if (!storedEnabled()) return;
      try {
        play("tap", { volume: 0 });
      } catch {}
    },
    true,
  );
} catch {}

try {
  // A page can end up with both the versioned entry module and an older
  // unversioned copy from the module cache. Bind the document only once so
  // key and pointer cues do not play twice.
  if (typeof window !== "undefined" && !window.__juiceboxCuelumeBound) {
    window.__juiceboxCuelumeBound = true;
    bind();
  }
} catch {}
applyEnabled(storedEnabled());

export function playSound(name, opts) {
  sound(name, opts);
}

export function setSoundEnabled(on) {
  applyEnabled(!!on);
  if (on) sound("toggle");
}

window.JuiceSounds = {
  play: sound,
  toggle: toggle,
  isEnabled: storedEnabled,
};
