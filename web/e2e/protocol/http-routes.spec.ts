/** Creating and looking up meetings over HTTP: `/api/new`, `/api/meeting/{words}`, `/api/ice`. */
import { bearer, body, NO_AUTH, type IceWire, type MeetingInfoWire } from '../fixtures/api.ts';
import { expect, test } from '../fixtures/test.ts';

test.describe('POST /api/new', () => {
	test('answers 401 without a secret and with a wrong one', async ({ api }) => {
		expect((await api.newMeeting(NO_AUTH)).status).toBe(401);
		expect((await api.newMeeting(bearer('wrong'))).status).toBe(401);
	});

	test('creates a meeting with the bearer secret: 201 and three words, nothing else', async ({
		api,
		stack
	}) => {
		const response = await api.newMeeting(bearer(stack.ownerPassword), { recording: 'fromStart' });
		expect(response.status).toBe(201);
		const created = await body<Record<string, unknown>>(response);
		expect(Object.keys(created)).toEqual(['meeting']);
		expect(created.meeting).toMatch(/^[a-z]+-[a-z]+-[a-z]+$/);
	});

	test("creates a meeting with the owner's cookie", async ({ api, owner }) => {
		expect((await api.newMeeting(owner.headers, { recording: 'hostDecides' })).status).toBe(201);
	});
});

test.describe('GET /api/meeting/{words}', () => {
	test('reports the recording policy and the scheduled phase', async ({ api, owner }) => {
		const { meeting } = await body<{ meeting: string }>(
			await api.newMeeting(owner.headers, { recording: 'fromStart' })
		);
		const response = await api.meeting(meeting);
		expect(response.status).toBe(200);
		expect(await body<MeetingInfoWire>(response)).toMatchObject({
			recordingPolicy: 'fromStart',
			phase: 'scheduled',
			ended: false
		});
		expect((await api.meeting(meeting.toUpperCase())).status, 'case-insensitive').toBe(200);
	});

	test('answers 404 for an unknown meeting', async ({ api }) => {
		expect((await api.meeting('no-such-meeting')).status).toBe(404);
	});
});

test('GET /ws/{words} without an upgrade answers 426', async ({ api, owner }) => {
	const { meeting } = await body<{ meeting: string }>(await api.newMeeting(owner.headers));
	expect((await api.socketWithoutUpgrade(meeting)).status).toBe(426);
});

test.describe('GET /api/ice', () => {
	test('serves Cloudflare STUN for a known meeting when no TURN key is set', async ({
		api,
		owner
	}) => {
		const { meeting } = await body<{ meeting: string }>(await api.newMeeting(owner.headers));
		const response = await api.ice(meeting);
		expect(response.status).toBe(200);
		expect(await body<IceWire>(response)).toEqual({
			iceServers: [{ urls: ['stun:stun.cloudflare.com:3478'] }]
		});
	});

	test('answers 404 without a meeting and for an unknown one', async ({ api }) => {
		expect((await api.ice(null)).status).toBe(404);
		expect((await api.ice('no-such-meeting')).status).toBe(404);
	});
});
