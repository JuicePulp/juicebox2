// Toggled by the g g g g chord. When off, uploads use a fixed default stream
// count and the adaptive tuner leaves it alone.
export const TWEAK_ENGINE_KEY = "juicebox_tweak_engine";

export function tweakEngineEnabled() {
  try {
    return localStorage.getItem(TWEAK_ENGINE_KEY) !== "off";
  } catch {
    return true;
  }
}

export function setTweakEngineEnabled(enabled) {
  try {
    localStorage.setItem(TWEAK_ENGINE_KEY, enabled ? "on" : "off");
  } catch {}
}
