import { describe, expect, it, vi } from 'vitest';
import {
	NO_OUTGOING,
	NO_REMOTE_TRACKS,
	orderVideoCodecs,
	planPublish,
	Publisher,
	Subscriber,
	trackName
} from './sfu.ts';
import type { ClientMsg, Sdp, TrackKind, TrackRef } from './protocol.ts';

describe('orderVideoCodecs', () => {
	it('puts H.264 first, then VP8, keeps helpers and drops VP9 and AV1', () => {
		const codecs = [
			'video/AV1',
			'video/VP9',
			'video/VP8',
			'video/rtx',
			'video/H264',
			'video/red'
		].map((mimeType) => ({ mimeType }));
		expect(orderVideoCodecs(codecs).map((c) => c.mimeType)).toEqual([
			'video/H264',
			'video/VP8',
			'video/rtx',
			'video/red'
		]);
	});
});

describe('planPublish', () => {
	const none = { audio: null, video: null, screen: null };
	const published = (entries: [TrackKind, string][]) => new Map(entries);

	it('adds new kinds, replaces swapped tracks and unpublishes kinds turned off', () => {
		const plan = planPublish(
			published([
				['audio', 'mic-a'],
				['video', 'cam-a']
			]),
			{ audio: 'mic-b', video: null, screen: 'screen-a' },
			new Map()
		);
		expect(plan).toEqual({ unpublish: ['video'], replace: ['audio'], add: ['screen'] });
	});

	it('does nothing when the published tracks match', () => {
		expect(
			planPublish(published([['audio', 'mic']]), { ...none, audio: 'mic' }, new Map())
		).toEqual({ unpublish: [], replace: [], add: [] });
	});

	it('does not retry a failed track, but tries a new one of the same kind', () => {
		const failed = new Map<TrackKind, string>([['video', 'cam-a']]);
		expect(planPublish(new Map(), { ...none, video: 'cam-a' }, failed).add).toEqual([]);
		expect(planPublish(new Map(), { ...none, video: 'cam-b' }, failed).add).toEqual(['video']);
	});
});

describe('trackName', () => {
	it('names tracks by kind and generation', () => {
		expect([trackName('audio', 1), trackName('video', 2), trackName('screen', 3)]).toEqual([
			'mic-1',
			'cam-2',
			'screen-3'
		]);
	});
});

/** Adds the connection state of `RTCPeerConnection` to a fake; `fail()` makes it fail. */
function failable<T extends object>(fake: T) {
	const events = new EventTarget();
	const pc = Object.assign(fake, {
		connectionState: 'new' as RTCPeerConnectionState,
		addEventListener: events.addEventListener.bind(events),
		removeEventListener: events.removeEventListener.bind(events)
	});
	const fail = () => {
		pc.connectionState = 'failed';
		events.dispatchEvent(new Event('connectionstatechange'));
	};
	return { pc, fail };
}

/** The part of `RTCPeerConnection` the publisher uses, answering every offer at once. */
function fakePeerConnection() {
	const transceivers: { mid: string | null; stop: () => void }[] = [];
	const pc = {
		signalingState: 'stable',
		localDescription: null as { sdp: string } | null,
		addTransceiver: () => {
			const transceiver = { mid: null as string | null, stop: vi.fn(), sender: {} };
			transceivers.push(transceiver);
			return transceiver;
		},
		createOffer: () => Promise.resolve({ type: 'offer', sdp: 'offer' }),
		setLocalDescription: () => {
			transceivers.forEach((t, i) => (t.mid ??= String(i)));
			pc.localDescription = { sdp: 'offer' };
			return Promise.resolve();
		},
		setRemoteDescription: () => Promise.resolve(),
		close: () => {}
	};
	const { fail } = failable(pc);
	return { pc: pc as unknown as RTCPeerConnection, transceivers, fail };
}

