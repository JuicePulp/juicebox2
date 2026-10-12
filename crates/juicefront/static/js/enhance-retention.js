import { readAllowedTtlHours, readDefaultTtlHours } from "./upload-config.js";
import { ttlLabel } from "./i18n.js";
import { playSound } from "./sounds.js";
import { soundEnabled } from "./sounds.js";
import { play } from "./vendor/cuelume/index.js";
import { haptic } from "./haptics.js";

function scrubTick(direction) {
  if (!soundEnabled()) return;
  try {
    play("select", { emphasis: "subtle", volume: 0.35, direction: direction });
  } catch {}
}

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
  let tierEl = null;
  const ensureTier = () => {
    if (!tierEl) {
      tierEl = document.createElement("div");
      tierEl.className = "rtt-tier";
      fieldset.appendChild(tierEl);
    }
    return tierEl;
  };
  for (const hours of allowed) {
    const tier = ttlTier(hours);
    if (prevTier !== null && prevTier !== tier) {
      const divider = document.createElement("span");
      divider.className = "rtt-divider";
      divider.setAttribute("aria-hidden", "true");
      fieldset.appendChild(divider);
      tierEl = null;
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
    ensureTier().appendChild(label);
  }
  return Array.from(fieldset.querySelectorAll('input[type="radio"]'));
}

function domValues(fieldset) {
  return Array.from(fieldset.querySelectorAll('input[type="radio"]')).map((r) => r.value);
}

function sameValueSet(a, b) {
  if (a.length !== b.length) return false;
  const set = new Set(a);
  return b.every((v) => set.has(v));
}

