/**
 * Pure maths for the active-speaker display: from audio samples to a level in 0..1, and from a
 * level to the three bars next to a speaker's name.
 */

/** Quieter than this counts as silence (level 0). */
const FLOOR_DB = -55;
/** Louder than this counts as full level (1). */
const CEIL_DB = -15;
/** How far a level may fall per poll (10 polls a second): about 0.6 s from full to silence. */
const DECAY = 0.15;
/** At or above this level a participant counts as speaking (about -45 dBFS). */
const SPEAKING = 0.25;
/** The shortest bar, as a fraction of the full height, so the bars never vanish. */
const MIN_BAR = 0.25;

const clamp01 = (value: number) => Math.min(1, Math.max(0, value));

/** Root mean square of time-domain samples in -1..1. */
export function rms(samples: Float32Array): number {
	if (samples.length === 0) return 0;
	let sum = 0;
	for (const sample of samples) sum += sample * sample;
	return Math.sqrt(sum / samples.length);
}

/** Maps an RMS amplitude to a level in 0..1 on a decibel scale. */
export function levelFromRms(value: number): number {
	if (value <= 0) return 0;
	const db = 20 * Math.log10(value);
	return clamp01((db - FLOOR_DB) / (CEIL_DB - FLOOR_DB));
}

/**
 * The level shown after a poll: it rises at once and falls slowly, so the ring does not flicker
 * between words.
 */
export const smoothLevel = (previous: number, measured: number): number =>
	Math.max(clamp01(measured), previous - DECAY);

export const isSpeaking = (level: number): boolean => level >= SPEAKING;

/**
 * The heights of the three level bars, each a fraction of the full height. The middle bar
 * reacts most, the left one least, so the group reads as a small equaliser.
 */
export function levelBars(level: number): [number, number, number] {
	const l = clamp01(level);
	const bar = (gain: number) => MIN_BAR + (1 - MIN_BAR) * Math.min(1, l * gain);
	return [bar(0.7), bar(1.4), bar(1)];
}
