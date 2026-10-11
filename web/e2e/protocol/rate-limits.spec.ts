/**
 * The per-client rate limits as `wrangler.toml` deploys them (D6), on a stack of their own: the
 * other stacks raise them out of reach. Local `wrangler dev` keeps a `CF-Connecting-IP` the
 * client sends, so each test plays its own clients; in production Cloudflare's edge sets it.
 */
import { Api, bearer, createMeeting, type Auth } from '../fixtures/api.ts';
import { startStack, type Stack } from '../fixtures/stack.ts';
import { expect, test } from '../fixtures/test.ts';

/** `PUBLIC_LIMIT` and `OWNER_AUTH_LIMIT` in `wrangler.toml`, per 60 seconds. */
const PUBLIC_LIMIT = 60;
const OWNER_AUTH_LIMIT = 10;

/** A well-formed transcript token that names no transcript. */
const UNKNOWN_TOKEN = '0'.repeat(32);

/** A client of its own: a fresh documentation address (RFC 5737) per call. */
let lastClient = 0;
const newClient = (): Auth => ({ 'cf-connecting-ip': `198.51.100.${++lastClient}` });

/**
 * Waits, if needed, until at least `seconds` remain in the current minute. The limits count in
 * windows aligned to the clock, so a burst that starts here stays in one window.
 */
async function startOfWindow(seconds = 15): Promise<void> {
	const left = 60_000 - (Date.now() % 60_000);
	if (left < seconds * 1000) await new Promise((resolve) => setTimeout(resolve, left + 100));
}

test.describe('rate limits', () => {
	let stack: Stack;
	let baseUrl: string;
	let api: Api;
	const post = (path: string, headers: Auth, body: unknown) =>
		fetch(`${baseUrl}${path}`, { method: 'POST', headers, body: JSON.stringify(body) });
	const get = (path: string, headers: Auth) => fetch(`${baseUrl}${path}`, { headers });

	test.beforeAll(async () => {
		test.setTimeout(120_000);
		stack = await startStack('mock', 'rate-limits', 'deployed');
		baseUrl = stack.baseUrl;
		api = new Api(baseUrl);
	});
	test.afterAll(async () => {
		await stack.stop();
	});

	test('password attempts are throttled per client, the right password included', async () => {
		const client = newClient();
		await startOfWindow();
		for (let attempt = 1; attempt <= OWNER_AUTH_LIMIT; attempt++) {
			expect((await post('/api/login', client, { password: 'wrong' })).status, `${attempt}`).toBe(
				401
			);
		}
		const throttled = await post('/api/login', client, { password: stack.ownerPassword });
		expect(throttled.status).toBe(429);
		expect(throttled.headers.get('retry-after')).toBe('60');
		const asBearer = await post('/api/new', { ...client, ...bearer(stack.ownerPassword) }, {});
		expect(asBearer.status, 'the bearer path counts too').toBe(429);
		const other = await post('/api/login', newClient(), { password: stack.ownerPassword });
		expect(other.status, 'another client').toBe(200);
		// The owner's cookie is not a password attempt.
		const cookie = other.headers.getSetCookie()[0]?.split(';')[0] ?? '';
		expect((await get('/api/meetings', { ...client, cookie })).status).toBe(200);
	});

	test('meeting lookups, ICE servers, joins and transcripts share one limit per client', async () => {
		const meeting = await createMeeting(api, {
			...newClient(),
			...bearer(stack.ownerPassword)
		});
		const client = newClient();
		await startOfWindow();
		// One join's worth and a transcript lookup, then meeting lookups up to the limit.
		expect((await get(`/api/meeting/${meeting}`, client)).status).toBe(200);
		expect((await get(`/api/ice?meeting=${meeting}`, client)).status).toBe(200);
		expect(await api.upgradeStatus(meeting, client)).toBe(101);
		expect((await get(`/api/transcript/${UNKNOWN_TOKEN}`, client)).status).toBe(404);
		for (let guess = 5; guess <= PUBLIC_LIMIT; guess++) {
			expect((await get('/api/meeting/no-such-meeting', client)).status, `${guess}`).toBe(404);
		}
		const throttled = await get(`/api/meeting/${meeting}`, client);
		expect(throttled.status).toBe(429);
		expect(throttled.headers.get('retry-after')).toBe('60');
		expect((await get(`/api/ice?meeting=${meeting}`, client)).status).toBe(429);
		expect(await api.upgradeStatus(meeting, client)).toBe(429);
		const transcript = await get(`/api/transcript/${UNKNOWN_TOKEN}`, client);
		expect(transcript.status).toBe(429);
		expect(transcript.headers.get('referrer-policy'), 'still private').toBe('no-referrer');
		expect((await get(`/api/transcript/${UNKNOWN_TOKEN}/transcript.md`, client)).status).toBe(429);
		expect((await get(`/api/meeting/${meeting}`, newClient())).status, 'another client').toBe(200);
	});
});
