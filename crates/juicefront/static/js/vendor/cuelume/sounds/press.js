import { knock, LANDED } from "./recipes.js";
const A2 = 110;
const C3 = 130.81, G3 = 196, A3 = 220;
const C4 = 261.63, E4 = 329.63, G4 = 392, A4 = 440;
const C5 = 523.25, E5 = 659.25, G5 = 783.99, A5 = 880;
const CLICK_BODY = 0.675;
const ring = (frequency, peak, more = {}) => ({
    kind: "noise", filterType: "bandpass", filterFrequency: frequency, filterQ: 8, attack: 0.001, decay: 0.006, peak, ...more,
});
const click = (frequency, peak, more = {}) => [
    ring(frequency, peak, more),
    ring(frequency * CLICK_BODY, peak * 0.5, { filterQ: 2, ...more }),
];
const press = (peak, more = {}) => click(4000, peak, more);
const release = (peak, more = {}) => click(4800, peak, more);
const STRIKE = 1.09;
const SWELL = 0.06;
function note(frequency, decay, peak, from) {
    const attack = Math.min(SWELL, decay / 2);
    const partial = (multiple, length, level, layerFrom = from) => ({
        kind: "tone", waveform: "sine", frequency: frequency * multiple * STRIKE, glideTo: frequency * multiple, glideTime: 0.015,
        attack, decay: decay * length, peak: peak * level, ...(layerFrom ? { from: layerFrom } : {}),
    });
    return [partial(1, 1, 1), partial(2, 0.8, 0.4), partial(3, 0.5, 0.05, from ?? "normal"), partial(4, 0.4, 0.08, from ?? "normal")];
}
const sub = (frequency, decay, peak) => ({
    from: "strong", kind: "tone", waveform: "sine", frequency: frequency / 2, attack: Math.min(SWELL, decay / 2), decay, peak: peak / 2,
});
const air = (from, to, decay, peak, more = {}) => ({
    kind: "noise", filterType: "bandpass", filterFrequency: from, glideTo: to, glideTime: decay, filterQ: 1.5, attack: 0.008, decay, peak, ...more,
});
const thud = (frequency, decay, peak, more = {}) => ({
    kind: "noise", filterType: "lowpass", filterFrequency: frequency, filterQ: 0.7, attack: 0.001, decay, peak, ...more,
});
const STEADY = { pitch: 0, level: 0.08 };
export const PRESS = {
    tap: {
        masterGain: 0.05,
        layers: [
            ...press(0.245),
            ...note(C3, 0.6, 0.12),
            sub(C3, 0.65, 0.12),
        ],
        vary: STEADY,
    },
    type: {
        masterGain: 0.452,
        layers: [
            ...press(0.245),
            knock(C4, 0.02, 0.02),
            knock(C3, 0.03, 0.03, { from: "strong" }),
        ],
        vary: { pitch: 0.08, level: 0.15 },
    },
    select: {
        masterGain: 0.499,
        layers: [...release(0.12), knock(700, 0.015, 0.02), knock(350, 0.025, 0.02, { from: "strong" })],
        vary: { pitch: 0.02, level: 0.1 },
    },
    toggle: {
        masterGain: 0.201,
        layers: [
            ...press(0.245),
            knock(330, 0.035, 0.05),
            ring(4800, 0.1, { from: "normal" }),
            knock(165, 0.045, 0.05, { from: "strong" }),
        ],
        vary: STEADY,
    },
    open: {
        masterGain: 0.525,
        layers: [
            ...release(0.15),
            knock(440, 0.03, 0.03),
            air(1600, 3200, 0.035, 0.1),
            thud(260, 0.04, 0.12, { from: "strong" }),
        ],
        vary: STEADY,
    },
    close: {
        masterGain: 0.281,
        layers: [
            ...click(3200, 0.204),
            knock(C3, 0.04, 0.06),
            air(1800, 700, 0.04, 0.1, { from: "normal" }),
            thud(180, 0.04, 0.2, { from: "strong" }),
        ],
        vary: STEADY,
    },
    success: {
        masterGain: 0.139,
        layers: [...note(C4, 0.6, 0.072), ...note(G4, 0.6, 0.06), ...note(E5, 0.55, 0.045), sub(C4, 0.65, 0.072)],
        room: LANDED,
        vary: STEADY,
    },
    error: {
        masterGain: 0.109,
        layers: [
            ...note(A2, 0.24, 0.12),
            ...note(C5, 0.2, 0.04),
            { kind: "noise", filterType: "lowpass", filterFrequency: 200, filterQ: 0.7, attack: 0.01, decay: 0.06, peak: 0.2 },
            sub(A2 * 2, 0.26, 0.12),
        ],
        vary: STEADY,
    },
    navigate: {
        masterGain: 0.75,
        layers: [
            ...release(0.15),
            air(600, 1400, 0.09, 0.12, { filterQ: 1 }),
            knock(587.33, 0.015, 0.015, { from: "normal" }),
            thud(300, 0.05, 0.12, { from: "strong" }),
        ],
        vary: STEADY,
    },
    warning: {
        masterGain: 0.114,
        layers: [...note(A3, 0.26, 0.096), ...note(E4, 0.24, 0.072), sub(A3, 0.28, 0.096)],
        vary: STEADY,
    },
    loading: {
        masterGain: 0.066,
        layers: [
            { kind: "tone", waveform: "sine", frequency: G3, attack: 0.35, decay: 0.8, peak: 0.05 },
            { from: "normal", kind: "tone", waveform: "sine", frequency: G3 * 2, attack: 0.35, decay: 0.6, peak: 0.01 },
            { from: "strong", kind: "tone", waveform: "sine", frequency: G3 / 2, attack: 0.35, decay: 0.8, peak: 0.03 },
        ],
        vary: STEADY,
    },
    ready: {
        masterGain: 0.131,
        layers: [...note(G3, 0.7, 0.108), sub(G3, 0.75, 0.108)],
        room: LANDED,
        vary: STEADY,
    },
    attention: {
        masterGain: 0.153,
        layers: [...note(A4, 0.9, 0.067), ...note(E5, 0.8, 0.048), ...note(A5, 0.6, 0.024), sub(A4, 0.9, 0.067)],
        vary: STEADY,
    },
    count: {
        masterGain: 0.366,
        layers: [
            { stretch: true, kind: "noise", filterType: "bandpass", filterFrequency: 900, glideTo: 1800, glideTime: 0.6, filterQ: 2, attack: 0.15, decay: 0.55, peak: 0.08 },
            { stretch: true, kind: "noise", filterType: "lowpass", filterFrequency: 200, glideTo: 500, glideTime: 0.6, filterQ: 0.7, attack: 0.15, decay: 0.55, peak: 0.06 },
            { stretch: true, from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 2600, glideTo: 3600, glideTime: 0.6, filterQ: 4, attack: 0.15, decay: 0.45, peak: 0.02 },
            { stretch: true, from: "strong", kind: "noise", filterType: "lowpass", filterFrequency: 150, glideTo: 300, glideTime: 0.6, filterQ: 0.7, attack: 0.15, decay: 0.55, peak: 0.08 },
        ],
        vary: { pitch: 0.02, level: 0.1 },
    },
};
