/**
 * Client side of the Cloudflare Realtime SFU (docs/media/README.md, D10–D12).
 *
 * Each participant holds two peer connections, so offers never collide ("glare"):
 * - publish: the client offers its tracks, the room forwards the offer to the SFU and returns
 *   the answer (`ClientMsg::PublishOffer` → `ServerMsg::PublishAnswer`);
 * - subscribe: the room offers remote tracks, the client answers
 *   (`ServerMsg::SubscribeOffer` → `ClientMsg::SubscribeAnswer`).
 *
 * The SFU is ICE-lite and returns its candidates in the SDP, so the client sends its offer
 * right after `setLocalDescription` without waiting for ICE gathering.
 */
import {
	trackKey,
	type IncomingTrack,
	type LocalTrack,
	type ParticipantId,
	type Rid,
	type Sdp,
	type TrackKind,
	type TrackName,
	type TrackRef
} from './protocol.ts';

export const peerConnection = (iceServers: RTCIceServer[]): RTCPeerConnection =>
	new RTCPeerConnection({ iceServers, bundlePolicy: 'max-bundle' });

/**
 * Camera simulcast: quarter, half and full resolution, named by the core's `Rid`s. Each
 * subscriber picks a layer per track (`ClientMsg::SetLayer`) to match its tile size.
 */
export const CAMERA_SIMULCAST: readonly (RTCRtpEncodingParameters & { rid: Rid })[] = [
	{ rid: 'low', scaleResolutionDownBy: 4 },
	{ rid: 'mid', scaleResolutionDownBy: 2 },
	{ rid: 'high', scaleResolutionDownBy: 1 }
];

/** The local track to publish per kind; `null` publishes nothing of that kind. */
export type Outgoing = Readonly<Record<TrackKind, MediaStreamTrack | null>>;
export const NO_OUTGOING: Outgoing = { audio: null, video: null, screen: null };

const TRACK_KINDS: readonly TrackKind[] = ['audio', 'video', 'screen'];
const NAME_BASE: Record<TrackKind, string> = { audio: 'mic', video: 'cam', screen: 'screen' };

/**
 * Pure: the SFU track name for the `generation`-th track published in this session. The
 * room rejects a name already in use, and a kind turned off stays published until it is
 * replaced (the protocol has no unpublish), so every publish needs a new name.
 */
export const trackName = (kind: TrackKind, generation: number): TrackName =>
	`${NAME_BASE[kind]}-${generation}`;

export interface PublishPlan {
	/** Published kinds no longer wanted: stop their transceivers. */
	stop: TrackKind[];
	/** Published kinds with a new track: swap it onto the sender, no negotiation needed. */
	replace: TrackKind[];
	/** Wanted kinds not yet published: add transceivers and send a publish offer. */
	add: TrackKind[];
}

/**
 * Pure: the changes that make `published` match `desired`. A kind whose `failed` track is
 * still the desired one is not retried, so a rejected publish does not loop; turning the
 * device off and on again produces a new track, which is tried.
 */
export function planPublish<T>(
	published: ReadonlyMap<TrackKind, T>,
	desired: Readonly<Record<TrackKind, T | null>>,
	failed: ReadonlyMap<TrackKind, T>
): PublishPlan {
	const plan: PublishPlan = { stop: [], replace: [], add: [] };
	for (const kind of TRACK_KINDS) {
		const want = desired[kind];
		const have = published.get(kind);
		if (want === null) {
			if (have !== undefined) plan.stop.push(kind);
		} else if (have !== undefined) {
			if (have !== want) plan.replace.push(kind);
		} else if (failed.get(kind) !== want) {
			plan.add.push(kind);
		}
	}
	return plan;
}

/** Why a publish offer did not go through. */
export type PublishFailure = 'rejected' | 'timeout' | 'not-sent' | 'local';

/** How long to wait for `PublishAnswer` before giving up on an offer. */
const PUBLISH_TIMEOUT_MS = 20_000;

class PublishError extends Error {
	constructor(readonly reason: PublishFailure) {
		super(reason);
	}
}

interface Published {
	transceiver: RTCRtpTransceiver;
	track: MediaStreamTrack;
}

/**
 * The publish session. `sync` states which local tracks should be published; the publisher
 * runs one negotiation at a time (the room rejects a second one) until the published set
 * matches. Turning a kind off stops its transceiver; turning it on publishes a new track,
 * because the SFU garbage-collects a track that sends nothing for 30 seconds.
 */
export class Publisher {
	readonly #pc: RTCPeerConnection;
	/** Sends the offer; returns `false` if the socket is down. */
	readonly #sendOffer: (sdp: Sdp, tracks: LocalTrack[]) => boolean;
	readonly #onFailed: (kinds: TrackKind[], reason: PublishFailure) => void;
	readonly #published = new Map<TrackKind, Published>();
	readonly #failed = new Map<TrackKind, MediaStreamTrack>();
	#desired: Outgoing = NO_OUTGOING;
	#generation = 0;
	#running = false;
	#closed = false;
	#pendingAnswer: { resolve: (sdp: Sdp) => void; reject: (e: PublishError) => void } | null = null;

