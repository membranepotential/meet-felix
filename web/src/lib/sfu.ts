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
 *
 * A peer connection whose `connectionState` becomes `failed` does not recover by itself: both
 * classes report it once, and the session asks the room for a new SFU session (D33,
 * `ClientMsg::ResetSession`), after which it replaces the whole class.
 */
import {
	trackKey,
	type ClientMsg,
	type IncomingTrack,
	type LocalTrack,
	type Rid,
	type Sdp,
	type TrackKind,
	type TrackName,
	type TrackRef
} from './protocol.ts';

export const peerConnection = (iceServers: RTCIceServer[]): RTCPeerConnection =>
	new RTCPeerConnection({ iceServers, bundlePolicy: 'max-bundle' });

/**
 * Gives `pc` new ICE servers, such as fresh TURN credentials. A browser uses them from the
 * connection's next ICE gathering on, not for the candidates it already has. A closed
 * connection keeps what it had.
 */
function setIceServers(pc: RTCPeerConnection, iceServers: RTCIceServer[]): void {
	if (pc.signalingState === 'closed') return;
	pc.setConfiguration({ ...pc.getConfiguration(), iceServers });
}

/** Calls `onFailed` once, when `pc.connectionState` first becomes `failed`. */
function watchFailure(pc: RTCPeerConnection, onFailed: () => void): void {
	const check = () => {
		if (pc.connectionState !== 'failed') return;
		pc.removeEventListener('connectionstatechange', check);
		onFailed();
	};
	pc.addEventListener('connectionstatechange', check);
}

/**
 * Camera simulcast: quarter, half and full resolution, named by the core's `Rid`s. Each
 * subscriber picks a layer per track (`ClientMsg::SetLayer`) to match its tile size.
 */
const CAMERA_SIMULCAST: readonly (RTCRtpEncodingParameters & { rid: Rid })[] = [
	{ rid: 'q', scaleResolutionDownBy: 4 },
	{ rid: 'h', scaleResolutionDownBy: 2 },
	{ rid: 'f', scaleResolutionDownBy: 1 }
];

/** The local track to publish per kind; `null` publishes nothing of that kind. */
export type Outgoing = Readonly<Record<TrackKind, MediaStreamTrack | null>>;
export const NO_OUTGOING: Outgoing = { audio: null, video: null, screen: null };

const TRACK_KINDS: readonly TrackKind[] = ['audio', 'video', 'screen'];
const NAME_BASE: Record<TrackKind, string> = { audio: 'mic', video: 'cam', screen: 'screen' };

/**
 * Pure: the SFU track name for the `generation`-th track published in this session. The
 * room rejects a name already in use, so every publish takes a new one.
 */
export const trackName = (kind: TrackKind, generation: number): TrackName =>
	`${NAME_BASE[kind]}-${generation}`;

export interface PublishPlan {
	/** Published kinds no longer wanted: send `Unpublish` and stop their transceivers. */
	unpublish: TrackKind[];
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
	const plan: PublishPlan = { unpublish: [], replace: [], add: [] };
	for (const kind of TRACK_KINDS) {
		const want = desired[kind];
		const have = published.get(kind);
		if (want === null) {
			if (have !== undefined) plan.unpublish.push(kind);
		} else if (have !== undefined) {
			if (have !== want) plan.replace.push(kind);
		} else if (failed.get(kind) !== want) {
			plan.add.push(kind);
		}
	}
	return plan;
}

/**
 * Why a publish offer, or some of its tracks, did not go through: `refused` names the tracks
 * the SFU did not publish from an offer whose other tracks it published.
 */
export type PublishFailure = 'rejected' | 'refused' | 'timeout' | 'not-sent' | 'local';

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

/** Stops the transceiver of a track the room did not publish. */
function stopTransceiver(transceiver: RTCRtpTransceiver): void {
	try {
		transceiver.stop();
	} catch {
		// Never negotiated, or the connection is closed: nothing to stop.
	}
}

