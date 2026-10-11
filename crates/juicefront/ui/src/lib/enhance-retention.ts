





import type { Locale } from "../i18n";
import { readAllowedTtlHours, readDefaultTtlHours } from "./upload-config";
import { ttlLabel, ttlTier } from "./retention";
import { playSound } from "./sounds";
import { soundEnabled } from "./sounds";
import { play } from "cuelume";
import { haptic } from "./haptics";

function scrubTick(direction: "forward" | "back"): void {
    if (!soundEnabled()) return;
    try {
        play("select", { emphasis: "subtle", volume: 0.35, direction });
    } catch {}
}

const TTL_STORAGE_KEY = "juicebox_ttl_hours";

function readStoredTtl(): string | null {
    try {
        return localStorage.getItem(TTL_STORAGE_KEY);
    } catch {
        return null;
    }
}

function storeTtl(hours: string) {
    try {
        localStorage.setItem(TTL_STORAGE_KEY, hours);
    } catch {
    }
}

function buildOptions(
    fieldset: HTMLElement,
    locale: Locale,
    allowed: number[],
    def: number,
    stored: string | null,
): HTMLInputElement[] {
    fieldset.innerHTML = "";
    let prevTier: number | null = null;
    let checkedValue: string | null = null;
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
    return Array.from(
        fieldset.querySelectorAll('input[type="radio"]'),
    ) as HTMLInputElement[];
}

function resolveOptions(card: Element, locale: Locale): HTMLInputElement[] | null {
    const fieldset = card.querySelector(".rtt-options") as HTMLElement | null;
    if (!fieldset) return null;

    const allowed = readAllowedTtlHours();
    const def = readDefaultTtlHours();
    if (allowed.length === 0) {
        return Array.from(
            fieldset.querySelectorAll('input[type="radio"]'),
        ) as HTMLInputElement[];
    }

    const stored = readStoredTtl();
    return buildOptions(fieldset, locale, allowed, def, stored);
}

function splitRetentionLabel(label: string): {
    number: string;
    unit: string;
    base: string;
    hasS: boolean;
} {
    const m = label.match(/^(\d+(?:\.\d+)?)(.*)$/);
    const num = m ? m[1] : "";
    const unit = m ? m[2].trim() : "";
    const hasS = unit.endsWith("s") && unit.length > 1;
    return {
        number: num,
        unit,
        base: hasS ? unit.slice(0, -1) : unit,
        hasS,
    };
}

function ensureValueWrap(labelGroup: Element): HTMLElement {
    let wrap = labelGroup.querySelector(".rttvalue-container") as HTMLElement | null;
    if (!wrap) {
        wrap = document.createElement("div");
        wrap.className = "rttvalue-container";
        wrap.setAttribute("aria-hidden", "true");
        labelGroup.appendChild(wrap);
    }
    return wrap;
}

