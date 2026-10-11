import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ApiResult } from '../api.ts';
import {
	FAILED_RETRY_MS,
	FALLBACK_ICE,
	IceServers,
	RATE_LIMITED_RETRY_MS,
	REFRESH_MS,
	refreshDelay,
	STUN_ONLY_RETRY_MS
} from './ice.ts';

const turn = (credential: string): RTCIceServer[] => [
	{ urls: ['stun:stun.cloudflare.com:3478'] },
	{ urls: ['turns:turn.cloudflare.com:443?transport=tcp'], username: 'u', credential }
];
const ok = (value: RTCIceServer[]): ApiResult<RTCIceServer[]> => ({ ok: true, value });

describe('refreshDelay', () => {
	it('fetches TURN credentials again before they expire, and sooner after a failure', () => {
		expect(refreshDelay(ok(turn('a')))).toBe(REFRESH_MS);
		expect(REFRESH_MS).toBeLessThan(6 * 60 * 60 * 1000);
		expect(refreshDelay(ok(FALLBACK_ICE))).toBe(STUN_ONLY_RETRY_MS);
		expect(refreshDelay({ ok: false, error: { kind: 'rate-limited' } })).toBe(
			RATE_LIMITED_RETRY_MS
		);
		expect(refreshDelay({ ok: false, error: { kind: 'network' } })).toBe(FAILED_RETRY_MS);
	});
});

describe('IceServers', () => {
	beforeEach(() => {
		vi.useFakeTimers();
		vi.spyOn(console, 'warn').mockImplementation(() => {});
	});
	afterEach(() => {
		vi.useRealTimers();
		vi.restoreAllMocks();
	});

	it('falls back to STUN, then hands fresh servers to the open connections', async () => {
		const answers: ApiResult<RTCIceServer[]>[] = [
			{ ok: false, error: { kind: 'rate-limited' } },
			ok(turn('first')),
			ok(turn('second'))
		];
		const fetch = vi.fn(async () => answers.shift() ?? ok(turn('later')));
		const changes: RTCIceServer[][] = [];
		const ice = new IceServers(fetch, (servers) => changes.push(servers));

		expect(await ice.latest).toEqual(FALLBACK_ICE);
		// The rate limit's window passes; the retry brings TURN.
		await vi.advanceTimersByTimeAsync(RATE_LIMITED_RETRY_MS);
		expect(changes).toEqual([turn('first')]);
		expect(await ice.latest).toEqual(turn('first'));
		// Before the credentials expire, new ones replace them.
		await vi.advanceTimersByTimeAsync(REFRESH_MS);
		expect(await ice.latest).toEqual(turn('second'));
		expect(fetch).toHaveBeenCalledTimes(3);

		ice.dispose();
		await vi.advanceTimersByTimeAsync(REFRESH_MS);
		expect(fetch).toHaveBeenCalledTimes(3);
	});

	it('fetches at once on recovery only after a failure', async () => {
		const answers: ApiResult<RTCIceServer[]>[] = [
			{ ok: false, error: { kind: 'network' } },
			ok(turn('back'))
		];
		const fetch = vi.fn(async () => answers.shift() ?? ok(turn('later')));
		const ice = new IceServers(fetch, () => {});
		await ice.latest;
		ice.recover();
		await vi.advanceTimersByTimeAsync(0);
		expect(await ice.latest).toEqual(turn('back'));
		// The last fetch worked: nothing to recover.
		ice.recover();
		await vi.advanceTimersByTimeAsync(0);
		expect(fetch).toHaveBeenCalledTimes(2);
		ice.dispose();
	});
});