/** The room's answer to a publish offer (`ServerMsg::PublishAnswer`). */
interface Answer {
	sdp: Sdp;
	/** The offered kinds the SFU did not publish. */
	rejected: readonly TrackKind[];
}

/**
 * The publish session. `sync` states which local tracks should be published; the publisher
 * runs one negotiation at a time (the room rejects a second one) until the published set
 * matches. Turning a kind off sends `Unpublish` and stops its transceiver, so the track
 * leaves every subscriber at once; turning it on publishes a new track.
 */
export class Publisher {
	readonly #pc: RTCPeerConnection;
	/** Sends a message to the room; returns `false` if the socket is down. */
	readonly #send: (msg: ClientMsg) => boolean;
	readonly #onFailed: (kinds: TrackKind[], reason: PublishFailure) => void;
	readonly #published = new Map<TrackKind, Published>();
	readonly #failed = new Map<TrackKind, MediaStreamTrack>();
	/** Kinds whose `Unpublish` could not go out; sent again by `retryFailed`. */
	readonly #unsentUnpublish = new Set<TrackKind>();
	#desired: Outgoing = NO_OUTGOING;
	#generation = 0;
	#running = false;
	#closed = false;
	#pendingAnswer: { resolve: (answer: Answer) => void; reject: (e: PublishError) => void } | null =
		null;

	/**
	 * `onFailed` hears about publish offers that did not go through; `onConnectionFailed` hears
	 * once that the peer connection failed.
	 */
	constructor(
		pc: RTCPeerConnection,
		send: (msg: ClientMsg) => boolean,
		onFailed: (kinds: TrackKind[], reason: PublishFailure) => void,
		onConnectionFailed: () => void
	) {
		this.#pc = pc;
		this.#send = send;
		this.#onFailed = onFailed;
		watchFailure(pc, onConnectionFailed);
	}

	sync(desired: Outgoing): void {
		this.#desired = desired;
		void this.#run();
	}

	/**
	 * Tries the kinds that failed before again, e.g. after the socket reconnected, and sends
	 * the `Unpublish` messages that could not go out while it was down.
	 */
	retryFailed(): void {
		this.#failed.clear();
		for (const kind of this.#unsentUnpublish) this.#sendUnpublish(kind);
		void this.#run();
	}

	/**
	 * The room's answer to the offer in flight; ignored if none is (it timed out). The SFU did
	 * not publish the `rejected` kinds: they count as failed, like a rejected offer.
	 */
	answer(sdp: Sdp, rejected: readonly TrackKind[] = []): void {
		this.#pendingAnswer?.resolve({ sdp, rejected });
		this.#pendingAnswer = null;
	}

	/** The room rejected the offer in flight. */
	rejected(): void {
		this.#pendingAnswer?.reject(new PublishError('rejected'));
		this.#pendingAnswer = null;
	}

	setIceServers(iceServers: RTCIceServer[]): void {
		setIceServers(this.#pc, iceServers);
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
				if (plan.unpublish.length + plan.replace.length + plan.add.length === 0) break;
				for (const kind of plan.unpublish) this.#unpublish(kind);
				for (const kind of plan.replace) await this.#replace(kind);
				if (plan.add.length > 0) await this.#publish(plan.add);
			}
		} finally {
			this.#running = false;
		}
	}