	constructor(
		pc: RTCPeerConnection,
		sendOffer: (sdp: Sdp, tracks: LocalTrack[]) => boolean,
		onFailed: (kinds: TrackKind[], reason: PublishFailure) => void
	) {
		this.#pc = pc;
		this.#sendOffer = sendOffer;
		this.#onFailed = onFailed;
	}

	sync(desired: Outgoing): void {
		this.#desired = desired;
		void this.#run();
	}

	/** Tries the kinds that failed before again, e.g. after the socket reconnected. */
	retryFailed(): void {
		this.#failed.clear();
		void this.#run();
	}

	/** The room's answer to the offer in flight; ignored if none is (it timed out). */
	answer(sdp: Sdp): void {
		this.#pendingAnswer?.resolve(sdp);
		this.#pendingAnswer = null;
	}

	/** The room rejected the offer in flight. */
	rejected(): void {
		this.#pendingAnswer?.reject(new PublishError('rejected'));
		this.#pendingAnswer = null;
	}

	close(): void {
		this.#closed = true;
		this.#pendingAnswer?.reject(new PublishError('local'));
		this.#pendingAnswer = null;
		this.#pc.close();
	}

	async #run(): Promise<void> {
		if (this.#running) return;
		this.#running = true;
		try {
			while (!this.#closed) {
				const published = new Map([...this.#published].map(([kind, p]) => [kind, p.track]));
				const plan = planPublish(published, this.#desired, this.#failed);
				if (plan.stop.length + plan.replace.length + plan.add.length === 0) break;
				for (const kind of plan.stop) this.#stop(kind);
				for (const kind of plan.replace) await this.#replace(kind);
				if (plan.add.length > 0) await this.#publish(plan.add);
			}
		} finally {
			this.#running = false;
		}
	}

	#stop(kind: TrackKind): void {
		const published = this.#published.get(kind);
		this.#published.delete(kind);
		try {
			published?.transceiver.stop();
		} catch {
			// Already stopped, or the connection is closed.
		}
	}

	async #replace(kind: TrackKind): Promise<void> {
		const published = this.#published.get(kind);
		const track = this.#desired[kind];
		if (!published || !track) return;
		try {
			await published.transceiver.sender.replaceTrack(track);
			published.track = track;
		} catch {
			// The browser refused the swap: publish the track anew on the next round.
			this.#stop(kind);
		}
	}

	async #publish(kinds: TrackKind[]): Promise<void> {
		const added = kinds.flatMap((kind) => {
			const track = this.#desired[kind];
			if (!track) return [];
			const transceiver = this.#pc.addTransceiver(track, {
				direction: 'sendonly',
				...(kind === 'video' ? { sendEncodings: CAMERA_SIMULCAST.map((e) => ({ ...e })) } : {})
			});
			if (kind !== 'audio') preferVideoCodecs(transceiver);
			return [{ kind, track, transceiver }];
		});
		try {
			await this.#pc.setLocalDescription(await this.#pc.createOffer());
			const sdp = this.#pc.localDescription?.sdp;
			if (!sdp) throw new PublishError('local');
			const tracks = added.map(({ kind, transceiver }): LocalTrack => {
				// `mid` is assigned by `setLocalDescription`.
				if (transceiver.mid === null) throw new PublishError('local');
				return { mid: transceiver.mid, kind, name: trackName(kind, ++this.#generation) };
			});
			const answer = await this.#exchange(sdp, tracks);
			await this.#pc.setRemoteDescription({ type: 'answer', sdp: answer });
			for (const { kind, track, transceiver } of added) {
				this.#published.set(kind, { transceiver, track });
				this.#failed.delete(kind);
			}
		} catch (e) {
			if (this.#closed) return;
			if (this.#pc.signalingState === 'have-local-offer') {
				await this.#pc.setLocalDescription({ type: 'rollback' }).catch(() => {});
			}
			for (const { kind, track, transceiver } of added) {
				this.#failed.set(kind, track);
				try {
					transceiver.stop();
				} catch {
					// Never negotiated: nothing to stop.
				}
			}
			this.#onFailed(
				added.map((a) => a.kind),
				e instanceof PublishError ? e.reason : 'local'
			);
		}
	}

	/** Sends the offer and waits for the room's answer, its rejection, or the timeout. */
	#exchange(sdp: Sdp, tracks: LocalTrack[]): Promise<Sdp> {
		return new Promise<Sdp>((resolve, reject) => {
			if (!this.#sendOffer(sdp, tracks)) {
				reject(new PublishError('not-sent'));
				return;
			}
			const timer = setTimeout(() => {
				this.#pendingAnswer = null;
				reject(new PublishError('timeout'));
			}, PUBLISH_TIMEOUT_MS);
			this.#pendingAnswer = {
				resolve: (answer) => {
					clearTimeout(timer);
					resolve(answer);
				},
				reject: (e) => {
					clearTimeout(timer);
					reject(e);
				}
			};
		});
	}
}

