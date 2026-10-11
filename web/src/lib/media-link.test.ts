import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ClientMsg } from './protocol.ts';
import { MediaLink } from './media-link.svelte.ts';
import { NO_OUTGOING } from './sfu.ts';

vi.mock('./api.ts', () => ({
	getIceServers: () => Promise.resolve({ ok: false, error: { kind: 'network' } })
}));

/**
 * The parts of `RTCPeerConnection` a link uses while nothing is published; a subscribe offer
 * brings one transceiver, mid `0`.
 */
class FakePeerConnection extends EventTarget {
	static all: FakePeerConnection[] = [];
	connectionState: RTCPeerConnectionState = 'new';
	localDescription: { sdp: string } | null = null;
	closed = false;

	constructor() {
		super();
		FakePeerConnection.all.push(this);
	}

	close(): void {
		this.closed = true;
	}

	fail(): void {
		this.connectionState = 'failed';
		this.dispatchEvent(new Event('connectionstatechange'));
	}

	async setRemoteDescription(): Promise<void> {}

	getTransceivers() {
		return [{ mid: '0', receiver: { track: { id: 'track-0' } } }];
	}

	async createAnswer() {
		return { type: 'answer', sdp: 'answer' };
	}

	async setLocalDescription(): Promise<void> {
		this.localDescription = { sdp: 'answer' };
	}
}

/** Lets the peer connections that wait for the ICE servers come into being. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const open = () => FakePeerConnection.all.filter((pc) => !pc.closed).length;

describe('MediaLink', () => {
	let link: MediaLink;
	let sent: ClientMsg[];

	beforeEach(() => {
		FakePeerConnection.all = [];
		vi.stubGlobal('RTCPeerConnection', FakePeerConnection);
		vi.spyOn(console, 'warn').mockImplementation(() => {});
		sent = [];
		link = new MediaLink(
			'amber-otter-ocean',
			(msg) => sent.push(msg) > 0,
			() => NO_OUTGOING,
			() => {}
		);
	});

	afterEach(() => {
		link.dispose();
		vi.unstubAllGlobals();
		vi.restoreAllMocks();
	});

	it('keeps both connections on a resume and replaces the one whose session the room replaced', async () => {
		link.run({ kind: 'welcomed', fresh: true, epochs: { publish: 1, subscribe: 1 } });
		await settle();
		expect(FakePeerConnection.all).toHaveLength(2);

		link.run({ kind: 'welcomed', fresh: false, epochs: { publish: 1, subscribe: 1 } });
		await settle();
		expect(FakePeerConnection.all).toHaveLength(2);

		// The room replaced the subscribe session while the socket was down.
		link.run({ kind: 'welcomed', fresh: false, epochs: { publish: 1, subscribe: 2 } });
		await settle();
		expect(FakePeerConnection.all).toHaveLength(3);
		expect(FakePeerConnection.all[1]?.closed).toBe(true);
		expect(open()).toBe(2);

		// A new participant gets new connections whatever the epochs say.
		link.run({ kind: 'welcomed', fresh: true, epochs: { publish: 1, subscribe: 2 } });
		await settle();
		expect(open()).toBe(2);
		expect(FakePeerConnection.all).toHaveLength(5);
	});

	it('asks the room for a new session when a connection fails', async () => {
		link.run({ kind: 'welcomed', fresh: true, epochs: { publish: 1, subscribe: 1 } });
		await settle();
		FakePeerConnection.all[1]?.fail();
		expect(sent).toEqual([{ type: 'resetSession', kind: 'subscribe', epoch: 1 }]);

		// The new connection fails as well: the room hears about its epoch, once.
		link.run({ kind: 'subscribe-reset', epoch: 2 });
		await settle();
		FakePeerConnection.all[2]?.fail();
		FakePeerConnection.all[2]?.fail();
		expect(sent.slice(1)).toEqual([{ type: 'resetSession', kind: 'subscribe', epoch: 2 }]);
	});

	it('receives the tracks of the current subscribe connection and none after a reset', async () => {
		link.run({ kind: 'welcomed', fresh: true, epochs: { publish: 1, subscribe: 1 } });
		link.run({
			kind: 'subscribe-offer',
			sdp: 'offer',
			tracks: [{ mid: '0', track: { owner: 2, kind: 'audio' } }]
		});
		await settle();
		expect([...link.remote.keys()]).toEqual(['2:audio']);
		expect(sent).toContainEqual({ type: 'subscribeAnswer', sdp: 'answer' });

		link.run({ kind: 'subscribe-reset', epoch: 2 });
		expect(link.remote.size).toBe(0);
	});
});
