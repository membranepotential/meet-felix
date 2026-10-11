import { readFileSync } from 'node:fs';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import {
	checkSession,
	createMeeting,
	getIceServers,
	getMeeting,
	getTranscript,
	ICE_TIMEOUT_MS,
	listMeetings,
	login,
	transcriptFile,
	type ApiErrorCode
} from './api.ts';
import { initSync } from './wasm/meeting_core.js';

beforeAll(() => {
	initSync({ module: readFileSync(new URL('./wasm/meeting_core_bg.wasm', import.meta.url)) });
});

afterEach(() => {
	vi.unstubAllGlobals();
});

/** Answers every `fetch` with `body`, `status` and `headers`, and records the requests. */
function serve(status: number, body?: unknown, headers: HeadersInit = {}) {
	const fetch = vi.fn(
		async () => new Response(body === undefined ? null : JSON.stringify(body), { status, headers })
	);
	vi.stubGlobal('fetch', fetch);
	return fetch;
}

/** The Worker's error body for `code`. */
const refusal = (code: ApiErrorCode) => ({ error: code });

describe('errors', () => {
	it('reads the code from the Worker, and takes any other answer as a bare status', async () => {
		serve(429, refusal('rateLimited'));
		expect(await listMeetings()).toEqual({
			ok: false,
			error: { kind: 'rate-limited', retryAfterMs: null }
		});
		serve(429, refusal('rateLimited'), { 'Retry-After': '42' });
		expect(await listMeetings()).toEqual({
			ok: false,
			error: { kind: 'rate-limited', retryAfterMs: 42_000 }
		});
		serve(403, refusal('crossOrigin'));
		expect(await listMeetings()).toEqual({ ok: false, error: { kind: 'unauthorized' } });
		serve(503, refusal('noFreeWords'));
		expect(await createMeeting('fromStart')).toEqual({
			ok: false,
			error: { kind: 'server', status: 503, code: 'noFreeWords' }
		});
		// Not the Worker's body: a Worker without the route, or another server.
		serve(404, 'Not Found');
		expect(await listMeetings()).toEqual({
			ok: false,
			error: { kind: 'server', status: 404, code: null }
		});
		serve(401);
		expect(await listMeetings()).toEqual({
			ok: false,
			error: { kind: 'server', status: 401, code: null }
		});
	});
});

describe('owner session', () => {
	it('logs in with the password as JSON and reports a wrong one', async () => {
		const fetch = serve(200, { name: 'Robin Hart' });
		expect(await login('secret')).toEqual({ ok: true, value: { name: 'Robin Hart' } });
		expect(fetch).toHaveBeenCalledWith('/api/login', {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ password: 'secret' })
		});
		serve(401, refusal('unauthorized'));
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
		serve(401, refusal('unauthorized'));
		expect(await checkSession()).toEqual({ ok: true, value: { kind: 'none' } });
		serve(404);
		expect(await checkSession()).toEqual({
			ok: false,
			error: { kind: 'server', status: 404, code: null }
		});
		// The static app's fallback page answers 200 for a route the Worker does not have.
		serve(200, '<!doctype html>');
		expect(await checkSession()).toEqual({ ok: false, error: { kind: 'malformed' } });
		serve(204);
		expect(await checkSession()).toEqual({ ok: false, error: { kind: 'malformed' } });
		serve(200, { name: '' });
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
		serve(201, { meeting: 'Not A Meeting' });
		expect(await createMeeting('fromStart')).toEqual({ ok: false, error: { kind: 'malformed' } });
	});
});

describe('getMeeting', () => {
	it('reads an open meeting with its policy, an ended one and an unknown one', async () => {
		serve(200, { phase: 'scheduled', recordingPolicy: 'fromStart' });
		expect(await getMeeting('abide-acid-acorn')).toEqual({
			ok: true,
			value: { kind: 'open', recordingPolicy: 'fromStart', phase: 'scheduled' }
		});
		serve(200, { phase: 'ended' });
		expect(await getMeeting('abide-acid-acorn')).toEqual({
			ok: true,
			value: { kind: 'ended' }
		});
		serve(404, refusal('notFound'));
		expect(await getMeeting('abide-acid-acorn')).toEqual({
			ok: true,
			value: { kind: 'not-found' }
		});
	});

	it('reads neither a body without a phase nor a live meeting without its policy', async () => {
		serve(200, { recordingPolicy: 'fromStart', ended: false });
		expect(await getMeeting('abide-acid-acorn')).toEqual({
			ok: false,
			error: { kind: 'malformed' }
		});
		serve(200, { phase: 'live' });
		expect(await getMeeting('abide-acid-acorn')).toEqual({
			ok: false,
			error: { kind: 'malformed' }
		});
	});
});

