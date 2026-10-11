import { knock, LANDED } from "./recipes.js";
const slide = (from, to, time, decay, peak, more = {}) => ({
    kind: "tone", waveform: "sine", frequency: from, glideTo: to, glideTime: time, attack: 0.002, decay, peak, ...more,
});
const bloop = (frequency, decay, peak, more = {}) => slide(frequency, frequency * 1.5, 0.04, decay, peak, more);
const drip = (frequency, decay, peak, more = {}) => slide(frequency, frequency * 1.5, 0.012, decay, peak, { attack: 0.001, ...more });
const pluck = (frequency, decay, peak, more = {}) => slide(frequency * 1.3, frequency, 0.012, decay, peak, { attack: 0.001, ...more });
const pop = (filterFrequency, peak, more = {}) => ({
    kind: "noise", filterType: "lowpass", filterFrequency, filterQ: 0.9, attack: 0.001, decay: 0.003, peak, ...more,
});
const LIVELY = { pitch: 0.06, level: 0.1 };
const STEADY = { pitch: 0, level: 0.08 };
export const BUBBLE = {
    tap: {
        masterGain: 0.349,
        layers: [
            knock(300, 0.02, 0.06),
            bloop(600, 0.035, 0.014, { attack: 0.012 }),
            knock(600, 0.012, 0.015, { from: "normal" }),
            knock(150, 0.035, 0.05, { from: "strong" }),
        ],
        vary: { pitch: 0.08, level: 0.1 },
    },
    type: {
        masterGain: 0.281,
        layers: [
            knock(415, 0.014, 0.06),
            pop(2500, 0.03, { from: "normal" }),
            knock(208, 0.025, 0.05, { from: "strong" }),
        ],
        vary: { pitch: 0.1, level: 0.2 },
    },
    select: {
        masterGain: 0.278,
        layers: [
            drip(700, 0.025, 0.05),
            pop(3000, 0.075, { from: "normal" }),
            bloop(350, 0.05, 0.03, { from: "strong" }),
        ],
        vary: { pitch: 0.03, level: 0.1 },
    },
    toggle: {
        masterGain: 0.153,
        layers: [
            { kind: "noise", filterType: "lowpass", filterFrequency: 900, filterQ: 0.9, attack: 0.001, decay: 0.008, peak: 0.12 },
            slide(330, 660, 0.06, 0.07, 0.045, { attack: 0.003 }),
            slide(990, 1485, 0.02, 0.03, 0.01, { from: "normal", attack: 0.03 }),
            slide(165, 330, 0.06, 0.08, 0.03, { from: "strong" }),
        ],
        vary: { pitch: 0.04, level: 0.1 },
    },
    open: {
        masterGain: 0.211,
        layers: [
            slide(220, 520, 0.14, 0.06, 0.04, { attack: 0.1 }),
            slide(330, 780, 0.14, 0.06, 0.012, { from: "normal", attack: 0.1 }),
            slide(110, 260, 0.14, 0.08, 0.03, { from: "strong", attack: 0.1 }),
        ],
        vary: LIVELY,
    },
    close: {
        masterGain: 0.28,
        layers: [
            slide(700, 220, 0.07, 0.08, 0.045, { attack: 0.004 }),
            pop(1200, 0.06, { from: "normal" }),
            slide(350, 110, 0.07, 0.1, 0.03, { from: "strong", attack: 0.004 }),
        ],
        vary: LIVELY,
    },
    success: {
        masterGain: 0.48,
        layers: [
            slide(523.25, 783.99, 0.08, 0.28, 0.04, { attack: 0.003 }),
            slide(1046.5, 1567.98, 0.08, 0.08, 0.01, { from: "normal", attack: 0.003 }),
            slide(261.63, 392, 0.08, 0.3, 0.02, { from: "strong", attack: 0.003 }),
        ],
        room: LANDED,
        vary: STEADY,
    },
    error: {
        masterGain: 0.42,
        layers: [
            slide(349.23, 261.63, 0.14, 0.2, 0.045, { attack: 0.004 }),
            pop(900, 0.12, { from: "normal" }),
            knock(147, 0.05, 0.05, { from: "strong" }),
        ],
        vary: STEADY,
    },
    navigate: {
        masterGain: 0.406,
        layers: [
            slide(300, 1500, 0.09, 0.08, 0.035, { attack: 0.01 }),
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 1500, glideTo: 3000, glideTime: 0.09, filterQ: 1, attack: 0.02, decay: 0.07, peak: 0.05 },
            slide(150, 600, 0.1, 0.1, 0.025, { from: "strong", attack: 0.01 }),
        ],
        vary: LIVELY,
    },
    warning: {
        masterGain: 0.468,
        layers: [
            slide(500, 440, 0.05, 0.16, 0.045),
            pop(1800, 0.09, { from: "normal" }),
            { from: "strong", kind: "tone", waveform: "sine", frequency: 220, attack: 0.003, decay: 0.18, peak: 0.025 },
        ],
        vary: STEADY,
    },
    loading: {
        masterGain: 0.191,
        layers: [
            { kind: "noise", filterType: "lowpass", filterFrequency: 900, filterQ: 0.7, attack: 0.35, decay: 0.6, peak: 0.03 },
            slide(400, 600, 0.35, 0.7, 0.016, { attack: 0.35 }),
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 1800, filterQ: 1.5, attack: 0.35, decay: 0.4, peak: 0.004 },
            { from: "strong", kind: "noise", filterType: "lowpass", filterFrequency: 400, filterQ: 0.7, attack: 0.35, decay: 0.7, peak: 0.04 },
        ],
        vary: STEADY,
    },
    ready: {
        masterGain: 0.477,
        layers: [
            slide(1400, 700, 0.012, 0.02, 0.03, { attack: 0.001 }),
            slide(392, 587.33, 0.06, 0.25, 0.04, { attack: 0.012 }),
            pop(2000, 0.03, { from: "normal" }),
            slide(196, 294, 0.08, 0.3, 0.02, { from: "strong", attack: 0.012 }),
        ],
        room: LANDED,
        vary: STEADY,
    },
    attention: {
        masterGain: 0.561,
        layers: [
            slide(659.25, 880, 0.14, 0.25, 0.035, { attack: 0.003 }),
            pop(2200, 0.12, { from: "normal" }),
            bloop(329.63, 0.2, 0.02, { from: "strong" }),
        ],
        vary: STEADY,
    },
    count: {
        masterGain: 0.238,
        layers: [
            { stretch: true, kind: "noise", filterType: "lowpass", filterFrequency: 900, glideTo: 1800, glideTime: 0.6, filterQ: 0.7, attack: 0.15, decay: 0.55, peak: 0.04 },
            { ...slide(587.33, 880, 0.6, 0.55, 0.02, { attack: 0.15 }), stretch: true },
            pop(1600, 0.05, { from: "normal" }),
            { ...slide(293.66, 440, 0.6, 0.55, 0.02, { attack: 0.15, from: "strong" }), stretch: true },
        ],
        vary: LIVELY,
    },
};
