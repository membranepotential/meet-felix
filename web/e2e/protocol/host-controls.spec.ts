/** What a host can do to others: lock, admit a knock, mute, kick; and what a guest cannot. */
import { isOp } from '../fixtures/mock-sfu-control.ts';
import { joinAs, liveMeeting } from '../fixtures/meeting.ts';
import { publishOffer, receive } from '../fixtures/negotiation.ts';
import { expect, test } from '../fixtures/test.ts';

test('lock: a newcomer knocks, the host admits them', async ({ api, owner, rooms }) => {
	const { meeting, host, guest } = await liveMeeting({ api, owner, rooms });
	host.client.send({ type: 'lock', on: true });
	await guest.client.take('state', (m) => m.state.locked);
	const third = await rooms.open(meeting);
	third.join({ name: 'Third' });
	await third.take('waiting');
	const knock = await host.client.take('knock');
	host.client.send({ type: 'admit', who: knock.who });
	expect((await third.take('welcome')).you).toBe(knock.who);
});

test('kick: the kicked client is told, closed, its tracks closed, and a Join right after does not bring it back', async ({
	api,
	owner,
	rooms,
	sfu
}) => {
	const { meeting, host, guest } = await liveMeeting({ api, owner, rooms });
	const third = await joinAs(rooms, meeting, 'Third');
	third.client.send(publishOffer([{ mid: '0', kind: 'audio', name: 'third-mic' }]));
	await third.client.take('publishAnswer');
	await Promise.all([host, guest].map((p) => receive(p.client, sfu, third.id)));

	// The mock holds the kick's tracks/close while the kicked client tries to join again.
	await sfu.setCloseDelay(1_000);
	host.client.send({ type: 'kick', who: third.id });
	await third.client.take('kicked');
	third.client.join({ name: 'Third' });
	await third.client.closes();
	await sfu.waitForCall(isOp('tracks/close'));
	const [knocked, waited] = await Promise.all([
		host.client.gets('knock', (m) => m.who === third.id, 2_000),
		third.client.gets('waiting', undefined, 2_000)
	]);
	expect({ knocked, waited }).toEqual({ knocked: false, waited: false });
});

test('a guest cannot kick', async ({ api, owner, rooms }) => {
	const { host, guest } = await liveMeeting({ api, owner, rooms });
	guest.client.send({ type: 'kick', who: host.id });
	await guest.client.take('error', (m) => m.code === 'notHost');
});

test("a host's mute reaches the participant", async ({ api, owner, rooms }) => {
	const { host, guest } = await liveMeeting({ api, owner, rooms });
	host.client.send({ type: 'mute', who: guest.id });
	await guest.client.take('muted');
});
