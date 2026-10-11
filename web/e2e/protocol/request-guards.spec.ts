/**
 * The Worker's guards on every request: `Cache-Control: no-store` on `/api/*`, and the rule that
 * a browser request the owner's cookie would authorize must come from the app's own origin
 * (`Sec-Fetch-Site`, else `Origin`). Node sends neither header unless a test sets it, so it
 * stands for a script, which the rule does not check.
 */
import type { TranscriptRef } from '../../src/lib/protocol.ts';
import { bearer, createMeeting, type Auth } from '../fixtures/api.ts';
import { joinAs } from '../fixtures/meeting.ts';
import { expect, test } from '../fixtures/test.ts';

const FOREIGN = 'https://elsewhere.example';
const JSON_BODY: Auth = { 'content-type': 'application/json' };

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

test('a socket with the owner cookie joins as host from the app, as a guest from elsewhere', async ({
	api,
	owner,
	rooms,
	stack
}) => {
	const meeting = await createMeeting(api, bearer(stack.ownerPassword));
	const roleOf = async (name: string, headers: Auth) => {
		const joined = await joinAs(rooms, meeting, name, headers);
		return joined.welcome.state.participants.find((p) => p.id === joined.id)?.role;
	};
	expect(await roleOf('Same origin', { ...owner.headers, 'sec-fetch-site': 'same-origin' })).toBe(
		'host'
	);
	expect(await roleOf('Own origin', { ...owner.headers, origin: stack.baseUrl })).toBe('host');
	// The cookie is ignored, not refused: a refusal would make the app retry without end.
	expect(await roleOf('Cross site', { ...owner.headers, 'sec-fetch-site': 'cross-site' })).toBe(
		'guest'
	);
	expect(await roleOf('Elsewhere', { ...owner.headers, origin: FOREIGN })).toBe('guest');
	expect(await api.upgradeStatus(meeting, { origin: FOREIGN }), 'a guest from anywhere').toBe(101);
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
