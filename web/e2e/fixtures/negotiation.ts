/** SDP exchanges for protocol tests, which play the browser's part with fake SDP. */
import { randomBytes } from 'node:crypto';
import type { LocalTrack, ParticipantId } from '../../src/lib/protocol.ts';
import { newTransport, offerSdp, type SdpMediaKind } from '../sdp.ts';
import { isOp, type MockSfu, type SfuCall } from './mock-sfu-control.ts';

type Renegotiation = Extract<SfuCall, { op: 'renegotiate' }>;
import type { RoomClient, ServerMsgOf } from './room-client.ts';

const random = (bytes: number) => new Uint8Array(randomBytes(bytes));
const sdpKind = (kind: LocalTrack['kind']): SdpMediaKind => (kind === 'audio' ? 'audio' : 'video');

/** A `PublishOffer` for `tracks`, with an SDP offer the mock SFU can answer. */
export function publishOffer(tracks: LocalTrack[]): {
	type: 'publishOffer';
	sdp: string;
	tracks: LocalTrack[];
} {
	const sdp = offerSdp(
		tracks.map((t, i) => ({
			mid: t.mid,
			kind: sdpKind(t.kind),
			direction: 'sendonly',
			ssrc: 1000 + i,
			stream: 'e2e-client',
			track: t.name
		})),
		newTransport(random),
		{ id: '1', version: 1 }
	);
	return { type: 'publishOffer', sdp, tracks };
}

let answers = 0;

export interface Received {
	/** The offer that carried a track of the owner. */
	offer: ServerMsgOf<'subscribeOffer'>;
	/** The offers before it, which did not. */
	before: ServerMsgOf<'subscribeOffer'>[];
	/** The subscribe session the answers went to. */
	session: string;
}

/**
 * Answers every `SubscribeOffer` `client` gets until one carries a track of `owner`, and waits
 * until the SFU got each answer. Fails if no such offer comes within `timeoutMs`.
 */
export async function receive(
	client: RoomClient,
	sfu: MockSfu,
	owner: ParticipantId | undefined,
	timeoutMs = 15_000
): Promise<Received> {
	const deadline = Date.now() + timeoutMs;
	const before: ServerMsgOf<'subscribeOffer'>[] = [];
	for (;;) {
		const offer = await client.take(
			'subscribeOffer',
			undefined,
			Math.max(deadline - Date.now(), 1)
		);
		const sdp = `client-answer-${answers++}`;
		client.send({ type: 'subscribeAnswer', sdp });
		const call = await sfu.waitForCall(
			(c): c is Renegotiation => isOp('renegotiate')(c) && c.body.sessionDescription.sdp === sdp
		);
		// The SFU's reply reaches the room a moment after the mock logged the call.
		await new Promise((resolve) => setTimeout(resolve, 200));
		if (offer.tracks.some((t) => t.track.owner === owner)) {
			return { offer, before, session: call.session };
		}
		before.push(offer);
	}
}
