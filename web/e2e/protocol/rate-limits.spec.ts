/**
 * The per-client rate limits as `wrangler.toml` deploys them (D6), on a stack of their own: the
 * other stacks raise them out of reach. Local `wrangler dev` keeps a `CF-Connecting-IP` the
 * client sends, so each test plays its own clients; in production Cloudflare's edge sets it.
 */
import { randomBytes } from 'node:crypto';
import { request } from 'node:http';
import { Api, bearer, createMeeting, type Auth } from '../fixtures/api.ts';
import { startStack, type Stack } from '../fixtures/stack.ts';
import { expect, test } from '../fixtures/test.ts';

/** `PUBLIC_LIMIT` and `OWNER_AUTH_LIMIT` in `wrangler.toml`, per 60 seconds. */
const PUBLIC_LIMIT = 60;
const OWNER_AUTH_LIMIT = 10;

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

/** The status of a WebSocket upgrade to `/ws/{meeting}` with `headers`: 101 if accepted. */
function upgradeStatus(baseUrl: string, meeting: string, headers: Auth): Promise<number> {
	return new Promise((resolve, reject) => {
		const req = request(`${baseUrl}/ws/${meeting}`, {
			headers: {
				...headers,
				connection: 'Upgrade',
				upgrade: 'websocket',
				'sec-websocket-version': '13',
				'sec-websocket-key': randomBytes(16).toString('base64')
			}
		});
		req.on('upgrade', (_response, socket) => {
			socket.destroy();
			resolve(101);
		});
		req.on('response', (response) => {
			response.resume();
			resolve(response.statusCode ?? 0);
		});
		req.on('error', reject);
		req.setTimeout(5_000, () => req.destroy(new Error('upgrade request timed out')));
		req.end();
	});
}

test.describe('rate limits', () => {
	let stack: Stack;
	let baseUrl: string;
	const post = (path: string, headers: Auth, body: unknown) =>
		fetch(`${baseUrl}${path}`, { method: 'POST', headers, body: JSON.stringify(body) });
	const get = (path: string, headers: Auth) => fetch(`${baseUrl}${path}`, { headers });

	test.beforeAll(async () => {
		test.setTimeout(120_000);
		stack = await startStack('mock', 'rate-limits', 'deployed');
		baseUrl = stack.baseUrl;
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

	test('meeting lookups, ICE servers and joins share one limit per client', async () => {
		const meeting = await createMeeting(new Api(baseUrl), {
			...newClient(),
			...bearer(stack.ownerPassword)
		});
		const client = newClient();
		await startOfWindow();
		// One join's worth, then lookups up to the limit.
		expect((await get(`/api/meeting/${meeting}`, client)).status).toBe(200);
		expect((await get(`/api/ice?meeting=${meeting}`, client)).status).toBe(200);
		expect(await upgradeStatus(baseUrl, meeting, client)).toBe(101);
		for (let guess = 4; guess <= PUBLIC_LIMIT; guess++) {
			expect((await get('/api/meeting/no-such-meeting', client)).status, `${guess}`).toBe(404);
		}
		const throttled = await get(`/api/meeting/${meeting}`, client);
		expect(throttled.status).toBe(429);
		expect(throttled.headers.get('retry-after')).toBe('60');
		expect((await get(`/api/ice?meeting=${meeting}`, client)).status).toBe(429);
		expect(await upgradeStatus(baseUrl, meeting, client)).toBe(429);
		expect((await get(`/api/meeting/${meeting}`, newClient())).status, 'another client').toBe(200);
	});
});
