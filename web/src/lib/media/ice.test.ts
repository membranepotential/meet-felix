import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ApiResult, IceConfig } from '../api.ts';
import {
	FAILED_RETRY_MS,
	FALLBACK_ICE,
	IceServers,
	RATE_LIMITED_RETRY_MS,
	refreshDelay,
	STUN_ONLY_RETRY_MS,
	UNKNOWN_TTL_REFRESH_MS
} from './ice.ts';

const HOUR_MS = 60 * 60 * 1000;
const turn = (credential: string): RTCIceServer[] => [
	{ urls: ['stun:stun.cloudflare.com:3478'] },
	{ urls: ['turns:turn.cloudflare.com:443?transport=tcp'], username: 'u', credential }
];
const ok = (servers: RTCIceServer[], ttlMs: number | null = 6 * HOUR_MS): ApiResult<IceConfig> => ({
	ok: true,
	value: { servers, ttlMs }
});
const rateLimited = (retryAfterMs: number | null): ApiResult<IceConfig> => ({
	ok: false,
	error: { kind: 'rate-limited', retryAfterMs }
});

describe('refreshDelay', () => {
	it('fetches TURN credentials an hour before they expire, and sooner after a failure', () => {
		expect(refreshDelay(ok(turn('a')))).toBe(5 * HOUR_MS);
		// Credentials too short-lived for the margin are fetched again after half their life.
		expect(refreshDelay(ok(turn('a'), HOUR_MS))).toBe(HOUR_MS / 2);
		expect(refreshDelay(ok(turn('a'), null))).toBe(UNKNOWN_TTL_REFRESH_MS);
		expect(refreshDelay(ok(FALLBACK_ICE, null))).toBe(STUN_ONLY_RETRY_MS);
		expect(refreshDelay(rateLimited(15_000))).toBe(15_000);
		expect(refreshDelay(rateLimited(null))).toBe(RATE_LIMITED_RETRY_MS);
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
		const answers: ApiResult<IceConfig>[] = [
			rateLimited(20_000),
			ok(turn('first')),
			ok(turn('second'))
		];
		const fetch = vi.fn(async () => answers.shift() ?? ok(turn('later')));
		const changes: RTCIceServer[][] = [];
		const ice = new IceServers(fetch, (servers) => changes.push(servers));

		expect(await ice.latest).toEqual(FALLBACK_ICE);
		// The rate limit's window passes; the retry brings TURN.
		await vi.advanceTimersByTimeAsync(20_000);
		expect(changes).toEqual([turn('first')]);
		expect(await ice.latest).toEqual(turn('first'));
		// Before the credentials expire, new ones replace them.
		await vi.advanceTimersByTimeAsync(5 * HOUR_MS);
		expect(await ice.latest).toEqual(turn('second'));
		expect(fetch).toHaveBeenCalledTimes(3);

		ice.dispose();
		await vi.advanceTimersByTimeAsync(5 * HOUR_MS);
		expect(fetch).toHaveBeenCalledTimes(3);
	});

	it('fetches at once on recovery only after a failure', async () => {
		const answers: ApiResult<IceConfig>[] = [
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

	it('does not fetch on recovery before the rate limit lets it', async () => {
		const answers: ApiResult<IceConfig>[] = [rateLimited(20_000), ok(turn('back'))];
		const fetch = vi.fn(async () => answers.shift() ?? ok(turn('later')));
		const ice = new IceServers(fetch, () => {});
		await ice.latest;
		// Welcomes before the `Retry-After` has passed ask nothing.
		ice.recover();
		await vi.advanceTimersByTimeAsync(10_000);
		ice.recover();
		await vi.advanceTimersByTimeAsync(0);
		expect(fetch).toHaveBeenCalledTimes(1);
		// The scheduled retry fetches once the window is over.
		await vi.advanceTimersByTimeAsync(10_000);
		expect(fetch).toHaveBeenCalledTimes(2);
		expect(await ice.latest).toEqual(turn('back'));
		ice.dispose();
	});
});
