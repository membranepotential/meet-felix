import { describe, expect, it } from 'vitest';
import { backoffDelay } from './signaling.ts';

describe('backoffDelay', () => {
	it('doubles from 500 ms up to a 10 s ceiling, with jitter in the upper half', () => {
		const low = (n: number) => backoffDelay(n, () => 0);
		const high = (n: number) => backoffDelay(n, () => 1);
		expect([1, 2, 3, 4].map(high)).toEqual([500, 1000, 2000, 4000]);
		expect([1, 2, 3].map(low)).toEqual([250, 500, 1000]);
		expect(high(20)).toBe(10_000);
		expect(low(20)).toBe(5_000);
	});
});
