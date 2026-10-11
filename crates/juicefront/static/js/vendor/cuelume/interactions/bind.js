import { playInContext } from "../audio/engine.js";
import { resolveSound } from "../sounds/recipes.js";
const HOVER_GAP_MS = 150;
const TYPE_GAP_MS = 40;
const IME_KEY_CODE = 229;
const CLICK_CUES = {
    "data-cuelume-tap": "tap",
    "data-cuelume-select": "select",
    "data-cuelume-toggle": "toggle",
    "data-cuelume-open": "open",
    "data-cuelume-close": "close",
    "data-cuelume-navigate": "navigate",
};
const CLICK_SELECTOR = Object.keys(CLICK_CUES).map((attr) => `[${attr}]`).join();
const boundRoots = new WeakSet();
const handledEvents = new WeakSet();
const lastPlayed = { hover: -Infinity, type: -Infinity };
const lastSelected = new WeakMap();
function throttled(kind, gapMs) {
    const now = performance.now();
    if (now - lastPlayed[kind] < gapMs)
        return true;
    lastPlayed[kind] = now;
    return false;
}
function isMouse(event) {
    return (event.pointerType === "mouse" && window.matchMedia("(hover: hover) and (pointer: fine)").matches);
}
function isNativeControl(element) {
    return element.tagName === "SELECT" || element.tagName === "INPUT";
}
function deletes(event, field) {
    const { selectionStart: start, selectionEnd: end, value } = field;
    if (typeof start !== "number" || typeof end !== "number")
        return true;
    if (start !== end)
        return true;
    return event.key === "Backspace" ? start > 0 : end < value.length;
}
function keyRole(key) {
    if (key === " ")
        return "space";
    if (key === "Enter")
        return "enter";
    return key === "Backspace" || key === "Delete" ? "delete" : "printable";
}
function inputMethod(event) {
    const { detail, pointerType } = event;
    if (detail === 0)
        return "keyboard";
    return pointerType === "mouse" || pointerType === "touch" || pointerType === "pen" ? pointerType : undefined;
}
function moved(group, index) {
    const previous = lastSelected.get(group);
    if (!(index >= 0))
        return undefined;
    lastSelected.set(group, index);
    if (previous === undefined || previous === index)
        return undefined;
    return index > previous ? 1 : -1;
}
function isChosen(option) {
    return option.getAttribute("aria-checked") === "true" || option.getAttribute("aria-selected") === "true";
}
function siblingDirection(option) {
    const group = option.parentElement;
    if (!group)
        return undefined;
    const options = Array.from(group.children).filter((child) => child.hasAttribute("data-cuelume-select"));
    if (!lastSelected.has(group)) {
        const chosen = options.findIndex(isChosen);
        if (chosen >= 0)
            lastSelected.set(group, chosen);
    }
    return moved(group, options.indexOf(option));
}
function switched(element) {
    const { type, checked } = element;
    if (type === "checkbox" || type === "radio")
        return checked ? 1 : -1;
    const state = element.getAttribute("aria-checked") ?? element.getAttribute("aria-pressed");
    if (state === "true")
        return -1;
    return state === "false" ? 1 : undefined;
}
function isTypingKey(event, field) {
    if (event.isComposing || event.keyCode === IME_KEY_CODE)
        return false;
    if (field.type === "password")
        return false;
    if (event.key === "Backspace" || event.key === "Delete")
        return deletes(event, field);
    if (event.repeat)
        return false;
    if (event.metaKey || (event.ctrlKey && !event.altKey))
        return false;
    if (event.key === "Enter")
        return field.tagName !== "INPUT";
    return event.key.length === 1;
}
function match(root, event, attr, fallback) {
    if (!(event.target instanceof Element))
        return null;
    const element = event.target.closest(`[${attr}]`);
    return element && root.contains(element) ? [element, attr, fallback] : null;
}
function listen(root, eventName, find) {
    root.addEventListener(eventName, (event) => {
        if (handledEvents.has(event))
            return;
        const found = find(event);
        if (!found)
            return;
        handledEvents.add(event);
        const [element, attr, fallback, context = {}] = found;
        const emphasis = element.closest("[data-cuelume-emphasis]")?.getAttribute("data-cuelume-emphasis");
        const theme = element.closest("[data-cuelume-theme]")?.getAttribute("data-cuelume-theme");
        playInContext(resolveSound(element.getAttribute(attr)) ?? fallback, { emphasis: emphasis, theme: theme }, context);
    }, true);
}
export function bind(root) {
    if (typeof document === "undefined")
        return;
    const scope = root ?? document;
    if (boundRoots.has(scope))
        return;
    boundRoots.add(scope);
    listen(scope, "click", (event) => {
        if (!(event.target instanceof Element))
            return null;
        const element = event.target.closest(CLICK_SELECTOR);
        if (!element || !scope.contains(element))
            return null;
        const control = element.tagName === "LABEL" ? element.control : null;
        const inside = control && element.contains(control) ? control : null;
        if (inside && event.target !== inside)
            return null;
        const attr = Object.keys(CLICK_CUES).find((name) => element.hasAttribute(name));
        const selecting = attr === "data-cuelume-select";
        if (selecting && isNativeControl(element))
            return null;
        const direction = selecting ? siblingDirection(element) : attr === "data-cuelume-toggle" ? switched(inside ?? element) : undefined;
        return [element, attr, CLICK_CUES[attr], { input: inputMethod(event), direction }];
    });
    listen(scope, "change", (event) => {
        const found = match(scope, event, "data-cuelume-select", "select");
        if (!found || !isNativeControl(found[0]))
            return null;
        const [element, attr, fallback] = found;
        return [element, attr, fallback, { direction: moved(element, element.selectedIndex) }];
    });
    listen(scope, "keydown", (event) => {
        const found = match(scope, event, "data-cuelume-type", "type");
        if (!found || found[0] !== event.target)
            return null;
        const key = event;
        if (!isTypingKey(key, found[0]) || throttled("type", TYPE_GAP_MS))
            return null;
        const [element, attr, fallback] = found;
        return [element, attr, fallback, { key: keyRole(key.key) }];
    });
    listen(scope, "pointerenter", (event) => {
        const found = match(scope, event, "data-cuelume-hover", "select");
        if (!found || !isMouse(event))
            return null;
        const relatedTarget = event.relatedTarget;
        if (relatedTarget instanceof Node && found[0].contains(relatedTarget))
            return null;
        return throttled("hover", HOVER_GAP_MS) ? null : found;
    });
    listen(scope, "pointerdown", (event) => {
        const found = match(scope, event, "data-cuelume-press", "tap");
        return found && [found[0], found[1], found[2], { input: inputMethod(event) }];
    });
    listen(scope, "pointerup", (event) => {
        const found = match(scope, event, "data-cuelume-release", "tap");
        return found && !found[0].hasAttribute("data-cuelume-press") ? found : null;
    });
}
