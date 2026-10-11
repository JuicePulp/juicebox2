import { readAllowedTtlHours, readDefaultTtlHours } from "./upload-config.js";
import { ttlLabel } from "./i18n.js";
import { playSound } from "./sounds.js";

const TTL_STORAGE_KEY = "juicebox_ttl_hours";

function readStoredTtl() {
  try {
    return localStorage.getItem(TTL_STORAGE_KEY);
  } catch {
    return null;
  }
}

function storeTtl(hours) {
  try {
    localStorage.setItem(TTL_STORAGE_KEY, hours);
  } catch {}
}

function ttlTier(hours) {
  const h = Number(hours);
  if (!Number.isFinite(h)) return 1;
  if (h < 1) return 0;
  if (Number.isInteger(h / 24)) return 2;
  return 1;
}

function buildOptions(fieldset, locale, allowed, def, stored) {
  fieldset.innerHTML = "";
  let prevTier = null;
  let checkedValue = null;
  if (stored !== null && allowed.some((h) => String(h) === stored)) {
    checkedValue = stored;
  } else if (allowed.includes(def)) {
    checkedValue = String(def);
  } else if (allowed.length > 0) {
    checkedValue = String(allowed[0]);
  }
  for (const hours of allowed) {
    const tier = ttlTier(hours);
    if (prevTier !== null && prevTier !== tier) {
      const divider = document.createElement("span");
      divider.className = "rtt-divider";
      divider.setAttribute("aria-hidden", "true");
      fieldset.appendChild(divider);
    }
    prevTier = tier;
    const label = document.createElement("label");
    label.className = "rtt-option";
    const input = document.createElement("input");
    input.type = "radio";
    input.name = "ttl_hours";
    input.value = String(hours);
    input.checked = String(hours) === checkedValue;
    const span = document.createElement("span");
    span.textContent = ttlLabel(locale, hours);
    label.append(input, span);
    fieldset.appendChild(label);
  }
  return Array.from(fieldset.querySelectorAll('input[type="radio"]'));
}

function resolveOptions(card, locale) {
  const fieldset = card.querySelector(".rtt-options");
  if (!fieldset) return null;
  const allowed = readAllowedTtlHours();
  const def = readDefaultTtlHours();
  if (allowed.length === 0) {
    return Array.from(fieldset.querySelectorAll('input[type="radio"]'));
  }
  const stored = readStoredTtl();
  return buildOptions(fieldset, locale, allowed, def, stored);
}

function syncStored(card) {
  const stored = readStoredTtl();
  if (stored === null) return;
  const radios = Array.from(card.querySelectorAll('.rtt-options input[type="radio"]'));
  if (radios.some((r) => r.value === stored)) {
    radios.forEach((r) => (r.checked = r.value === stored));
  }
}

export function enhanceRetention(card, locale = "en") {
  const toolbar = card.querySelector("[data-rttoolbar]");
  const fieldset = toolbar?.querySelector(".rtt-options");
  if (!toolbar || !fieldset) return;
  const radios = resolveOptions(card, locale);
  if (!radios || radios.length === 0) return;
  // The radios intentionally carry no data-cuelume-select: cuelume would
  // tick on its own terms, but retention ticks are played here instead —
  // one per change, spammable like every other sound (the shared burst
  // gate in playSound is the only throttle).
  let lastTicked = radios.find((r) => r.checked)?.value ?? null;
  const orderOf = (value) => radios.findIndex((r) => r.value === value);
  const onChange = (e) => {
    const target = e.target;
    if (target && target.name === "ttl_hours" && target.checked) {
      storeTtl(target.value);
      const direction = orderOf(target.value) >= orderOf(lastTicked) ? "forward" : "back";
      lastTicked = target.value;
      playSound("select", { emphasis: "subtle", direction });
    }
  };
  fieldset.addEventListener("change", onChange);
  return {
    destroy() {
      fieldset.removeEventListener("change", onChange);
    },
  };
}

export function rebuildRetention(card, locale = "en", destroyFn) {
  if (destroyFn) destroyFn();
  const fieldset = card.querySelector(".rtt-options");
  if (fieldset) {
    const allowed = readAllowedTtlHours();
    const def = readDefaultTtlHours();
    if (allowed.length > 0) {
      const stored = readStoredTtl();
      buildOptions(fieldset, locale, allowed, def, stored);
    } else {
      syncStored(card);
    }
  } else {
    syncStored(card);
  }
  return enhanceRetention(card, locale);
}