/** A remote track and the transceiver (`mid`) that carries it. */
export interface RemoteTrack {
	ref: TrackRef;
	mid: string;
	track: MediaStreamTrack;
}

/** Remote tracks by `trackKey`. */
export type RemoteTracks = ReadonlyMap<string, RemoteTrack>;
export const NO_REMOTE_TRACKS: RemoteTracks = new Map();

/**
 * The subscribe session: applies the room's offers one at a time and maps each transceiver
 * to the participant track the offer names.
 */
export class Subscriber {
	readonly #pc: RTCPeerConnection;
	readonly #sendAnswer: (sdp: Sdp) => void;
	readonly #onTracks: (tracks: RemoteTracks, delivered: TrackRef[]) => void;
	#tracks = new Map<string, RemoteTrack>();
	#queue: Promise<void> = Promise.resolve();

	constructor(
		pc: RTCPeerConnection,
		sendAnswer: (sdp: Sdp) => void,
		onTracks: (tracks: RemoteTracks, delivered: TrackRef[]) => void
	) {
		this.#pc = pc;
		this.#sendAnswer = sendAnswer;
		this.#onTracks = onTracks;
	}

	offer(sdp: Sdp, incoming: IncomingTrack[]): void {
		this.#queue = this.#queue
			.then(() => this.#apply(sdp, incoming))
			.catch((e: unknown) => {
				// The room times the negotiation out and offers the tracks again.
				console.warn('subscribe offer failed', e);
			});
	}

	/** Drops the tracks of participants no longer in the room. */
	retain(owners: readonly ParticipantId[]): void {
		const kept = new Map([...this.#tracks].filter(([, t]) => owners.includes(t.ref.owner)));
		if (kept.size === this.#tracks.size) return;
		this.#tracks = kept;
		this.#onTracks(kept, []);
	}

	close(): void {
		this.#pc.close();
	}

	async #apply(sdp: Sdp, incoming: IncomingTrack[]): Promise<void> {
		await this.#pc.setRemoteDescription({ type: 'offer', sdp });
		const byMid = new Map(this.#pc.getTransceivers().map((t) => [t.mid, t]));
		const tracks = new Map(this.#tracks);
		const delivered: TrackRef[] = [];
		for (const { mid, track: ref } of incoming) {
			const transceiver = byMid.get(mid);
			if (!transceiver) continue;
			// The SFU may reuse a transceiver for another track: one entry per mid.
			for (const [key, t] of tracks) if (t.mid === mid) tracks.delete(key);
			tracks.set(trackKey(ref), { ref, mid, track: transceiver.receiver.track });
			delivered.push(ref);
		}
		await this.#pc.setLocalDescription(await this.#pc.createAnswer());
		const answer = this.#pc.localDescription?.sdp;
		if (!answer) throw new Error('subscribe answer has no local description');
		this.#sendAnswer(answer);
		this.#tracks = tracks;
		this.#onTracks(tracks, delivered);
	}
}

/**
 * Pure: orders video codecs H.264 first, then VP8, and keeps the helper codecs
 * (RTX, RED, FEC). VP9 and AV1 are dropped: H.264 is hardware-encoded everywhere including
 * iOS, and AV1 is not smooth on iPhones (docs/media/README.md, D12).
 */
export function orderVideoCodecs<C extends { mimeType: string }>(codecs: readonly C[]): C[] {
	const type = (c: C) => c.mimeType.toLowerCase();
	const h264 = codecs.filter((c) => type(c) === 'video/h264');
	const vp8 = codecs.filter((c) => type(c) === 'video/vp8');
	const helpers = codecs.filter((c) =>
		['video/rtx', 'video/red', 'video/ulpfec', 'video/flexfec-03'].includes(type(c))
	);
	return [...h264, ...vp8, ...helpers];
}

/**
 * Applies `orderVideoCodecs` where `setCodecPreferences` exists (Baseline 2024). Codecs come
 * from `RTCRtpReceiver.getCapabilities`, which is what browsers validate against.
 * Leaves the browser default in place if the API is missing or rejects the list.
 */
function preferVideoCodecs(transceiver: RTCRtpTransceiver): void {
	if (typeof transceiver.setCodecPreferences !== 'function') return;
	const codecs = RTCRtpReceiver.getCapabilities('video')?.codecs;
	if (!codecs) return;
	const ordered = orderVideoCodecs(codecs);
	if (!ordered.some((c) => ['video/h264', 'video/vp8'].includes(c.mimeType.toLowerCase()))) return;
	try {
		transceiver.setCodecPreferences(ordered);
	} catch {
		// InvalidAccessError / InvalidModificationError: keep the browser's order.
	}
}
