import { BAR, contact, knock, struck } from "./recipes.js";
export const MECH = {
    tap: {
        masterGain: 0.661,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 2650, filterQ: 1.5, attack: 0.001, decay: 0.004, peak: 0.08 },
            knock(455, 0.012, 0.02),
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 4600, filterQ: 2, attack: 0.001, decay: 0.002, peak: 0.05 },
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 215, filterQ: 2, attack: 0.001, decay: 0.025, peak: 0.3 },
        ],
        vary: { pitch: 0.02, level: 0.1 },
    },
    type: {
        masterGain: 0.421,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 3000, filterQ: 1.2, attack: 0.001, decay: 0.003, peak: 0.1 },
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 2150, filterQ: 5, attack: 0.001, decay: 0.008, peak: 0.18 },
            { kind: "noise", filterType: "bandpass", filterFrequency: 750, filterQ: 3, attack: 0.001, decay: 0.01, peak: 0.22 },
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 330, filterQ: 2.5, attack: 0.001, decay: 0.02, peak: 0.3 },
        ],
        vary: { pitch: 0.05, level: 0.15 },
    },
    select: {
        masterGain: 0.495,
        layers: [
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 3650, filterQ: 3, attack: 0.001, decay: 0.003, peak: 0.2 },
            { kind: "noise", filterType: "bandpass", filterFrequency: 1500, filterQ: 8, attack: 0.001, decay: 0.009, peak: 0.35 },
            { kind: "tone", waveform: "sine", frequency: 1480, attack: 0.001, decay: 0.03, peak: 0.012 },
            knock(250, 0.015, 0.025, { from: "strong" }),
        ],
    },
    toggle: {
        masterGain: 0.294,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 1650, filterQ: 2, attack: 0.001, decay: 0.004, peak: 0.14 },
            knock(330, 0.025, 0.04),
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 2650, filterQ: 3, attack: 0.001, decay: 0.006, peak: 0.18 },
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 250, filterQ: 2, attack: 0.001, decay: 0.03, peak: 0.3 },
        ],
    },
    open: {
        masterGain: 0.854,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 2150, filterQ: 3, attack: 0.001, decay: 0.004, peak: 0.16 },
            { kind: "noise", filterType: "bandpass", filterFrequency: 1160, glideTo: 2150, glideTime: 0.05, filterQ: 2, attack: 0.02, decay: 0.03, peak: 0.08 },
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 1330, filterQ: 5, attack: 0.001, decay: 0.008, peak: 0.14 },
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 290, filterQ: 2, attack: 0.001, decay: 0.03, peak: 0.25 },
        ],
    },
    close: {
        masterGain: 0.627,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 2150, glideTo: 1080, glideTime: 0.04, filterQ: 2, attack: 0.015, decay: 0.025, peak: 0.04 },
            { kind: "noise", filterType: "bandpass", filterFrequency: 1000, filterQ: 4, attack: 0.001, decay: 0.01, peak: 0.22 },
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 3300, filterQ: 2, attack: 0.001, decay: 0.003, peak: 0.1 },
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 180, filterQ: 2, attack: 0.001, decay: 0.035, peak: 0.3 },
        ],
    },
    success: {
        masterGain: 0.597,
        layers: [
            contact(2200, 0.05),
            ...struck(698.46, 0.1, 0.03, BAR),
            ...struck(1046.5, 0.12, 0.026, BAR),
            ...struck(1760, 0.08, 0.018, BAR),
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 4150, filterQ: 2, attack: 0.001, decay: 0.003, peak: 0.06 },
        ],
    },
    error: {
        masterGain: 0.553,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 430, filterQ: 4, attack: 0.001, decay: 0.02, peak: 0.3 },
            ...struck(293.66, 0.06, 0.045, BAR),
            ...struck(698.46, 0.04, 0.02, BAR),
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 1830, filterQ: 3, attack: 0.001, decay: 0.006, peak: 0.1 },
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 150, filterQ: 2, attack: 0.001, decay: 0.04, peak: 0.3 },
        ],
    },
    navigate: {
        masterGain: 0.723,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 750, glideTo: 1400, glideTime: 0.13, filterQ: 1.8, attack: 0.05, decay: 0.096, peak: 0.1 },
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 2500, filterQ: 4, attack: 0.001, decay: 0.003, peak: 0.1 },
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 250, filterQ: 1.5, attack: 0.04, decay: 0.112, peak: 0.12 },
        ],
    },
    warning: {
        masterGain: 0.504,
        layers: [
            contact(2000, 0.05),
            ...struck(587.33, 0.08, 0.04, BAR),
            ...struck(880, 0.08, 0.03, BAR),
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 300, filterQ: 2, attack: 0.001, decay: 0.03, peak: 0.25 },
        ],
    },
    loading: {
        masterGain: 0.212,
        layers: [
            { kind: "noise", filterType: "bandpass", filterFrequency: 500, filterQ: 1, attack: 0.35, decay: 0.7, peak: 0.03 },
            { kind: "tone", waveform: "sine", frequency: 233.08, attack: 0.35, decay: 0.7, peak: 0.015 },
            { from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 1000, filterQ: 2, attack: 0.35, decay: 0.5, peak: 0.005 },
            { from: "strong", kind: "noise", filterType: "lowpass", filterFrequency: 250, filterQ: 0.7, attack: 0.35, decay: 0.7, peak: 0.04 },
        ],
    },
    ready: {
        masterGain: 0.434,
        layers: [
            contact(2000, 0.05),
            ...struck(523.25, 0.16, 0.05, BAR),
            ...struck(261.63, 0.2, 0.03, BAR, { from: "strong" }),
        ],
    },
    attention: {
        masterGain: 0.414,
        layers: [
            contact(2500, 0.04),
            ...struck(880, 0.2, 0.04, BAR),
            ...struck(1318.51, 0.2, 0.03, BAR),
            { from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 300, filterQ: 2, attack: 0.001, decay: 0.03, peak: 0.25 },
        ],
    },
    count: {
        masterGain: 0.72,
        layers: [
            { stretch: true, kind: "noise", filterType: "bandpass", filterFrequency: 500, glideTo: 900, glideTime: 0.6, filterQ: 2, attack: 0.1, decay: 0.6, peak: 0.06 },
            { stretch: true, from: "normal", kind: "noise", filterType: "bandpass", filterFrequency: 1900, glideTo: 3300, glideTime: 0.6, filterQ: 4, attack: 0.1, decay: 0.5, peak: 0.03 },
            { stretch: true, from: "strong", kind: "noise", filterType: "bandpass", filterFrequency: 220, filterQ: 2, attack: 0.1, decay: 0.5, peak: 0.1 },
        ],
        vary: { pitch: 0.03, level: 0.1 },
    },
};
