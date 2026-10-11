import { BUBBLE } from "./bubble.js";
import { MECH } from "./mech.js";
import { PRESS } from "./press.js";
import { RECIPES } from "./recipes.js";
export const THEMES = {
    default: RECIPES,
    mech: MECH,
    bubble: BUBBLE,
    press: PRESS,
};
export const themes = Object.keys(THEMES);
export function isThemeName(value) {
    return typeof value === "string" && Object.prototype.hasOwnProperty.call(THEMES, value);
}
