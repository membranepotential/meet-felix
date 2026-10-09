/**
 * SFU failures: a lost session is replaced, not retried (D33), and the client can ask for a
 * new session with the epoch of the one that failed; with the SFU down, joining fails.
 */
import { createMeeting } from '../fixtures/api.ts';
import { isOp, isPull } from '../fixtures/mock-sfu-control.ts';
import { liveMeeting } from '../fixtures/meeting.ts';
import { publishOffer, receive } from '../fixtures/negotiation.ts';
import { expect, test } from '../fixtures/test.ts';

test('a pull answered with 410 session_error makes the room replace the subscribe session', async ({
	api,
	owner,
	rooms,
	sfu
}) => {
	const { host, guest } = await liveMeeting({ api, owner, rooms }, 'hostDecides');
	await sfu.loseNextPull();
	host.client.send(publishOffer([{ mid: '0', kind: 'audio', name: 'mic' }]));
	await host.client.take('publishAnswer');

	await guest.client.take('subscribeReset', (m) => m.epoch === 1, 8_000);
	const lost = (await sfu.waitForCall(isPull)).session;
	const calls = await sfu.calls();
	const lostAt = calls.find((c) => c.session === lost)?.seq ?? 0;
	expect(
		calls.filter((c) => c.op === 'new' && c.seq > lostAt),
		'one new session'
	).toHaveLength(1);

	const recovered = await receive(guest.client, sfu, host.id);
	expect(recovered.session).not.toBe(lost);
	expect(
		(await sfu.calls()).filter((c) => c.session === lost),
		'no further call on the lost session'
	).toHaveLength(1);
});

test('ResetSession is answered with a new epoch, and a stale one changes nothing', async ({
	api,
	owner,
	rooms,
	sfu
}) => {
	const { host, guest } = await liveMeeting({ api, owner, rooms }, 'hostDecides');
	guest.client.send({ type: 'resetSession', kind: 'subscribe', epoch: 0 });
	await guest.client.take('subscribeReset', (m) => m.epoch === 1);
	await sfu.waitForCall(isOp('new'));

	// Epoch 0 is the session just replaced: a late report about it must not replace epoch 1.
	guest.client.send({ type: 'resetSession', kind: 'subscribe', epoch: 0 });
	expect(await guest.client.gets('subscribeReset', undefined, 1_500)).toBe(false);

	guest.client.send({ type: 'resetSession', kind: 'publish', epoch: 0 });
	await guest.client.take('publishReset', (m) => m.epoch === 1);
	host.client.send({ type: 'resetSession', kind: 'subscribe', epoch: 0 });
	await host.client.take('subscribeReset', (m) => m.epoch === 1);
});

test('with the SFU down, joining fails with SfuUnavailable and the socket closes', async ({
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
	await late.closes();
});

test('the owner opening a scheduled meeting with the SFU down gets SfuUnavailable', async ({
	api,
	owner,
	rooms,
	sfu
}) => {
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	await sfu.setDown(true);
	const host = await rooms.open(meeting, owner.headers);
	host.join({ name: 'Host' });
	await host.take('error', (m) => m.code === 'sfuUnavailable');
	await host.closes();
});