function applyStoredCheck(fieldset, allowed, def, stored) {
  var want = null;
  if (stored !== null && allowed.some((h) => String(h) === stored)) {
    want = stored;
  } else if (allowed.includes(def)) {
    want = String(def);
  } else if (allowed.length > 0) {
    want = String(allowed[0]);
  }
  if (want === null) return;
  fieldset.querySelectorAll('input[type="radio"]').forEach(function (r) {
    r.checked = r.value === want;
  });
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
  // The server already rendered the host's tiers: only refresh the checked
  // state, never rebuild identical options (keeps JS and no-JS users on the
  // same host-advertised list).
  if (sameValueSet(domValues(fieldset), allowed.map(String))) {
    applyStoredCheck(fieldset, allowed, def, stored);
    return Array.from(fieldset.querySelectorAll('input[type="radio"]'));
  }
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

function splitRetentionLabel(label) {
  const m = String(label || "").match(/^(\d+(?:\.\d+)?)(.*)$/);
  const num = m ? m[1] : "";
  const unit = m ? m[2].trim() : "";
  const hasS = unit.endsWith("s") && unit.length > 1;
  return {
    number: num,
    unit: unit,
    base: hasS ? unit.slice(0, -1) : unit,
    hasS: hasS,
  };
}

function ensureValueWrap(labelGroup) {
  let wrap = labelGroup.querySelector(".rttvalue-container");
  if (!wrap) {
    wrap = document.createElement("div");
    wrap.className = "rttvalue-container";
    wrap.setAttribute("aria-hidden", "true");
    labelGroup.appendChild(wrap);
  }
  return wrap;
}

export function enhanceRetention(card, locale = "en") {
  const toolbar = card.querySelector("[data-rttoolbar]");
  const fieldset = toolbar?.querySelector(".rtt-options");
  if (!toolbar || !fieldset) return;
  const radios = resolveOptions(card, locale);
  if (!radios || radios.length === 0) return;
  const labelGroup = toolbar.querySelector(".rttlabel-group");
  const labels = radios.map((r) => (r.nextElementSibling && r.nextElementSibling.textContent || "").trim());
  const valueWrap = labelGroup ? ensureValueWrap(labelGroup) : null;
  let lastIndex = radios.findIndex((r) => r.checked);
  if (lastIndex < 0) lastIndex = 0;
  let cleanupTimer = 0;

  const setValue = (i) => {
    if (!valueWrap) return;
    const nw = splitRetentionLabel(labels[i] || "");

    let wrapper = valueWrap.querySelector(".rttvalue-wrapper");
    if (!wrapper) {
      wrapper = document.createElement("div");
      wrapper.className = "rttvalue-wrapper";
      wrapper.innerHTML =
        '<span class="rttnumber"></span><span class="rttunit-base"></span>';
      wrapper.querySelector(".rttnumber").textContent = nw.number;
      wrapper.querySelector(".rttunit-base").textContent = nw.base;
      if (nw.hasS) {
        const s = document.createElement("span");
        s.className = "rttunit-s";
        s.textContent = "s";
        wrapper.appendChild(s);
      }
      valueWrap.appendChild(wrapper);
      lastIndex = i;
      return;
    }
    if (i === lastIndex) return;

    const isUp = i > lastIndex;
    const exitClass = isUp ? "animate-up-exit" : "animate-down-exit";
    const enterClass = isUp ? "animate-up-enter" : "animate-down-enter";
    const old = splitRetentionLabel(labels[lastIndex] || "");

    const numberEl = wrapper.querySelector(".rttnumber");
    const baseEl = wrapper.querySelector(".rttunit-base");
    let suffixEl = wrapper.querySelector(".rttunit-s");
    const baseChanged = old.base !== nw.base;

    const wr = wrapper.getBoundingClientRect();
    const toRemove = [];
    const cloneExit = (el, cls) => {
      const rect = el.getBoundingClientRect();
      const exit = el.cloneNode(true);
      exit.removeAttribute("id");
      exit.dataset.exitClone = "";
      exit.style.position = "absolute";
      exit.style.left = (rect.left - wr.left) + "px";
      exit.style.top = (rect.top - wr.top) + "px";
      exit.style.width = rect.width + "px";
      exit.style.margin = "0";
      exit.style.pointerEvents = "none";
      exit.className = cls;
      wrapper.appendChild(exit);
      toRemove.push(exit);
    };
    clearTimeout(cleanupTimer);
    wrapper.querySelectorAll("[data-exit-clone]").forEach((el) => el.remove());

    cloneExit(numberEl, "rttnumber " + exitClass);
    if (baseChanged) cloneExit(baseEl, "rttunit-base " + exitClass);

    const suffixChanges = old.hasS !== nw.hasS || old.base !== nw.base;
    if (suffixChanges && suffixEl) {
      cloneExit(suffixEl, "rttunit-s " + exitClass);
      suffixEl.remove();
      suffixEl = null;
    }

    numberEl.textContent = nw.number;
    numberEl.className = "rttnumber";
    baseEl.textContent = nw.base;
    baseEl.className = "rttunit-base";
    if (suffixChanges && nw.hasS) {
      const s = document.createElement("span");
      s.className = "rttunit-s " + enterClass;
      s.textContent = "s";
      wrapper.appendChild(s);
      window.setTimeout(() => {
        s.className = "rttunit-s";
      }, 200);
    }
    requestAnimationFrame(() => {
      numberEl.classList.add(enterClass);
      if (baseChanged) baseEl.classList.add(enterClass);
    });
    cleanupTimer = window.setTimeout(() => {
      toRemove.forEach((el) => el.remove());
    }, 200);
    lastIndex = i;
  };
  setValue(lastIndex);

  const stabilizeValueWidth = () => {
    if (!valueWrap) return;
    const wrapper = valueWrap.querySelector(".rttvalue-wrapper");
    if (!wrapper) return;
    const numberEl = wrapper.querySelector(".rttnumber");
    const baseEl = wrapper.querySelector(".rttunit-base");
    if (!numberEl || !baseEl) return;
    const prevNumber = numberEl.textContent || "";
    const prevBase = baseEl.textContent || "";
    const liveS = wrapper.querySelector(".rttunit-s:not([data-exit-clone])");
    if (liveS) liveS.remove();
    let max = 0;
    for (const text of labels) {
      const part = splitRetentionLabel(text);
      numberEl.textContent = part.number;
      baseEl.textContent = part.base;
      let tmp = null;
      if (part.hasS) {
        tmp = document.createElement("span");
        tmp.className = "rttunit-s";
        tmp.textContent = "s";
        wrapper.appendChild(tmp);
      }
      max = Math.max(max, wrapper.offsetWidth);
      if (tmp) tmp.remove();
    }
    numberEl.textContent = prevNumber;
    baseEl.textContent = prevBase;
    const current = splitRetentionLabel(labels[lastIndex] || "");
    if (current.hasS && !wrapper.querySelector(".rttunit-s")) {
      const s = document.createElement("span");
      s.className = "rttunit-s";
      s.textContent = "s";
      wrapper.appendChild(s);
    }
    valueWrap.style.minWidth = Math.ceil(max) + "px";
  };
  stabilizeValueWidth();
  try {
    if (document.fonts && document.fonts.ready) {
      document.fonts.ready.then(() => {
        stabilizeValueWidth();
      });
    }
  } catch {}

  let lastTicked = radios.find((r) => r.checked)?.value ?? null;
  const orderOf = (value) => radios.findIndex((r) => r.value === value);
  let lastDirection = "forward";
  const applyIndex = (i) => {
    const radio = radios[i];
    if (!radio || radio.checked) return;
    radio.checked = true;
    lastDirection = i >= orderOf(lastTicked) ? "forward" : "back";
    lastTicked = radio.value;
    storeTtl(radio.value);
    setValue(i);
    scrubTick(lastDirection);
    haptic("tick");
  };
  const onChange = (e) => {
    const target = e.target;
    if (target && target.name === "ttl_hours" && target.checked) {
      storeTtl(target.value);
      const direction = orderOf(target.value) >= orderOf(lastTicked) ? "forward" : "back";
      lastTicked = target.value;
      playSound("select", { emphasis: "subtle", direction });
      const i = radios.findIndex((r) => r.value === target.value);
      if (i >= 0) setValue(i);
    }
  };
  fieldset.addEventListener("change", onChange);

  let drag = null;
  const indexFromX = (clientX) => {
    const rect = fieldset.getBoundingClientRect();
    if (rect.width <= 0) return lastIndex < 0 ? 0 : lastIndex;
    const x = Math.min(Math.max(clientX - rect.left, 0), rect.width);
    return Math.min(radios.length - 1, Math.max(0, Math.floor((x / rect.width) * radios.length)));
  };
  const onPointerDown = (e) => {
    if (drag || e.pointerType !== "mouse" || e.button !== 0) return;
    if (getComputedStyle(fieldset).flexDirection !== "row") return;
    drag = { id: e.pointerId, startX: e.clientX, moved: false };
  };
  const ensureThumb = () => {
    let thumb = fieldset.querySelector(".rtt-dragthumb");
    if (!thumb) {
      thumb = document.createElement("div");
      thumb.className = "rtt-dragthumb";
      thumb.setAttribute("aria-hidden", "true");
      fieldset.appendChild(thumb);
    }
    return thumb;
  };
  const currentLabelText = () => {
    const checked = fieldset.querySelector('input[name="ttl_hours"]:checked');
    const label = checked ? checked.closest("label") : null;
    const span = label ? label.querySelector("span") : null;
    return (span && span.textContent || "").trim();
  };
  const moveThumb = (clientX) => {
    const thumb = ensureThumb();
    const fieldRect = fieldset.getBoundingClientRect();
    const x = Math.min(Math.max(clientX, fieldRect.left), fieldRect.right);
    thumb.style.left = (x - fieldRect.left) + "px";
  };
  const moveThumbToSelection = (thumb) => {
    const checked = fieldset.querySelector('input[name="ttl_hours"]:checked');
    const label = checked ? checked.closest("label") : null;
    if (!label) return;
    const fieldRect = fieldset.getBoundingClientRect();
    const labelRect = label.getBoundingClientRect();
    thumb.style.left = (labelRect.left + labelRect.width / 2 - fieldRect.left) + "px";
  };
  const onPointerMove = (e) => {
    if (!drag || e.pointerId !== drag.id) return;
    if (!drag.moved && Math.abs(e.clientX - drag.startX) < 8) return;
    if (!drag.moved) {
      drag.moved = true;
      try {
        fieldset.setPointerCapture(e.pointerId);
      } catch {}
      fieldset.classList.add("rtt-dragging");
      const thumb = ensureThumb();
      thumb.textContent = currentLabelText();
      thumb.classList.add("on");
    }
    drag.moved = true;
    e.preventDefault();
    applyIndex(indexFromX(e.clientX));
    const thumb = ensureThumb();
    thumb.textContent = currentLabelText();
    moveThumb(e.clientX);
  };
  const endDrag = (e) => {
    if (!drag || e.pointerId !== drag.id) return;
    const wasDrag = drag.moved;
    drag = null;
    fieldset.classList.remove("rtt-dragging");
    const thumb = fieldset.querySelector(".rtt-dragthumb");
    if (thumb && wasDrag) {
      thumb.style.transition = "left 0.18s ease-out, opacity 0.15s ease-out";
      moveThumbToSelection(thumb);
      window.setTimeout(() => {
        thumb.style.transition = "opacity 0.15s ease-out";
        thumb.classList.remove("on");
      }, 190);
      window.setTimeout(() => {
        thumb.remove();
      }, 380);
    } else if (thumb) {
      thumb.remove();
    }
    if (wasDrag) {
      playSound("select", { emphasis: "subtle", direction: lastDirection });
      haptic("tap");
    }
  };
  fieldset.addEventListener("pointerdown", onPointerDown);
  fieldset.addEventListener("pointermove", onPointerMove);
  fieldset.addEventListener("pointerup", endDrag);
  fieldset.addEventListener("pointercancel", endDrag);
  return {
    destroy() {
      fieldset.removeEventListener("change", onChange);
      fieldset.removeEventListener("pointerdown", onPointerDown);
      fieldset.removeEventListener("pointermove", onPointerMove);
      fieldset.removeEventListener("pointerup", endDrag);
      fieldset.removeEventListener("pointercancel", endDrag);
      drag = null;
      fieldset.classList.remove("rtt-dragging");
      clearTimeout(cleanupTimer);
      if (valueWrap) {
        valueWrap.querySelectorAll("[data-exit-clone]").forEach((el) => el.remove());
        valueWrap.remove();
      }
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