describe('listMeetings', () => {
	it('lists the meetings in the Worker’s order, with the transcripts it gives', async () => {
		const meetings = [
			{ meeting: 'abide-acid-acorn', createdAt: 3, phase: 'scheduled' },
			{ meeting: 'abide-acid-acorn', createdAt: 2, phase: 'live' },
			{ meeting: 'abide-acid-acorn', createdAt: 1, phase: 'ended', transcript: '1'.repeat(32) }
		];
		const fetch = serve(200, { meetings });
		expect(await listMeetings()).toEqual({ ok: true, value: meetings });
		expect(fetch).toHaveBeenCalledWith('/api/meetings', {});
	});

	it('rejects a list with an entry that is not a meeting', async () => {
		serve(200, { meetings: [{ meeting: 'not a meeting', createdAt: 1, phase: 'ended' }] });
		expect(await listMeetings()).toEqual({ ok: false, error: { kind: 'malformed' } });
		serve(200, { meetings: [{ meeting: 'abide-acid-acorn', createdAt: 1, phase: 'paused' }] });
		expect(await listMeetings()).toEqual({ ok: false, error: { kind: 'malformed' } });
	});
});

describe('getTranscript', () => {
	const TOKEN = '0123456789abcdef0123456789abcdef';
	/** A `transcript.json` as the recorder writes it: `withdrawnAt` only after a withdrawal. */
	const transcript = {
		version: 1,
		meeting: 'amber-otter-ocean',
		startedAt: 1_000,
		endedAt: 3_601_000,
		speakers: [{ who: 0, label: 'Robin (host)', role: 'host' }],
		consents: [{ who: 0, name: 'Robin', at: 800, noticeVersion: 1 }],
		gaps: [{ who: 0, start: 10_000, end: 22_000, reason: 'missing' }],
		paragraphs: [{ who: 0, label: 'Robin (host)', startMs: 1_000, endMs: 4_000, text: 'Hallo.' }]
	};
	const known = { meeting: 'amber-otter-ocean', startedAt: 1_000 };

	it('reads every status the recorder sends', async () => {
		const ready = { status: 'ready', ...known, endedAt: 3_601_000, audioUntil: 5, transcript };
		// The recorder also sends its progress with a ready status; the page needs none.
		const fetch = serve(200, { ...ready, progress: { done: 4, total: 4 } });
		expect(await getTranscript(TOKEN)).toEqual({ ok: true, value: ready });
		expect(fetch).toHaveBeenCalledWith(`/api/transcript/${TOKEN}`, {});
		const progress = { done: 60_000, total: 540_000 };
		for (const status of [
			{ status: 'recording', ...known, progress },
			{ status: 'processing', ...known, endedAt: 2_000, progress },
			{ status: 'processing', ...known, endedAt: 2_000, progress, retryAt: 42 },
			{ status: 'failed', ...known, endedAt: 2_000 }
		]) {
			serve(200, status);
			expect(await getTranscript(TOKEN)).toEqual({ ok: true, value: status });
		}
	});

	it('turns an unknown token into not-found and rejects what the Worker does not send', async () => {
		serve(404, refusal('notFound'));
		expect(await getTranscript(TOKEN)).toEqual({ ok: true, value: { status: 'not-found' } });
		for (const body of [
			{ status: 'done', ...known },
			{ status: 'failed', ...known },
			{ status: 'recording', ...known, progress: { done: '1', total: 2 } },
			{
				status: 'ready',
				...known,
				endedAt: 2,
				audioUntil: 5,
				transcript: { ...transcript, gaps: [{ ...transcript.gaps[0], reason: 'lost' }] }
			}
		]) {
			serve(200, body);
			expect(await getTranscript(TOKEN), JSON.stringify(body)).toEqual({
				ok: false,
				error: { kind: 'malformed' }
			});
		}
	});

	it('links the downloads', () => {
		expect(transcriptFile(TOKEN, 'md')).toBe(`/api/transcript/${TOKEN}/transcript.md`);
	});
});

describe('getIceServers', () => {
	it("asks for the meeting's servers, with the TURN credentials' lifetime", async () => {
		const servers = [{ urls: ['stun:stun.cloudflare.com:3478'] }];
		const fetch = serve(200, { iceServers: servers });
		expect(await getIceServers('abide-acid-acorn')).toEqual({
			ok: true,
			value: { servers, ttlMs: null }
		});
		const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
		expect(url).toBe('/api/ice?meeting=abide-acid-acorn');
		expect(init.signal).toBeInstanceOf(AbortSignal);

		const turn = [{ urls: ['turns:turn.cloudflare.com:443'], username: 'u', credential: 'c' }];
		serve(200, { iceServers: turn, ttl: 21_600 });
		expect(await getIceServers('abide-acid-acorn')).toEqual({
			ok: true,
			value: { servers: turn, ttlMs: 21_600_000 }
		});
		serve(200, { iceServers: turn, ttl: 'soon' });
		expect(await getIceServers('abide-acid-acorn')).toEqual({
			ok: false,
			error: { kind: 'malformed' }
		});
	});

	it('gives up after the timeout', async () => {
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
