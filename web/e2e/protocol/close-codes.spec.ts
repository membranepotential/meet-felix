/**
 * The close codes the room sends tell the client whether to come back (`CloseCode` in the
 * core): final ones in 4000–4099, a reload for an outdated page, and 1013 for a join the SFU
 * refused, which the client retries with backoff.
 */
import { liveMeeting, joinAs } from '../fixtures/meeting.ts';
import { PING } from '../fixtures/room-client.ts';
import { expect, test } from '../fixtures/test.ts';

test('kick and end close with their final codes', async ({ api, owner, rooms }) => {
	const { meeting, host, guest } = await liveMeeting({ api, owner, rooms });
	const third = await joinAs(rooms, meeting, 'Third');
	host.client.send({ type: 'kick', who: third.id });
	await third.client.take('kicked');
	expect(await third.client.closes()).toEqual({
		code: 4001,
		reason: 'A host removed you from the meeting.'
	});

	host.client.send({ type: 'end' });
	const closes = await Promise.all([host.client.closes(), guest.client.closes()]);
	expect(closes.map((c) => c.code)).toEqual([4002, 4002]);
});

test('a socket that never joined is closed at the end with a code that reconnects', async ({
	api,
	owner,
	rooms
}) => {
	const { meeting, host } = await liveMeeting({ api, owner, rooms });
	// Bound to nobody: it heard no `ended` message, so a final code would show it a refusal. The
	// ping makes local workerd deliver the close at once; on a socket that never sent anything it
	// arrives only when the room's join timeout wakes the room.
	const unjoined = await rooms.open(meeting);
	unjoined.sendRaw(PING);
	await unjoined.take('pong');
	host.client.send({ type: 'end' });
	expect(await unjoined.closes()).toEqual({ code: 1001, reason: 'The meeting has ended.' });
});

test('a page older than the room serves is told to reload', async ({ api, owner, rooms }) => {
	const { meeting } = await liveMeeting({ api, owner, rooms });
	const old = await rooms.open(meeting);
	old.join({ name: 'Old tab', protocol: 0 });
	await old.take('error', (m) => m.code === 'outdatedClient');
	expect(await old.closes()).toEqual({
		code: 4004,
		reason: 'This page is out of date. Reload it to join the meeting.'
	});
	// A page from before the version existed sends none and is still served.
	const unversioned = await rooms.open(meeting);
	unversioned.sendRaw('{"type":"join","name":"Cached tab","consent":true}');
	await unversioned.take('welcome');
});

test('a join the SFU refuses closes with 1013, so the client comes back later', async ({
	api,
	owner,
	rooms,
	sfu
}) => {
	const { meeting } = await liveMeeting({ api, owner, rooms });
	await sfu.setDown(true);
	const late = await rooms.open(meeting);
	late.join({ name: 'Late' });
	await late.take('error', (m) => m.code === 'sfuUnavailable');
	expect((await late.closes()).code).toBe(1013);
});
