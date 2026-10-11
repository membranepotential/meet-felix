import { readFileSync } from 'node:fs';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import {
	checkSession,
	createMeeting,
	getIceServers,
	getMeeting,
	ICE_TIMEOUT_MS,
	listMeetings,
	login
} from './api.ts';
import { initSync } from './wasm/meeting_core.js';

beforeAll(() => {
	initSync({ module: readFileSync(new URL('./wasm/meeting_core_bg.wasm', import.meta.url)) });
});

afterEach(() => {
	vi.unstubAllGlobals();
});

/** Answers every `fetch` with `body` and `status`, and records the requests. */
function serve(status: number, body?: unknown) {
	const fetch = vi.fn(
		async () => new Response(body === undefined ? null : JSON.stringify(body), { status })
	);
	vi.stubGlobal('fetch', fetch);
	return fetch;
}

describe('owner session', () => {
	it('logs in with the password as JSON and reports a wrong one', async () => {
		const fetch = serve(200, { name: 'Robin Hart' });
		expect(await login('secret')).toEqual({ ok: true, value: { name: 'Robin Hart' } });
		expect(fetch).toHaveBeenCalledWith('/api/login', {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ password: 'secret' })
		});
		serve(401);
		expect(await login('wrong')).toEqual({ ok: false, error: { kind: 'unauthorized' } });
	});

	it('tells a valid session from none, and a missing route from both', async () => {
		serve(200, { name: 'Robin Hart' });
		expect(await checkSession()).toEqual({
			ok: true,
			value: { kind: 'owner', owner: { name: 'Robin Hart' } }
		});
		// A deployment without a valid `OWNER_NAME`.
		serve(200, { name: null });
		expect(await checkSession()).toEqual({
			ok: true,
			value: { kind: 'owner', owner: { name: null } }
		});
		serve(401);
		expect(await checkSession()).toEqual({ ok: true, value: { kind: 'none' } });
		serve(404);
		expect(await checkSession()).toEqual({ ok: false, error: { kind: 'server', status: 404 } });
		// The static app's fallback page answers 200 for a route the Worker does not have.
		serve(200, '<!doctype html>');
		expect(await checkSession()).toEqual({ ok: false, error: { kind: 'malformed' } });
		serve(204);
		expect(await checkSession()).toEqual({ ok: false, error: { kind: 'malformed' } });
	});

	it('creates a meeting with the cookie alone and reads its words', async () => {
		const fetch = serve(201, { meeting: 'abide-acid-acorn' });
		expect(await createMeeting('fromStart')).toEqual({ ok: true, value: 'abide-acid-acorn' });
		expect(fetch).toHaveBeenCalledWith('/api/new', {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ recording: 'fromStart' })
		});
	});
});

describe('getMeeting', () => {
	it('reads the phase, and an ended meeting with or without a transcript', async () => {
		serve(200, { recordingPolicy: 'fromStart', phase: 'scheduled', ended: false });
		expect(await getMeeting('abide-acid-acorn')).toEqual({
			ok: true,
			value: { kind: 'open', recordingPolicy: 'fromStart', phase: 'scheduled' }
		});
		serve(200, { recordingPolicy: 'fromStart', phase: 'ended', ended: true, transcript: 't1' });
		expect(await getMeeting('abide-acid-acorn')).toEqual({
			ok: true,
			value: { kind: 'ended', transcript: 't1' }
		});
		serve(200, { recordingPolicy: 'hostDecides', phase: 'ended', ended: true });
		expect(await getMeeting('abide-acid-acorn')).toEqual({
			ok: true,
			value: { kind: 'ended', transcript: null }
		});
		serve(404, {});
		expect(await getMeeting('abide-acid-acorn')).toEqual({
			ok: true,
			value: { kind: 'not-found' }
		});
	});
});

describe('listMeetings', () => {
	it('lists newest first', async () => {
		const fetch = serve(200, {
			meetings: [
				{
					meeting: 'abide-acid-acorn',
					createdAt: 1,
					phase: 'ended',
					ended: true,
					transcript: 't1'
				},
				{ meeting: 'abide-acid-acorn', createdAt: 3, phase: 'scheduled', ended: false },
				{ meeting: 'abide-acid-acorn', createdAt: 2, phase: 'live', ended: false }
			]
		});
		const result = await listMeetings();
		expect(fetch).toHaveBeenCalledWith('/api/meetings', {});
		expect(result).toEqual({
			ok: true,
			value: [
				{ meeting: 'abide-acid-acorn', createdAt: 3, phase: 'scheduled', transcript: null },
				{ meeting: 'abide-acid-acorn', createdAt: 2, phase: 'live', transcript: null },
				{ meeting: 'abide-acid-acorn', createdAt: 1, phase: 'ended', transcript: 't1' }
			]
		});
	});

	it('falls back to `ended` from a Worker without phases', async () => {
		serve(200, {
			meetings: [
				{ meeting: 'abide-acid-acorn', createdAt: 2, ended: false },
				{ meeting: 'abide-acid-acorn', createdAt: 1, ended: true }
			]
		});
		const result = await listMeetings();
		expect(result.ok && result.value.map((m) => m.phase)).toEqual(['live', 'ended']);
	});

	it('reports a missing session and a server without the endpoint', async () => {
		serve(401, {});
		expect(await listMeetings()).toEqual({ ok: false, error: { kind: 'unauthorized' } });
		serve(404, {});
		expect(await listMeetings()).toEqual({
			ok: false,
			error: { kind: 'server', status: 404 }
		});
	});

	it('rejects a list with an entry that is not a meeting', async () => {
		serve(200, { meetings: [{ meeting: 'not a meeting', createdAt: 1, ended: true }] });
		expect(await listMeetings()).toEqual({ ok: false, error: { kind: 'malformed' } });
		serve(200, { meetings: [{ meeting: 'abide-acid-acorn', createdAt: 1, phase: 'paused' }] });
		expect(await listMeetings()).toEqual({ ok: false, error: { kind: 'malformed' } });
	});
});

describe('getIceServers', () => {
	it("asks for the meeting's servers and gives up after the timeout", async () => {
		const servers = [{ urls: ['stun:stun.cloudflare.com:3478'] }];
		const fetch = serve(200, { iceServers: servers });
		expect(await getIceServers('abide-acid-acorn')).toEqual({ ok: true, value: servers });
		const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
		expect(url).toBe('/api/ice?meeting=abide-acid-acorn');
		expect(init.signal).toBeInstanceOf(AbortSignal);

		vi.useFakeTimers();
		vi.stubGlobal(
			'fetch',
			vi.fn(
				(_url: string, init: RequestInit) =>
					new Promise((_, reject) =>
						init.signal?.addEventListener('abort', () => reject(init.signal?.reason))
					)
			)
		);
		const stalled = getIceServers('abide-acid-acorn');
		await vi.advanceTimersByTimeAsync(ICE_TIMEOUT_MS);
		expect(await stalled).toEqual({ ok: false, error: { kind: 'network' } });
		vi.useRealTimers();
	});
});