	/** Takes `kind` off the room and the SFU; its next track is published anew. */
	#unpublish(kind: TrackKind): void {
		const published = this.#published.get(kind);
		this.#published.delete(kind);
		this.#sendUnpublish(kind);
		try {
			published?.transceiver.stop();
		} catch {
			// Already stopped, or the connection is closed.
		}
	}

	#sendUnpublish(kind: TrackKind): void {
		if (this.#send({ type: 'unpublish', kind })) this.#unsentUnpublish.delete(kind);
		else this.#unsentUnpublish.add(kind);
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
			this.#unpublish(kind);
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
			await this.#pc.setRemoteDescription({ type: 'answer', sdp: answer.sdp });
			for (const { kind, track, transceiver } of added) {
				if (answer.rejected.includes(kind)) {
					this.#failed.set(kind, track);
					stopTransceiver(transceiver);
					continue;
				}
				this.#published.set(kind, { transceiver, track });
				this.#failed.delete(kind);
				// The new track replaced the old one in the room; its `Unpublish` is moot.
				this.#unsentUnpublish.delete(kind);
			}
			const refused = added.map((a) => a.kind).filter((kind) => answer.rejected.includes(kind));
			if (refused.length > 0) this.#onFailed(refused, 'refused');
		} catch (e) {
			if (this.#closed) return;
			if (this.#pc.signalingState === 'have-local-offer') {
				await this.#pc.setLocalDescription({ type: 'rollback' }).catch(() => {});
			}
			for (const { kind, track, transceiver } of added) {
				this.#failed.set(kind, track);
				stopTransceiver(transceiver);
			}
			this.#onFailed(
				added.map((a) => a.kind),
				e instanceof PublishError ? e.reason : 'local'
			);
		}
	}

	/** Sends the offer and waits for the room's answer, its rejection, or the timeout. */
	#exchange(sdp: Sdp, tracks: LocalTrack[]): Promise<Answer> {
		return new Promise<Answer>((resolve, reject) => {
			if (!this.#send({ type: 'publishOffer', sdp, tracks })) {
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

	/** `onConnectionFailed` hears once that the peer connection failed. */
	constructor(
		pc: RTCPeerConnection,
		sendAnswer: (sdp: Sdp) => void,
		onTracks: (tracks: RemoteTracks, delivered: TrackRef[]) => void,
		onConnectionFailed: () => void
	) {
		this.#pc = pc;
		this.#sendAnswer = sendAnswer;
		this.#onTracks = onTracks;
		watchFailure(pc, onConnectionFailed);
	}

	offer(sdp: Sdp, incoming: IncomingTrack[]): void {
		this.#queue = this.#queue
			.then(() => this.#apply(sdp, incoming))
			.catch((e: unknown) => {
				// The room times the negotiation out and offers the tracks again.
				console.warn('subscribe offer failed', e);
			});
	}

	/**
	 * Drops the tracks the room no longer lists as `published`: participants who left and
	 * kinds unpublished. The room's `State` comes before the SFU closes the tracks.
	 */
	retain(published: readonly TrackRef[]): void {
		const keys = new Set(published.map(trackKey));
		const kept = new Map([...this.#tracks].filter(([key]) => keys.has(key)));
		if (kept.size === this.#tracks.size) return;
		this.#tracks = kept;
		this.#onTracks(kept, []);
	}

	setIceServers(iceServers: RTCIceServer[]): void {
		setIceServers(this.#pc, iceServers);
	}

	close(): void {
		this.#pc.close();
	}

	async #apply(sdp: Sdp, incoming: IncomingTrack[]): Promise<void> {
		await this.#pc.setRemoteDescription({ type: 'offer', sdp });
		const byMid = new Map(this.#pc.getTransceivers().map((t) => [t.mid, t]));
		// An offer may carry no track the room could pull (its publisher is not connected
		// yet); the SFU still waits for the answer.
		const arrived = incoming.flatMap(({ mid, track: ref }): RemoteTrack[] => {
			const transceiver = byMid.get(mid);
			return transceiver ? [{ ref, mid, track: transceiver.receiver.track }] : [];
		});
		await this.#pc.setLocalDescription(await this.#pc.createAnswer());
		const answer = this.#pc.localDescription?.sdp;
		if (!answer) throw new Error('subscribe answer has no local description');
		this.#sendAnswer(answer);
		// Applied to the tracks as they are now: `retain` may have dropped some meanwhile.
		const tracks = new Map(this.#tracks);
		for (const remote of arrived) {
			// The SFU may reuse a transceiver for another track: one entry per mid.
			for (const [key, t] of tracks) if (t.mid === remote.mid) tracks.delete(key);
			tracks.set(trackKey(remote.ref), remote);
		}
		this.#tracks = tracks;
		this.#onTracks(
			tracks,
			arrived.map((t) => t.ref)
		);
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
