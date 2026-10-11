/**
 * Keeps the server-rendered retention radios in sync with the effective
 * host config. The radios themselves are the single unified selector used
 * with and without JS; this module only persists the choice and rebuilds
 * options (with data-driven tier dividers) when the host changes.
 */
import type { Locale } from "../i18n";
import { readAllowedTtlHours, readDefaultTtlHours } from "./upload-config";
import { ttlLabel, ttlTier } from "./retention";
import { playSound } from "./sounds";

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

    // The radios intentionally carry no data-cuelume-select: cuelume would
    // tick on its own terms, but retention ticks are played here instead —
    // one per change, spammable like every other sound (the shared burst
    // gate in playSound is the only throttle).
    let lastTicked: string | null = radios.find((r) => r.checked)?.value ?? null;
    const orderOf = (value: string | null): number => radios.findIndex((r) => r.value === value);
    const onChange = (e: Event) => {
        const target = e.target as HTMLInputElement | null;
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
