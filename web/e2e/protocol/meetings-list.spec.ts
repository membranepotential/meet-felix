/** The owner's list of meetings, `GET /api/meetings`, with each meeting's phase. */
import {
	bearer,
	body,
	createMeeting,
	NO_AUTH,
	type Api,
	type Auth,
	type PastMeeting
} from '../fixtures/api.ts';
import { joinAs } from '../fixtures/meeting.ts';
import { expect, test } from '../fixtures/test.ts';

const list = async (api: Api, auth: Auth) =>
	(await body<{ meetings: PastMeeting[] }>(await api.meetings(auth))).meetings;

test('answers 401 without a secret and with a wrong one, 200 with the cookie', async ({
	api,
	owner
}) => {
	expect((await api.meetings(NO_AUTH)).status).toBe(401);
	expect((await api.meetings(bearer('wrong'))).status).toBe(401);
	expect((await api.meetings(owner.headers)).status).toBe(200);
});

test('lists meetings newest first, an ended one with its transcript and a new one as scheduled', async ({
	api,
	owner,
	stack,
	rooms
}) => {
	const scheduled = await createMeeting(api, owner.headers, 'hostDecides');
	const ended = await createMeeting(api, owner.headers, 'fromStart');
	const host = await joinAs(rooms, ended, 'Host', owner.headers);
	host.client.send({ type: 'end' });
	const { transcript } = await host.client.take('ended');

	const meetings = await list(api, bearer(stack.ownerPassword));
	const createdAt = meetings.map((m) => m.createdAt);
	expect(createdAt).toEqual(createdAt.toSorted((a, b) => b - a));
	expect(meetings.find((m) => m.meeting === scheduled)).toMatchObject({ phase: 'scheduled' });
	expect(meetings.find((m) => m.meeting === scheduled)?.transcript).toBeUndefined();
	// The index settles an ended meeting a moment after the room ended.
	await expect
		.poll(async () => (await list(api, owner.headers)).find((m) => m.meeting === ended))
		.toMatchObject({ phase: 'ended', transcript, createdAt: expect.any(Number) });
});

test('an ended meeting that never recorded lists no transcript', async ({ api, owner, rooms }) => {
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	const host = await joinAs(rooms, meeting, 'Host', owner.headers);
	host.client.send({ type: 'end' });
	expect((await host.client.take('ended')).transcript).toBeUndefined();

	const listed = async () => (await list(api, owner.headers)).find((m) => m.meeting === meeting);
	await expect.poll(async () => (await listed())?.phase).toBe('ended');
	expect(await listed()).not.toHaveProperty('transcript');
});
