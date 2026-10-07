/**
 * The `press` theme: the same fourteen cues, each one press of a premium
 * switch. A press clicks (a tight ring over a softer body, gone in about 5 ms)
 * over the knock of what it moves. Modelled on the trackpad and keycaps in
 * DawoodUI's Artasaka preview.
 *
 * The interaction cues vary the one press by what it moves: which click
 * (pressing in, or the lighter one letting go), how low and how damped the
 * body is, and a breath of air when something lifts, slides or shuts.
 *
 * Outcomes are not touched, so they do not click: they are tap's warm note on
 * its own, swelling in, with a chord that says what happened. Success is
 * major and rings into a small room, error a low, spread minor over a soft
 * thump, warning a plain fifth between them, attention a high bell, and
 * loading one soft, low note that swells in and fades. Levels match the
 * default palette on the loudest 30 ms.
 *
 * Arranged for emphasis like the other themes: layers marked from "normal"
 * are ornament that subtle leaves out, and each cue has a layer only strong
 * plays.
 */
import { knock, LANDED } from "./recipes.js";
const A2 = 110;
const C3 = 130.81, G3 = 196, A3 = 220;
const C4 = 261.63, E4 = 329.63, G4 = 392, A4 = 440;
const C5 = 523.25, E5 = 659.25, G5 = 783.99, A5 = 880;
/** How far below its ring a click's body sits. */
const CLICK_BODY = 0.675;
/** One resonance of a click: a narrow ring that dies in about 5 ms. */
const ring = (frequency, peak, more = {}) => ({
    kind: "noise", filterType: "bandpass", filterFrequency: frequency, filterQ: 8, attack: 0.001, decay: 0.006, peak, ...more,
});
/**
 * A click as the video's: a tight ring over a softer, wider body below it,
 * so the energy spreads from 1.5 to 5.5 kHz the way a real switch's does.
 */
const click = (frequency, peak, more = {}) => [
    ring(frequency, peak, more),
    ring(frequency * CLICK_BODY, peak * 0.5, { filterQ: 2, ...more }),
];
/** The press: a ring at 4 kHz. */
const press = (peak, more = {}) => click(4000, peak, more);
/** Letting go: higher and quieter than the press. */
const release = (peak, more = {}) => click(4800, peak, more);
/** The video's note starts 9% sharp for its first cycle. */
const STRIKE = 1.09;
/** How long a note takes to swell to its peak: the video's is 13 dB down at 12 ms. */
const SWELL = 0.06;
/**
 * The press's voice: tap's body, and every outcome's. A deep note struck a
 * little sharp and settling within 15 ms, swelling in over SWELL as the
 * video's does (a short note over half its length, so it never swells after it
 * starts to fade). The octave carries it on small speakers; faint third and
 * fourth partials give it the video's edge.
 */
