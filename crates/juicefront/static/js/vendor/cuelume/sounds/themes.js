import { BUBBLE } from "./bubble.js?v=2.2.0";
import { MECH } from "./mech.js?v=2.2.0";
import { PRESS } from "./press.js?v=2.2.0";
import { RECIPES } from "./recipes.js?v=2.2.0";
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
