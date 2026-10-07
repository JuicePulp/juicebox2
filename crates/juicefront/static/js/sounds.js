import { bind, play, setEnabled } from "./vendor/cuelume/index.js";

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
  bind();
} catch {}
applyEnabled(storedEnabled());

export function playSound(name, opts) {
  sound(name, opts);
}

window.JuiceSounds = {
  play: sound,
  toggle: toggle,
  isEnabled: storedEnabled,
};
