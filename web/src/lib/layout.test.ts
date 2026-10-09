import { describe, expect, it } from 'vitest';
import { fitGrid, tileAspect } from './layout.ts';

describe('fitGrid', () => {
	it('puts two 16:9 tiles side by side on a wide stage', () => {
		const fit = fitGrid(2, 1600, 600, 8, 16 / 9);
		expect([fit.cols, fit.rows]).toEqual([2, 1]);
		expect(fit.tileWidth).toBeCloseTo(796);
	});

	it('stacks tiles on a tall portrait stage', () => {
		const fit = fitGrid(2, 400, 1000, 8, 3 / 4);
		expect([fit.cols, fit.rows]).toEqual([1, 2]);
	});

	it('keeps every tile inside the stage', () => {
		for (const count of [1, 3, 5, 7, 10]) {
			const fit = fitGrid(count, 1280, 720, 8, 16 / 9);
			expect(fit.cols * fit.rows).toBeGreaterThanOrEqual(count);
			expect(fit.cols * fit.tileWidth + (fit.cols - 1) * 8).toBeLessThanOrEqual(1280 + 1e-9);
			expect(fit.rows * fit.tileHeight + (fit.rows - 1) * 8).toBeLessThanOrEqual(720 + 1e-9);
		}
	});
});

describe('tileAspect', () => {
	it('uses 16:9 in landscape and 3:4 in portrait', () => {
		expect(tileAspect(1280, 720)).toBeCloseTo(16 / 9);
		expect(tileAspect(390, 844)).toBeCloseTo(3 / 4);
	});
});
