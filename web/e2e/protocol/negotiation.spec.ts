/**
 * Publish and subscribe through the mock SFU: the room forwards a publisher's offer and the
 * SFU's answer, pulls the track into the others' subscribe sessions (retrying the first pull,
 * which the SFU answers with `not_found_track_error`), and closes tracks on Unpublish.
 */
import { isPublish, isOp, type SfuCall } from '../fixtures/mock-sfu-control.ts';
import { joinAs, liveMeeting } from '../fixtures/meeting.ts';
import { publishOffer, receive } from '../fixtures/negotiation.ts';
import { expect, test } from '../fixtures/test.ts';
import { attribute, direction, parseSdp } from '../sdp.ts';

const closesMid = (session: string, mid: string | undefined) => (call: SfuCall) =>
	isOp('tracks/close')(call) &&
	call.session === session &&
	call.body.tracks.some((t) => t.mid === mid);

test('a published track reaches the guest after the retry of its first pull', async ({
	api,
	owner,
	rooms,
	sfu
}) => {
	const { host, guest } = await liveMeeting({ api, owner, rooms });
	host.client.send(publishOffer([{ mid: '0', kind: 'audio', name: 'mic' }]));
	const answer = parseSdp((await host.client.take('publishAnswer')).sdp);
	expect(answer?.media.map((m) => [attribute(m.attributes, 'mid'), direction(m)])).toEqual([
		['0', 'recvonly']
	]);

	const { offer, before, session } = await receive(guest.client, sfu, host.id);
	// The first pull fails like on the live SFU before the publisher is connected; its offer
	// still reaches the guest, without tracks, and the room retries after SUBSCRIBE_RETRY_MS.
	expect(before).toHaveLength(1);
	expect(before[0]?.tracks).toEqual([]);
	expect(parseSdp(before[0]?.sdp ?? '')?.media.map(direction)).toEqual(['inactive']);
	expect(offer.tracks).toEqual([
		expect.objectContaining({ track: { owner: host.id, kind: 'audio' } })
	]);
	// `receive` waited for each of the guest's answers to reach the SFU's renegotiate.
	expect(session, "the guest's subscribe session").toMatch(/^[0-9a-f]{32}$/);
});

test('Unpublish closes the track on the publish session and on the subscriber’s', async ({
	api,
	owner,
	rooms,
	sfu
}) => {
	const { host, guest } = await liveMeeting({ api, owner, rooms });
	host.client.send(publishOffer([{ mid: '0', kind: 'audio', name: 'mic' }]));
	await host.client.take('publishAnswer');
	const { offer, session: guestSubscribe } = await receive(guest.client, sfu, host.id);
	const hostPublish = (await sfu.waitForCall(isPublish)).session;

	host.client.send({ type: 'unpublish', kind: 'audio' });
	await sfu.waitForCall(closesMid(hostPublish, '0'));
	await sfu.waitForCall(closesMid(guestSubscribe, offer.tracks[0]?.mid));
});

test('a third participant’s track reaches everyone already in the room', async ({
	api,
	owner,
	rooms,
	sfu
}) => {
	const { meeting, host, guest } = await liveMeeting({ api, owner, rooms });
	const third = await joinAs(rooms, meeting, 'Third');
	third.client.send(publishOffer([{ mid: '0', kind: 'audio', name: 'third-mic' }]));
	await third.client.take('publishAnswer');
	const received = await Promise.all([host, guest].map((p) => receive(p.client, sfu, third.id)));
	for (const { offer } of received) {
		expect(offer.tracks.map((t) => t.track)).toContainEqual({ owner: third.id, kind: 'audio' });
	}
});
