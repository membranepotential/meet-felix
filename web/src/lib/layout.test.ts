import { describe, expect, it } from 'vitest';
import { fitGrid, tileAspect } from './layout.ts';

const WIDE = { min: 16 / 9, max: 16 / 9 };

describe('fitGrid', () => {
	it('puts two 16:9 tiles side by side on a wide stage', () => {
		const fit = fitGrid(2, 1600, 600, 8, WIDE);
		expect([fit.cols, fit.rows]).toEqual([2, 1]);
		expect(fit.tileWidth).toBeCloseTo(796);
	});

	it('stacks tiles on a tall portrait stage', () => {
		const fit = fitGrid(2, 400, 1000, 8, tileAspect(400, 1000));
		expect([fit.cols, fit.rows]).toEqual([1, 2]);
	});

	it('keeps every tile inside the stage', () => {
		for (const count of [1, 3, 5, 7, 10]) {
			for (const [w, h] of [
				[1280, 720],
				[358, 680]
			] as const) {
				const fit = fitGrid(count, w, h, 8, tileAspect(w, h));
				expect(fit.cols * fit.rows).toBeGreaterThanOrEqual(count);
				expect(fit.cols * fit.tileWidth + (fit.cols - 1) * 8).toBeLessThanOrEqual(w + 1e-9);
				expect(fit.rows * fit.tileHeight + (fit.rows - 1) * 8).toBeLessThanOrEqual(h + 1e-9);
			}
		}
	});

	it('fills the height of a portrait phone with four tiles', () => {
		// A 390 × 844 phone: the stage between header and control bar.
		const [w, h] = [358, 680];
		const fit = fitGrid(4, w, h, 8, tileAspect(w, h));
		expect([fit.cols, fit.rows]).toEqual([2, 2]);
		expect(fit.rows * fit.tileHeight + 8).toBeCloseTo(h);
		expect(fit.cols * fit.tileWidth + 8).toBeCloseTo(w);
	});

	it('lets a single tile fill a portrait phone', () => {
		const fit = fitGrid(1, 358, 680, 8, tileAspect(358, 680));
		expect([fit.tileWidth, fit.tileHeight]).toEqual([358, 680]);
	});

	it('does not stretch a lone tile past 16:9 on a wide stage', () => {
		const fit = fitGrid(1, 1600, 600, 8, tileAspect(1600, 600));
		expect(fit.tileHeight).toBe(600);
		expect(fit.tileWidth / fit.tileHeight).toBeCloseTo(16 / 9);
	});
});

describe('tileAspect', () => {
	it('allows tall tiles in portrait and wide ones in landscape', () => {
		expect(tileAspect(1280, 720)).toEqual({ min: 4 / 3, max: 16 / 9 });
		expect(tileAspect(390, 844)).toEqual({ min: 1 / 2, max: 4 / 3 });
	});
});
