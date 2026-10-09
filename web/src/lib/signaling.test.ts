import { readFileSync } from 'node:fs';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { initSync } from './wasm/meeting_core.js';
import { backoffDelay, decodeFrame } from './signaling.ts';

beforeAll(() => {
	initSync({ module: readFileSync(new URL('./wasm/meeting_core_bg.wasm', import.meta.url)) });
});

afterEach(() => {
	vi.restoreAllMocks();
});

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

describe('decodeFrame', () => {
	it('passes valid messages through and drops the keepalive answer silently', () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		expect(decodeFrame('{"type":"waiting"}')).toEqual({ type: 'waiting' });
		expect(decodeFrame('{"type":"pong"}')).toBeNull();
		expect(warn).not.toHaveBeenCalled();
	});

	it('logs an invalid frame with at most 200 characters of it and ignores it', () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		const raw = `{"type":"welcome","junk":"${'x'.repeat(500)}"}`;
		expect(decodeFrame(raw)).toBeNull();
		expect(warn).toHaveBeenCalledOnce();
		expect(warn.mock.calls[0]?.at(-1)).toBe(raw.slice(0, 200));
	});
});