export function enhanceRetention(card: Element, locale: Locale = "en"): { destroy: () => void } | void {
    const toolbar = card.querySelector(
        "[data-rttoolbar]",
    ) as HTMLElement | null;
    const fieldset = toolbar?.querySelector(
        ".rtt-options",
    ) as HTMLElement | null;
    if (!toolbar || !fieldset) return;

    const radios = resolveOptions(card, locale);
    if (!radios || radios.length === 0) return;

    const labelGroup = toolbar.querySelector(".rttlabel-group");
    const labels = radios.map(
        (r) => r.nextElementSibling?.textContent?.trim() ?? "",
    );
    const valueWrap = labelGroup ? ensureValueWrap(labelGroup) : null;
    let lastIndex = radios.findIndex((r) => r.checked);
    if (lastIndex < 0) lastIndex = 0;
    let cleanupTimer = 0;

    const setValue = (i: number): void => {
        if (!valueWrap) return;
        const nw = splitRetentionLabel(labels[i] ?? "");

        let wrapper = valueWrap.querySelector(
            ".rttvalue-wrapper",
        ) as HTMLElement | null;
        if (!wrapper) {
            wrapper = document.createElement("div");
            wrapper.className = "rttvalue-wrapper";
            wrapper.innerHTML =
                '<span class="rttnumber"></span><span class="rttunit-base"></span>';
            (wrapper.querySelector(".rttnumber") as HTMLElement).textContent = nw.number;
            (wrapper.querySelector(".rttunit-base") as HTMLElement).textContent = nw.base;
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
        const old = splitRetentionLabel(labels[lastIndex] ?? "");

        const numberEl = wrapper.querySelector(".rttnumber") as HTMLElement;
        const baseEl = wrapper.querySelector(".rttunit-base") as HTMLElement;
        let suffixEl = wrapper.querySelector(".rttunit-s") as HTMLElement | null;
        const baseChanged = old.base !== nw.base;

        const wr = wrapper.getBoundingClientRect();
        const toRemove: HTMLElement[] = [];
        const cloneExit = (el: HTMLElement, cls: string): void => {
            const rect = el.getBoundingClientRect();
            const exit = el.cloneNode(true) as HTMLElement;
            exit.removeAttribute("id");
            exit.dataset.exitClone = "";
            exit.style.position = "absolute";
            exit.style.left = `${rect.left - wr.left}px`;
            exit.style.top = `${rect.top - wr.top}px`;
            exit.style.width = `${rect.width}px`;
            exit.style.margin = "0";
            exit.style.pointerEvents = "none";
            exit.className = cls;
            wrapper!.appendChild(exit);
            toRemove.push(exit);
        };
        clearTimeout(cleanupTimer);
        wrapper
            .querySelectorAll("[data-exit-clone]")
            .forEach((el) => el.remove());

        cloneExit(numberEl, `rttnumber ${exitClass}`);
        if (baseChanged) cloneExit(baseEl, `rttunit-base ${exitClass}`);

        const suffixChanges = old.hasS !== nw.hasS || old.base !== nw.base;
        if (suffixChanges && suffixEl) {
            cloneExit(suffixEl, `rttunit-s ${exitClass}`);
            suffixEl.remove();
            suffixEl = null;
        }

        numberEl.textContent = nw.number;
        numberEl.className = "rttnumber";
        baseEl.textContent = nw.base;
        baseEl.className = "rttunit-base";
        if (suffixChanges && nw.hasS) {
            const s = document.createElement("span");
            s.className = `rttunit-s ${enterClass}`;
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

    // Lock the value display to the widest option so scrubbing between
    // short and long labels ("1 hour" vs "30 minutes") never reflows the
    // header around it.
    const stabilizeValueWidth = (): void => {
        if (!valueWrap) return;
        const wrapper = valueWrap.querySelector(
            ".rttvalue-wrapper",
        ) as HTMLElement | null;
        if (!wrapper) return;
        const numberEl = wrapper.querySelector(".rttnumber") as HTMLElement | null;
        const baseEl = wrapper.querySelector(".rttunit-base") as HTMLElement | null;
        if (!numberEl || !baseEl) return;
        const prevNumber = numberEl.textContent ?? "";
        const prevBase = baseEl.textContent ?? "";
        const liveS = wrapper.querySelector(
            ".rttunit-s:not([data-exit-clone])",
        ) as HTMLElement | null;
        if (liveS) liveS.remove();
        let max = 0;
        for (const text of labels) {
            const part = splitRetentionLabel(text);
            numberEl.textContent = part.number;
            baseEl.textContent = part.base;
            let tmp: HTMLElement | null = null;
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
        const current = splitRetentionLabel(labels[lastIndex] ?? "");
        if (current.hasS && !wrapper.querySelector(".rttunit-s")) {
            const s = document.createElement("span");
            s.className = "rttunit-s";
            s.textContent = "s";
            wrapper.appendChild(s);
        }
        valueWrap.style.minWidth = `${Math.ceil(max)}px`;
    };
    stabilizeValueWidth();
    try {
        void document.fonts?.ready.then(() => {
            stabilizeValueWidth();
        });
    } catch {}

    
    
    
    
    let lastTicked: string | null = radios.find((r) => r.checked)?.value ?? null;
    const orderOf = (value: string | null): number => radios.findIndex((r) => r.value === value);
    let lastDirection: "forward" | "back" = "forward";
    const applyIndex = (i: number): void => {
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
    const onChange = (e: Event) => {
        const target = e.target as HTMLInputElement | null;
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

    let drag: { id: number; startX: number; moved: boolean } | null = null;
    const indexFromX = (clientX: number): number => {
        const rect = fieldset.getBoundingClientRect();
        if (rect.width <= 0) return lastIndex < 0 ? 0 : lastIndex;
        const x = Math.min(Math.max(clientX - rect.left, 0), rect.width);
        return Math.min(radios.length - 1, Math.max(0, Math.floor((x / rect.width) * radios.length)));
    };
    const onPointerDown = (e: PointerEvent): void => {
        if (drag || e.pointerType !== "mouse" || e.button !== 0) return;
        if (getComputedStyle(fieldset).flexDirection !== "row") return;
        drag = { id: e.pointerId, startX: e.clientX, moved: false };
        // NOTE: no setPointerCapture here — capturing on down retargets the
        // click to the fieldset and breaks native label activation (taps).
        // Capture starts only once the drag threshold is crossed.
    };
    const ensureThumb = (): HTMLElement => {
        let thumb = fieldset.querySelector(".rtt-dragthumb") as HTMLElement | null;
        if (!thumb) {
            thumb = document.createElement("div");
            thumb.className = "rtt-dragthumb";
            thumb.setAttribute("aria-hidden", "true");
            fieldset.appendChild(thumb);
        }
        return thumb;
    };
    const currentLabelText = (): string => {
        const checked = fieldset.querySelector(
            'input[name="ttl_hours"]:checked',
        ) as HTMLInputElement | null;
        return checked?.closest("label")?.querySelector("span")?.textContent?.trim() ?? "";
    };
    const moveThumb = (clientX: number): void => {
        const thumb = ensureThumb();
        const fieldRect = fieldset.getBoundingClientRect();
        const x = Math.min(Math.max(clientX, fieldRect.left), fieldRect.right);
        thumb.style.left = `${x - fieldRect.left}px`;
    };
    const moveThumbToSelection = (thumb: HTMLElement): void => {
        const checked = fieldset.querySelector(
            'input[name="ttl_hours"]:checked',
        ) as HTMLInputElement | null;
        const label = checked?.closest("label");
        if (!label) return;
        const fieldRect = fieldset.getBoundingClientRect();
        const labelRect = label.getBoundingClientRect();
        thumb.style.left = `${labelRect.left + labelRect.width / 2 - fieldRect.left}px`;
    };
    const onPointerMove = (e: PointerEvent): void => {
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
    const endDrag = (e: PointerEvent): void => {
        if (!drag || e.pointerId !== drag.id) return;
        const wasDrag = drag.moved;
        drag = null;
        fieldset.classList.remove("rtt-dragging");
        const thumb = fieldset.querySelector(".rtt-dragthumb") as HTMLElement | null;
        if (thumb && wasDrag) {
            // Glide onto the final pill before fading out.
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
            fieldset.querySelector(".rtt-dragthumb")?.remove();
            clearTimeout(cleanupTimer);
            valueWrap?.querySelectorAll("[data-exit-clone]").forEach((el) => el.remove());
            valueWrap?.remove();
        },
    };
}

export function rebuildRetention(card: Element, locale: Locale = "en", destroyFn?: () => void) {
    if (destroyFn) destroyFn();
    const fieldset = card.querySelector(".rtt-options") as HTMLElement | null;
    if (fieldset) {
        const allowed = readAllowedTtlHours();
        const def = readDefaultTtlHours();
        if (allowed.length > 0) {
            buildOptions(fieldset, locale, allowed, def, readStoredTtl());
        }
    }
    return enhanceRetention(card, locale);
}
