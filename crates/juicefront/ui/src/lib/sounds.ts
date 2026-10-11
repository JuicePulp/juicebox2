import { bind, play, setEnabled, type PlayOptions, type SoundName } from "cuelume";

const STORE_KEY = "juicebox_sound";
const BURST_GAP_MS = 600;
const lastBurst = new Map<string, number>();
let bound = false;

function storedEnabled(): boolean {
  try {
    return localStorage.getItem(STORE_KEY) !== "0";
  } catch {
    return true;
  }
}

export function applySoundEnabled(on: boolean): void {
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

export function playSound(name: SoundName, opts?: PlayOptions): void {
  if (!storedEnabled()) return;
  try {
    const now = Date.now();
    if (now - (lastBurst.get(name) ?? 0) < BURST_GAP_MS) return;
    lastBurst.set(name, now);
    play(name, opts);
  } catch {}
}

export function toggleSound(): boolean {
  const next = !storedEnabled();
  applySoundEnabled(next);
  if (next) playSound("toggle");
  return next;
}

export function bindSounds(): void {
  try {
    if (!bound) {
      bound = true;
      bind();
      warmUpAudio();
    }
    applySoundEnabled(storedEnabled());
  } catch {}
}

// Warm up the shared AudioContext on the first pointer press, so the cue on
// the very first click - very often a navigation link - starts instantly.
// Silent and one-shot; navigation itself is never delayed.
function warmUpAudio(): void {
  const warmup = (): void => {
    document.removeEventListener("pointerdown", warmup, true);
    if (!storedEnabled()) return;
    try {
      play("tap", { volume: 0 });
    } catch {}
  };
  document.addEventListener("pointerdown", warmup, true);
}

declare global {
  interface Window {
    JuiceSounds?: {
      play: typeof playSound;
      toggle: typeof toggleSound;
      isEnabled: () => boolean;
    };
  }
}

try {
  window.JuiceSounds = {
    play: playSound,
    toggle: toggleSound,
    isEnabled: storedEnabled,
  };
} catch {}
