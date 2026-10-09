/**
 * A meeting's life over the room's socket: scheduled → waiting → live → ended, the words
 * released at the end; leaving, resuming, "join as host", and the room surviving an idle phase.
 */
import type { ServerMsgOf } from '../fixtures/room-client.ts';
import { body, createMeeting, type MeetingInfoWire } from '../fixtures/api.ts';
import { joinAs, liveMeeting } from '../fixtures/meeting.ts';
import { PING } from '../fixtures/room-client.ts';
import { expect, test } from '../fixtures/test.ts';

const present = (msg: ServerMsgOf<'state'>, id: number) =>
	msg.state.participants.some((p) => p.id === id);

test('a guest waits in a scheduled room until the owner joins as host; the room goes live', async ({
	api,
	owner,
	rooms
}) => {
	const meeting = await createMeeting(api, owner.headers, 'fromStart');
	// A header the client sets itself must not make anyone the owner.
	const guest = await rooms.open(meeting, { 'x-meet-owner': '1' });
	guest.join({ name: 'Guest' });
	await guest.take('waiting');

	const host = await joinAs(rooms, meeting, 'Host', owner.headers);
	expect(host.welcome.state.participants.find((p) => p.id === host.id)?.role).toBe('host');

	const guestWelcome = await guest.take('welcome');
	expect(guestWelcome.state.phase).toBe('live');
	expect(guestWelcome.state.participants.find((p) => p.id === guestWelcome.you)?.role).toBe(
		'guest'
	);
	expect(await body<MeetingInfoWire>(await api.meeting(meeting))).toMatchObject({
		phase: 'live',
		ended: false
	});
});

test('the room answers the exact ping and rejects a malformed frame', async ({
	api,
	owner,
	rooms
}) => {
	const { host } = await liveMeeting({ api, owner, rooms });
	host.client.sendRaw(PING);
	await host.client.take('pong');
	host.client.sendRaw('not json');
	await host.client.take('error', (m) => m.code === 'malformedMessage');
});

test('a client that closes its socket leaves the room for the others', async ({
	api,
	owner,
	rooms
}) => {
	const { meeting, host } = await liveMeeting({ api, owner, rooms });
	const leaver = await joinAs(rooms, meeting, 'Leaver');
	await host.client.take('state', (m) => present(m, leaver.id));
	leaver.client.close(1000, 'bye');
	await host.client.take('state', (m) => !present(m, leaver.id));
});

test('resume: a new socket with the resume token takes the participant over', async ({
	api,
	owner,
	rooms
}) => {
	const { meeting, host, guest } = await liveMeeting({ api, owner, rooms });
	const resumed = await rooms.open(meeting);
	resumed.join({ name: 'Guest', resume: guest.welcome.resume });
	expect((await resumed.take('welcome')).you).toBe(guest.id);
	await guest.client.closes();
	host.client.send({ type: 'mute', who: guest.id });
	await resumed.take('muted');
});

test('join as host: a guest who reconnects with the cookie and its resume token becomes host', async ({
	api,
	owner,
	rooms
}) => {
	const { meeting, host, guest } = await liveMeeting({ api, owner, rooms });
	const promoted = await rooms.open(meeting, owner.headers);
	promoted.join({ name: 'Guest', resume: guest.welcome.resume });
	const welcome = await promoted.take('welcome');
	expect(welcome.you).toBe(guest.id);
	expect(welcome.state.participants.find((p) => p.id === guest.id)?.role).toBe('host');
	await host.client.take(
		'state',
		(m) => m.state.participants.find((p) => p.id === guest.id)?.role === 'host'
	);
	await guest.client.closes();
});

test('End reaches everyone, closes every socket, and releases the words', async ({
	api,
	owner,
	rooms
}) => {
	const { meeting, host, guest } = await liveMeeting({ api, owner, rooms });
	host.client.send({ type: 'end' });
	await Promise.all([host.client.take('ended'), guest.client.take('ended')]);
	await Promise.all([host.client.closes(), guest.client.closes()]);
	expect([404, 410]).toContain(await api.upgradeStatus(meeting));
	expect((await api.meeting(meeting)).status).toBe(404);
});

test('a room keeps its participants through an idle phase', async ({ api, owner, rooms }) => {
	// Long enough for local workerd to hibernate the room; E2E_IDLE_MS shortens it.
	const idleMs = Number(process.env.E2E_IDLE_MS ?? 15_000);
	test.setTimeout(idleMs + 30_000);
	const { meeting, host, guest } = await liveMeeting({ api, owner, rooms });
	await new Promise((resolve) => setTimeout(resolve, idleMs));
	const late = await joinAs(rooms, meeting, 'Late');
	expect(late.welcome.state.participants.map((p) => p.id)).toEqual(
		expect.arrayContaining([host.id, guest.id])
	);
	await host.client.take('state', (m) => present(m, late.id));
	await guest.client.take('state', (m) => present(m, late.id));
});
