/**
 * Every refusal and failure the Worker's router answers has one body, `{"error": code}`
 * (`ErrorBody` in `crates/meeting-core/src/http.rs`), with the code's status and nothing else:
 * no parser message, no internal detail.
 */
import type { TranscriptRef } from '../../src/lib/protocol.ts';
import { bearer, createMeeting, type ErrorBody } from '../fixtures/api.ts';
import { expect, test } from '../fixtures/test.ts';

test('every refusal is the error body alone, with its status', async ({ api, stack }) => {
	const at = (path: string, init?: RequestInit) => fetch(`${stack.baseUrl}${path}`, init);
	const unknownToken: TranscriptRef = 'f'.repeat(32);
	const meeting = await createMeeting(api, bearer(stack.ownerPassword));
	const cases: [string, Response, number, ErrorBody['error']][] = [
		['a login body of the wrong type', await api.loginWith('{"password": 1}'), 400, 'badRequest'],
		['a login that is not JSON', await api.loginWith('password=x'), 400, 'badRequest'],
		['a wrong password', await api.login('wrong'), 401, 'unauthorized'],
		['the owner’s list without a session', await api.meetings({}), 401, 'unauthorized'],
		[
			'a cross-site logout',
			await at('/api/logout', { method: 'POST', headers: { origin: 'https://elsewhere.example' } }),
			403,
			'crossOrigin'
		],
		['an unknown meeting', await api.meeting('no-such-meeting'), 404, 'notFound'],
		['an unknown transcript', await api.transcript(unknownToken), 404, 'notFound'],
		['a malformed transcript token', await api.transcript('nope'), 404, 'notFound'],
		['an unknown download', await at(`/api/transcript/${unknownToken}/audio.wav`), 404, 'notFound'],
		['an unknown route', await at('/api/no-such-route'), 404, 'notFound'],
		[
			'a known route with another method',
			await at('/api/me', { method: 'DELETE' }),
			405,
			'methodNotAllowed'
		],
		[
			'a socket path without an upgrade',
			await api.socketWithoutUpgrade(meeting),
			426,
			'upgradeRequired'
		]
	];
	for (const [name, response, status, error] of cases) {
		expect.soft(response.status, name).toBe(status);
		expect.soft(response.headers.get('content-type'), name).toMatch(/^application\/json/);
		expect.soft(await response.json(), name).toEqual({ error });
	}
});
