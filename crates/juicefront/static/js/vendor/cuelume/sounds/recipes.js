export const LANDED = 2;
const KNOCK_DROP = 1.6;
const KNOCK_DROP_TIME = 0.018;
export const knock = (frequency, decay, peak, more = {}) => ({
    kind: "tone", waveform: "sine", frequency: frequency * KNOCK_DROP, glideTo: frequency, glideTime: KNOCK_DROP_TIME,
    attack: 0.001, decay, peak, ...more,
});
export const BAR = [[1, 1], [2.76, 0.3], [5.4, 0.1]];
export const MALLET = [[1, 1], [3.99, 0.2]];
const MODE_CEILING = 5000;
const STRIKE_ATTACK = 0.003;
const SHORTEST_MODE = 0.015;
export function struck(frequency, decay, peak, material, { from } = {}) {
    return material
        .filter(([ratio]) => frequency * ratio <= MODE_CEILING)
        .map(([ratio, level], mode) => {
        const layerFrom = mode === 0 ? from : (from ?? "normal");
        return {
            kind: "tone", waveform: "sine", frequency: frequency * ratio, attack: STRIKE_ATTACK,
            decay: Math.max(SHORTEST_MODE, decay / ratio), peak: peak * level, ...(layerFrom ? { from: layerFrom } : {}),
        };
    });
}
const FLAT = -3;
export const contact = (hardness, peak, more = {}) => ({
    kind: "noise", filterType: "lowpass", filterFrequency: hardness, filterQ: FLAT, attack: 0.001, decay: 0.003, peak, ...more,
});
export const RECIPES = {
    tap: {
        masterGain: 0.39,
        layers: [
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 4500, filterQ: 1.2, attack: 0.001, decay: 0.002, peak: 0.03 },
            { kind: "tone", waveform: "sine", frequency: 1174.66, attack: 0.001, decay: 0.128, peak: 0.0135 },
            { kind: "tone", waveform: "sine", frequency: 1180, attack: 0.001, decay: 0.104, peak: 0.008 },
            { kind: "tone", waveform: "sine", frequency: 3242, attack: 0.001, decay: 0.048, peak: 0.0055 },
            knock(300, 0.02, 0.02, { from: "normal" }),
            { from: "strong", kind: "tone", waveform: "sine", frequency: 587.33, attack: 0.002, decay: 0.16, peak: 0.009 },
            knock(150, 0.035, 0.014, { from: "strong" }),
        ],
        vary: { pitch: 0.012, level: 0.12 },
    },
    type: {
        masterGain: 0.375,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 3900, filterQ: 0.8, attack: 0.001, decay: 0.005, peak: 0.08 },
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 1550, filterQ: 3.5, attack: 0.001, decay: 0.016, peak: 0.22 },
            { kind: "noise", filterType: "bandpass", filterFrequency: 310, filterQ: 2.5, attack: 0.002, decay: 0.03, peak: 0.35 },
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 140, filterQ: 2, attack: 0.002, decay: 0.045, peak: 0.45 },
        ],
        vary: { pitch: 0.07, level: 0.2 },
    },
    select: {
        masterGain: 0.306,
        layers: [
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 2800, filterQ: 2.2, attack: 0.001, decay: 0.008, peak: 0.16 },
            knock(415, 0.016, 0.03),
            knock(208, 0.025, 0.025, { from: "strong" }),
        ],
    },
    toggle: {
        masterGain: 0.24,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 1830, filterQ: 1.6, attack: 0.001, decay: 0.016, peak: 0.12 },
            knock(330, 0.03, 0.045),
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 3150, filterQ: 1.6, attack: 0.001, decay: 0.012, peak: 0.08 },
            knock(165, 0.03, 0.025, { from: "strong" }),
        ],
    },
    open: {
        masterGain: 0.335,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 600, glideTo: 1500, glideTime: 0.1, filterQ: 1.4, attack: 0.05, decay: 0.06, peak: 0.168 },
            { kind: "tone", waveform: "sine", frequency: 659.25, attack: 0.05, decay: 0.12, peak: 0.015 },
            { from: "normal", kind: "tone", waveform: "sine", frequency: 1318.51, attack: 0.05, decay: 0.05, peak: 0.005 },
            { from: "strong", kind: "noise", filterType: "lowpass", filterFrequency: 250, filterQ: 0.7, attack: 0.04, decay: 0.08, peak: 0.12 },
        ],
    },
    close: {
        masterGain: 0.258,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 1400, glideTo: 540, glideTime: 0.07, filterQ: 1.4, attack: 0.02, decay: 0.06, peak: 0.216 },
            { kind: "tone", waveform: "sine", frequency: 261.63, attack: 0.003, decay: 0.08, peak: 0.02 },
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 350, filterQ: 2, attack: 0.001, decay: 0.03, peak: 0.2 },
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 165, filterQ: 2, attack: 0.001, decay: 0.04, peak: 0.3 },
        ],
    },
    success: {
        masterGain: 0.566,
        layers: [
            contact(1500, 0.04),
            ...struck(523.25, 0.26, 0.022, MALLET),
            ...struck(783.99, 0.3, 0.02, MALLET),
            ...struck(1318.51, 0.24, 0.014, MALLET),
            ...struck(261.63, 0.36, 0.02, MALLET, { from: "strong" }),
        ],
        room: LANDED,
    },
    error: {
        masterGain: 0.501,
        layers: [
            contact(900, 0.05),
            ...struck(293.66, 0.12, 0.04, MALLET),
            ...struck(698.46, 0.08, 0.018, MALLET),
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 250, filterQ: 1.5, attack: 0.001, decay: 0.03, peak: 0.2 },
        ],
    },
    navigate: {
        masterGain: 0.322,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 400, glideTo: 1100, glideTime: 0.21, filterQ: 1.2, attack: 0.12, decay: 0.12, peak: 0.18 },
            { from: "normal", kind: "noise", filterType: "lowpass", filterFrequency: 540, filterQ: 0.7, attack: 0.09, decay: 0.096, peak: 0.045 },
            { from: "strong", kind: "noise", filterType: "lowpass", filterFrequency: 230, filterQ: 0.7, attack: 0.1, decay: 0.144, peak: 0.1 },
        ],
    },
    warning: {
        masterGain: 0.437,
        layers: [
            contact(1200, 0.04),
            ...struck(440, 0.14, 0.038, MALLET),
            ...struck(659.25, 0.12, 0.03, MALLET),
            ...struck(220, 0.2, 0.02, MALLET, { from: "strong" }),
        ],
    },
    loading: {
        masterGain: 0.106,
        layers: [
            { kind: "tone", waveform: "sine", frequency: 196, attack: 0.35, decay: 0.8, peak: 0.03 },
            { kind: "tone", waveform: "sine", frequency: 392, attack: 0.35, decay: 0.6, peak: 0.01 },
            { from: "normal", kind: "noise", filterType: "lowpass", filterFrequency: 500, filterQ: 0.7, attack: 0.35, decay: 0.5, peak: 0.006 },
            { from: "strong", kind: "tone", waveform: "sine", frequency: 98, attack: 0.35, decay: 0.8, peak: 0.02 },
        ],
    },
    ready: {
        masterGain: 0.467,
        layers: [
            contact(1500, 0.02),
            ...struck(392, 0.4, 0.026, BAR),
            { kind: "tone", waveform: "sine", frequency: 393.2, attack: 0.003, decay: 0.32, peak: 0.012 },
            ...struck(196, 0.45, 0.016, BAR, { from: "strong" }),
        ],
        room: LANDED,
    },
    attention: {
        masterGain: 0.551,
        layers: [
            contact(2500, 0.02),
            ...struck(880, 0.45, 0.018, BAR),
            { kind: "tone", waveform: "sine", frequency: 882, attack: 0.003, decay: 0.38, peak: 0.01 },
            ...struck(1318.51, 0.4, 0.014, BAR),
            ...struck(440, 0.45, 0.012, BAR, { from: "strong" }),
        ],
    },
    count: {
        masterGain: 0.336,
        layers: [
            { stretch: true, kind: "noise", filterType: "bandpass", filterFrequency: 700, glideTo: 1400, glideTime: 0.6, filterQ: 1.2, attack: 0.15, decay: 0.55, peak: 0.05 },
            { stretch: true, kind: "tone", waveform: "sine", frequency: 1046.5, attack: 0.15, decay: 0.5, peak: 0.012 },
            { stretch: true, from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 2000, glideTo: 3200, glideTime: 0.6, filterQ: 2, attack: 0.15, decay: 0.5, peak: 0.02 },
            { stretch: true, from: "strong", kind: "tone", waveform: "sine", frequency: 261.63, attack: 0.15, decay: 0.55, peak: 0.015 },
        ],
        vary: { pitch: 0.02, level: 0.12 },
    },
};
export const sounds = Object.keys(RECIPES);
const ALIASES = {
    chime: "success",
    sparkle: "success",
    droplet: "close",
    bloom: "open",
    whisper: "select",
    tick: "select",
    press: "tap",
    release: "tap",
    page: "navigate",
    pulse: "tap",
    scan: "select",
    arrival: "navigate",
};
const own = (object, key) => Object.prototype.hasOwnProperty.call(object, key);
export function resolveSound(value) {
    if (typeof value !== "string")
        return null;
    if (own(RECIPES, value))
        return value;
    return own(ALIASES, value) ? ALIASES[value] : null;
}
