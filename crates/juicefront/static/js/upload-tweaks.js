export const TWEAK_ENGINE_KEY = "juicebox_tweak_engine";

/** Defaults to on: only the literal value `"off"` disables the engine. */
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
