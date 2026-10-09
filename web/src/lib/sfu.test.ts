import { describe, expect, it, vi } from 'vitest';
import { NO_OUTGOING, orderVideoCodecs, planPublish, Publisher, trackName } from './sfu.ts';
import type { ClientMsg, TrackKind } from './protocol.ts';

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
	return { pc: pc as unknown as RTCPeerConnection, transceivers };
}

describe('Publisher', () => {
	const camera = { kind: 'video' } as MediaStreamTrack;

	/** A publisher whose offers the room answers right away while `connected` is true. */
	function setup() {
		const { pc, transceivers } = fakePeerConnection();
		const sent: ClientMsg[] = [];
		let connected = true;
		const publisher: Publisher = new Publisher(
			pc,
			(msg) => {
				if (!connected) return false;
				sent.push(msg);
				if (msg.type === 'publishOffer') queueMicrotask(() => publisher.answer('answer'));
				return true;
			},
			() => {}
		);
		const setConnected = (on: boolean) => (connected = on);
		return { publisher, sent, transceivers, setConnected };
	}

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
