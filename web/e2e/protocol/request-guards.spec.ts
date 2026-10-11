/**
 * The Worker's guards on every request: `Cache-Control: no-store` on `/api/*`, and the rule that
 * a browser request the owner's cookie would authorize must come from the app's own origin
 * (`Sec-Fetch-Site`, else `Origin`). Node sends neither header unless a test sets it, so it
 * stands for a script, which the rule does not check.
 */
import { randomBytes } from 'node:crypto';
import { request } from 'node:http';
import type { MeetingId, TranscriptRef } from '../../src/lib/protocol.ts';
import { bearer, body, createMeeting, type Auth } from '../fixtures/api.ts';
import { expect, test } from '../fixtures/test.ts';

const FOREIGN = 'https://elsewhere.example';
const JSON_BODY: Auth = { 'content-type': 'application/json' };

/** The status of a WebSocket upgrade to `meeting` with extra request `headers`: 101 if accepted. */
function upgradeStatus(baseUrl: string, meeting: MeetingId, headers: Auth): Promise<number> {
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

test('every /api response is no-store, refusals and errors included', async ({
	api,
	owner,
	stack
}) => {
	const meeting = await createMeeting(api, bearer(stack.ownerPassword));
	const responses = {
		login: await api.login(stack.ownerPassword),
		wrongLogin: await api.login('wrong'),
		me: await api.me(owner.headers),
		new: await api.newMeeting(bearer(stack.ownerPassword)),
		meetings: await api.meetings(owner.headers),
		unauthorized: await api.meetings({}),
		meeting: await api.meeting(meeting),
		unknownMeeting: await api.meeting('no-such-meeting'),
		ice: await api.ice(meeting),
		transcript: await api.transcript('0'.repeat(32) as TranscriptRef),
		logout: await api.logout()
	};
	for (const [name, response] of Object.entries(responses)) {
		expect(response.headers.get('cache-control'), name).toBe('no-store');
	}
});

test.describe('POST /api/new with the owner cookie', () => {
	test('from the app’s origin with JSON creates the meeting', async ({ api, owner, stack }) => {
		for (const browser of [{ 'sec-fetch-site': 'same-origin' }, { origin: stack.baseUrl }]) {
			const response = await api.newMeeting(
				{ ...owner.headers, ...browser, ...JSON_BODY },
				{ recording: 'fromStart' }
			);
			expect(response.status, JSON.stringify(browser)).toBe(201);
		}
	});

	test('from another origin, a sibling subdomain included, answers 403', async ({ api, owner }) => {
		for (const browser of [
			{ origin: FOREIGN },
			{ origin: 'null' },
			{ 'sec-fetch-site': 'same-site' },
			{ 'sec-fetch-site': 'cross-site', origin: FOREIGN }
		]) {
			const response = await api.newMeeting({ ...owner.headers, ...browser, ...JSON_BODY });
			expect(response.status, JSON.stringify(browser)).toBe(403);
		}
	});

	test('from the app’s origin without JSON answers 415, as a form post would', async ({
		api,
		owner,
		stack
	}) => {
		// Node's fetch sends a string body as `text/plain`, like an HTML form with that enctype.
		const response = await api.newMeeting(
			{ ...owner.headers, origin: stack.baseUrl },
			{ recording: 'fromStart' }
		);
		expect(response.status).toBe(415);
	});

	test('the bearer secret works from any origin, since no browser holds it', async ({
		api,
		stack
	}) => {
		const response = await api.newMeeting({
			...bearer(stack.ownerPassword),
			origin: FOREIGN,
			'sec-fetch-site': 'cross-site'
		});
		expect(response.status).toBe(201);
	});
});

test('POST /api/logout from another origin answers 403, from the app 204', async ({ stack }) => {
	const logout = (headers: Auth) =>
		fetch(`${stack.baseUrl}/api/logout`, { method: 'POST', headers });
	expect((await logout({ origin: FOREIGN })).status).toBe(403);
	expect((await logout({ 'sec-fetch-site': 'same-site' })).status).toBe(403);
	expect((await logout({ 'sec-fetch-site': 'same-origin' })).status).toBe(204);
});

test('a host socket from another origin is refused; a guest socket is not', async ({
	api,
	owner,
	stack
}) => {
	const { meeting } = await body<{ meeting: MeetingId }>(
		await api.newMeeting(bearer(stack.ownerPassword))
	);
	const status = (headers: Auth) => upgradeStatus(stack.baseUrl, meeting, headers);
	expect(await status({ ...owner.headers, origin: FOREIGN })).toBe(403);
	expect(await status({ ...owner.headers, 'sec-fetch-site': 'same-site' })).toBe(403);
	expect(await status({ origin: FOREIGN }), 'a guest from anywhere').toBe(101);
	expect(await status({ ...owner.headers, origin: stack.baseUrl })).toBe(101);
});

test('static pages forbid framing and sniffing, and carry the script policy', async ({ stack }) => {
	const transcriptPage = `/t/${'0'.repeat(32)}`;
	// The index; a meeting page, which the SPA fallback serves; a transcript page.
	for (const path of ['/', '/abide-acid-acorn', transcriptPage]) {
		const page = await fetch(`${stack.baseUrl}${path}`);
		expect(page.status, path).toBe(200);
		expect(page.headers.get('x-content-type-options'), path).toBe('nosniff');
		expect(page.headers.get('content-security-policy'), path).toBe("frame-ancestors 'none'");
		const policy = /<meta http-equiv="content-security-policy" content="([^"]+)"/.exec(
			await page.text()
		)?.[1];
		expect(policy, path).toContain("script-src 'self' 'wasm-unsafe-eval' 'sha256-");
	}
	const transcript = await fetch(`${stack.baseUrl}${transcriptPage}`);
	expect(transcript.headers.get('referrer-policy')).toBe('no-referrer');
});
