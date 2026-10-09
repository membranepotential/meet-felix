import { describe, expect, it } from 'vitest';
import { isSpeaking, levelBars, levelFromRms, rms, smoothLevel } from './speaking.ts';

describe('levelFromRms', () => {
	it('maps silence to 0 and a full-scale signal to 1', () => {
		expect(levelFromRms(0)).toBe(0);
		expect(levelFromRms(1e-4)).toBe(0);
		expect(levelFromRms(rms(new Float32Array([1, -1, 1, -1])))).toBe(1);
	});

	it('counts normal speech as speaking and room noise as not', () => {
		expect(isSpeaking(levelFromRms(0.05))).toBe(true); // about -26 dBFS
		expect(isSpeaking(levelFromRms(0.002))).toBe(false); // about -54 dBFS
	});
});

describe('smoothLevel', () => {
	it('rises at once and falls step by step', () => {
		expect(smoothLevel(0, 0.9)).toBe(0.9);
		expect(smoothLevel(0.9, 0)).toBeCloseTo(0.75);
		expect(smoothLevel(0.1, 0)).toBe(0);
	});
});

describe('levelBars', () => {
	it('keeps short bars at silence', () => {
		expect(levelBars(0)).toEqual([0.25, 0.25, 0.25]);
	});

	it('makes the middle bar the tallest and full at a loud level', () => {
		const [left, middle, right] = levelBars(0.8);
		expect(middle).toBe(1);
		expect(left).toBeLessThan(right);
		expect(right).toBeLessThan(middle);
	});

	it('never shrinks a bar as the level rises', () => {
		const levels = [0, 0.2, 0.4, 0.6, 0.8, 1];
		for (let i = 1; i < levels.length; i++) {
			const before = levelBars(levels[i - 1]!);
			const after = levelBars(levels[i]!);
			after.forEach((height, bar) => expect(height).toBeGreaterThanOrEqual(before[bar]!));
		}
	});

	it('clamps levels outside 0..1', () => {
		expect(levelBars(2)).toEqual(levelBars(1));
		expect(levelBars(-1)).toEqual(levelBars(0));
	});
});