describe('Publisher', () => {
	const camera = { kind: 'video' } as MediaStreamTrack;

	/**
	 * A publisher whose offers the room answers right away while `connected` is true; the answer
	 * says the SFU did not publish the `rejected` kinds.
	 */
	function setup(rejected: TrackKind[] = []) {
		const { pc, transceivers, fail } = fakePeerConnection();
		const sent: ClientMsg[] = [];
		const publishFailed = vi.fn();
		const connectionFailed = vi.fn();
		let connected = true;
		const publisher: Publisher = new Publisher(
			Promise.resolve(pc),
			(msg) => {
				if (!connected) return false;
				sent.push(msg);
				if (msg.type === 'publishOffer') {
					queueMicrotask(() => publisher.answer('answer', rejected));
				}
				return true;
			},
			publishFailed,
			connectionFailed
		);
		const setConnected = (on: boolean) => (connected = on);
		return { publisher, sent, transceivers, setConnected, fail, publishFailed, connectionFailed };
	}

	it('reports a failed connection once, and not after it was closed', async () => {
		const { publisher, fail, connectionFailed } = setup();
		await Promise.resolve();
		fail();
		fail();
		expect(connectionFailed).toHaveBeenCalledOnce();

		const closed = setup();
		await Promise.resolve();
		closed.publisher.close();
		closed.fail();
		expect(closed.connectionFailed).not.toHaveBeenCalled();
		publisher.close();
	});

	it('counts a kind the SFU rejected as failed and does not offer it again', async () => {
		const microphone = { kind: 'audio' } as MediaStreamTrack;
		const { publisher, sent, transceivers, publishFailed } = setup(['video']);
		const both = { ...NO_OUTGOING, audio: microphone, video: camera };
		publisher.sync(both);
		await vi.waitFor(() => expect(publishFailed).toHaveBeenCalledWith(['video'], 'refused'));
		expect(transceivers[0]?.stop).not.toHaveBeenCalled();
		expect(transceivers[1]?.stop).toHaveBeenCalledOnce();
		// The same camera track is not offered again; a new one would be.
		publisher.sync(both);
		await Promise.resolve();
		expect(sent.filter((m) => m.type === 'publishOffer')).toHaveLength(1);
	});

	it('unpublishes a camera turned off and stops its transceiver', async () => {
		const { publisher, sent, transceivers } = setup();
		publisher.sync({ ...NO_OUTGOING, video: camera });
		await vi.waitFor(() => expect(sent.map((m) => m.type)).toEqual(['publishOffer']));
		publisher.sync(NO_OUTGOING);
		await vi.waitFor(() => expect(sent.at(-1)).toEqual({ type: 'unpublish', kind: 'video' }));
		expect(transceivers[0]?.stop).toHaveBeenCalledOnce();
	});

	it('sends an unpublish that missed the socket once it is back', async () => {
		const { publisher, sent, setConnected } = setup();
		publisher.sync({ ...NO_OUTGOING, video: camera });
		await vi.waitFor(() => expect(sent).toHaveLength(1));
		setConnected(false);
		publisher.sync(NO_OUTGOING);
		await Promise.resolve();
		setConnected(true);
		publisher.retryFailed();
		expect(sent.at(-1)).toEqual({ type: 'unpublish', kind: 'video' });
		publisher.retryFailed();
		expect(sent).toHaveLength(2);
	});
});

/** The part of `RTCPeerConnection` the subscriber uses; each offer brings the transceivers `mids`. */
function fakeSubscribePeerConnection() {
	let mids: string[] = [];
	const pc = {
		localDescription: null as { sdp: string } | null,
		setRemoteDescription: vi.fn(() => Promise.resolve()),
		getTransceivers: () =>
			mids.map((mid) => ({ mid, receiver: { track: { id: `track-${mid}` } } })),
		createAnswer: () => Promise.resolve({ type: 'answer', sdp: 'answer' }),
		setLocalDescription: () => {
			pc.localDescription = { sdp: 'answer' };
			return Promise.resolve();
		},
		close: () => {}
	};
	const offerMids = (next: string[]) => (mids = next);
	const { fail } = failable(pc);
	return { pc, offerMids, fail };
}

describe('Subscriber', () => {
	function setup() {
		const { pc, offerMids, fail } = fakeSubscribePeerConnection();
		const answers: Sdp[] = [];
		const delivered: TrackRef[][] = [];
		const connectionFailed = vi.fn();
		/** The received tracks, held as the subscriber's owner holds them. */
		const held = { tracks: NO_REMOTE_TRACKS };
		const subscriber = new Subscriber(
			Promise.resolve(pc as unknown as RTCPeerConnection),
			(sdp) => answers.push(sdp),
			(update) => (held.tracks = update(held.tracks)),
			(refs) => delivered.push(refs),
			connectionFailed
		);
		return { subscriber, pc, offerMids, answers, delivered, held, fail, connectionFailed };
	}

	it('reports a failed connection once', async () => {
		const { fail, connectionFailed } = setup();
		await Promise.resolve();
		fail();
		fail();
		expect(connectionFailed).toHaveBeenCalledOnce();
	});

	it('answers an offer that carries no track', async () => {
		const { subscriber, pc, answers, delivered, held } = setup();
		subscriber.offer('offer', []);
		await vi.waitFor(() => expect(answers).toEqual(['answer']));
		expect(pc.setRemoteDescription).toHaveBeenCalledWith({ type: 'offer', sdp: 'offer' });
		expect(delivered).toEqual([[]]);
		expect(held.tracks.size).toBe(0);
	});

	it("drops an unpublished kind but keeps the owner's other tracks", async () => {
		const { subscriber, offerMids, delivered, held } = setup();
		const audio: TrackRef = { owner: 2, kind: 'audio' };
		const video: TrackRef = { owner: 2, kind: 'video' };
		offerMids(['0', '1']);
		subscriber.offer('offer', [
			{ mid: '0', track: audio },
			{ mid: '1', track: video }
		]);
		await vi.waitFor(() => expect(delivered).toEqual([[audio, video]]));
		subscriber.retain([audio]);
		const kept = held.tracks;
		expect([...kept.keys()]).toEqual(['2:audio']);
		subscriber.retain([audio]);
		expect(held.tracks).toBe(kept);
	});

	it('answers and delivers nothing once closed', async () => {
		const { subscriber, offerMids, answers, delivered, held } = setup();
		offerMids(['0']);
		subscriber.offer('offer', [{ mid: '0', track: { owner: 2, kind: 'audio' } }]);
		subscriber.close();
		subscriber.retain([]);
		await new Promise((resolve) => setTimeout(resolve, 0));
		expect(answers).toEqual([]);
		expect(delivered).toEqual([]);
		expect(held.tracks).toBe(NO_REMOTE_TRACKS);
	});
});