function note(frequency, decay, peak, from) {
    const attack = Math.min(SWELL, decay / 2);
    const partial = (multiple, length, level, layerFrom = from) => ({
        kind: "tone", waveform: "sine", frequency: frequency * multiple * STRIKE, glideTo: frequency * multiple, glideTime: 0.015,
        attack, decay: decay * length, peak: peak * level, ...(layerFrom ? { from: layerFrom } : {}),
    });
    return [partial(1, 1, 1), partial(2, 0.8, 0.4), partial(3, 0.5, 0.05, from ?? "normal"), partial(4, 0.4, 0.08, from ?? "normal")];
}
/** A sub an octave below a note, half its level: only strong reaches down to it. */
const sub = (frequency, decay, peak) => ({
    from: "strong", kind: "tone", waveform: "sine", frequency: frequency / 2, attack: Math.min(SWELL, decay / 2), decay, peak: peak / 2,
});
/** Air moved by the part: filtered noise that sweeps from `from` to `to` Hz. Heard with the click, never after it. */
const air = (from, to, decay, peak, more = {}) => ({
    kind: "noise", filterType: "bandpass", filterFrequency: from, glideTo: to, glideTime: decay, filterQ: 1.5, attack: 0.008, decay, peak, ...more,
});
/** A dull thud under the click: the part hitting its stop. */
const thud = (frequency, decay, peak, more = {}) => ({
    kind: "noise", filterType: "lowpass", filterFrequency: frequency, filterQ: 0.7, attack: 0.001, decay, peak, ...more,
});
const STEADY = { pitch: 0, level: 0.08 };
export const PRESS = {
    /** One press and the pad's deep note under it: the model every other cue varies. */
    tap: {
        masterGain: 0.05,
        layers: [
            ...press(0.245),
            ...note(C3, 0.6, 0.12),
            sub(C3, 0.65, 0.12),
        ],
        vary: STEADY,
    },
    /** The press alone over a short knock: a keycap bottoming out. */
    type: {
        masterGain: 0.452,
        layers: [
            ...press(0.245),
            knock(C4, 0.02, 0.02),
            knock(C3, 0.03, 0.03, { from: "strong" }),
        ],
        vary: { pitch: 0.08, level: 0.15 },
    },
    /** A detent: only the light click of letting go, over a small, high knock. A later option knocks higher, an earlier one lower. */
    select: {
        masterGain: 0.499,
        layers: [...release(0.12), knock(700, 0.015, 0.02), knock(350, 0.025, 0.02, { from: "strong" })],
        vary: { pitch: 0.02, level: 0.1 },
    },
    /** A switch thrown: the press and a firm mid knock, with the snap of the other contact. Switching off knocks upward. */
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
    /** A latch letting go: the light click, a knock, and air drawn upward as the part lifts. */
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
    /** A lid shutting: a lower, duller click, air pushed out and down, and a low knock as it seats. */
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
    /** No click: a C major chord spread wide, C, G and E, swelling in and ringing into a small room. */
    success: {
        masterGain: 0.139,
        layers: [...note(C4, 0.6, 0.072), ...note(G4, 0.6, 0.06), ...note(E5, 0.55, 0.045), sub(C4, 0.65, 0.072)],
        room: LANDED,
        vary: STEADY,
    },
    /** No click: a low A with the C an octave and a third above, minor and spread so its overtones never rub, over a soft thump. A calm refusal. */
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
    /** A swipe: the light click with air sliding past it. Going back, the air falls. */
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
    /** No click: a plain fifth, A and E, between success and error. Heads up, nothing broke. */
    warning: {
        masterGain: 0.114,
        layers: [...note(A3, 0.26, 0.096), ...note(E4, 0.24, 0.072), sub(A3, 0.28, 0.096)],
        vary: STEADY,
    },
    /** No click: one warm, low G that swells in softly and fades within about a second. Work has begun. */
    loading: {
        masterGain: 0.066,
        layers: [
            { kind: "tone", waveform: "sine", frequency: G3, attack: 0.35, decay: 0.8, peak: 0.05 },
            { from: "normal", kind: "tone", waveform: "sine", frequency: G3 * 2, attack: 0.35, decay: 0.6, peak: 0.01 },
            { from: "strong", kind: "tone", waveform: "sine", frequency: G3 / 2, attack: 0.35, decay: 0.8, peak: 0.03 },
        ],
        vary: STEADY,
    },
    /** No click: one long, warm G below success, ringing into a small room. A result is there. */
    ready: {
        masterGain: 0.131,
        layers: [...note(G3, 0.7, 0.108), sub(G3, 0.75, 0.108)],
        room: LANDED,
        vary: STEADY,
    },
    /** No click: A and E over A, high and bell-like, ringing longest. Blocked until the user answers. */
    attention: {
        masterGain: 0.153,
        layers: [...note(A4, 0.9, 0.067), ...note(E5, 0.8, 0.048), ...note(A5, 0.6, 0.024), sub(A4, 0.9, 0.067)],
        vary: STEADY,
    },
    /** A number rolling: a dial spun, air rising with it for as long as the roll (falling counting down). */
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
