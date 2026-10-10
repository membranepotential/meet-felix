/**
 * Publish and subscribe through the mock SFU: the room forwards a publisher's offer and the
 * SFU's answer, pulls the track into the others' subscribe sessions (retrying the first pull,
 * which the SFU answers with `not_found_track_error`), leaves out a track the SFU rejected, and
 * closes tracks on Unpublish.
 */
import { isPublish, isPull, isOp, type SfuCall } from '../fixtures/mock-sfu-control.ts';
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

test('a track the SFU rejects is not published, and nobody pulls it', async ({
	api,
	owner,
	rooms,
	sfu
}) => {
	const { host, guest } = await liveMeeting({ api, owner, rooms });
	const audio = { mid: '0', kind: 'audio', name: 'mic' } as const;
	const video = { mid: '1', kind: 'video', name: 'cam' } as const;
	// The SDP has no section for the camera's mid, so the mock SFU rejects that track alone
	// (`invalid_track`) and publishes the microphone.
	host.client.send({ ...publishOffer([audio]), tracks: [audio, video] });
	const answer = await host.client.take('publishAnswer');
	expect(answer.rejected).toEqual(['video']);
	const listed = await guest.client.take('state', ({ state }) =>
		state.participants.some((p) => p.id === host.id && p.tracks.length > 0)
	);
	const shown = listed.state.participants.find((p) => p.id === host.id);
	expect(shown?.tracks).toEqual(['audio']);

	const { offer } = await receive(guest.client, sfu, host.id);
	expect(offer.tracks.map((t) => t.track)).toEqual([{ owner: host.id, kind: 'audio' }]);
	const pulled = (await sfu.calls()).filter(isPull).flatMap((c) => c.body.tracks);
	expect(pulled.map((t) => t.trackName)).not.toContain(video.name);
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
