/**
 * One stay in a meeting: the signaling socket, the client state from `client-state.ts`, the
 * two SFU peer connections, notices and the screen wake lock. Created when the user presses
 * Join, disposed when the stay is over.
 */
import { getIceServers, getMeeting } from './api.ts';
import {
	INITIAL,
	reduce,
	type ClientState,
	type Command,
	type SessionEnd
} from './client-state.ts';
import { LayerControl } from './layers.ts';
import type { LocalMedia } from './local-media.svelte.ts';
import { holdWakeLock } from './media/wakelock.ts';
import type {
	ClientMsg,
	DisplayName,
	HostToken,
	MeetingId,
	ParticipantId,
	ParticipantView,
	ServerMsg,
	TrackKind
} from './protocol.ts';
import {
	NO_REMOTE_TRACKS,
	peerConnection,
	Publisher,
	Subscriber,
	type PublishFailure,
	type RemoteTracks
} from './sfu.ts';
import { SignalingClient, type ConnectionStatus } from './signaling.ts';
import { forgetHostToken } from './storage.ts';
import { Toasts } from './toasts.svelte.ts';

export interface JoinRequest {
	name: DisplayName;
	/** Whether the user accepted recording on the pre-join screen. */
	consent: boolean;
	hostToken: HostToken | null;
}

/** Used when `/api/ice` fails: direct and STUN-only paths still work on most networks. */
const FALLBACK_ICE: RTCIceServer[] = [{ urls: 'stun:stun.cloudflare.com:3478' }];

const KIND_LABEL: Record<TrackKind, string> = {
	audio: 'microphone',
	video: 'camera',
	screen: 'screen'
};

interface MediaLink {
	publisher: Publisher;
	subscriber: Subscriber;
}

/** Participants who get a tile: admitted, not waiting outside. */
const isInRoom = (p: ParticipantView) => p.presence === 'joined' || p.presence === 'connecting';

export class Session {
	readonly toasts = new Toasts();
	connection: ConnectionStatus = $state.raw({ kind: 'connecting', attempt: 1 });
	state: ClientState = $state.raw(INITIAL);
	/** Received tracks by `trackKey`. */
	remote: RemoteTracks = $state.raw(NO_REMOTE_TRACKS);
	/** Sends `SetLayer` as tiles change size. */
	readonly layers: LayerControl;

	stage = $derived(this.state.stage);
	end: SessionEnd | null = $derived(this.stage.kind === 'over' ? this.stage.end : null);
	room = $derived(this.stage.kind === 'in' ? this.stage.room : null);
	you: ParticipantId | null = $derived(this.stage.kind === 'in' ? this.stage.you : null);
	me = $derived(this.room?.participants.find((p) => p.id === this.you) ?? null);
	isHost = $derived(this.me?.role === 'host');
	others = $derived(this.room?.participants.filter((p) => p.id !== this.you && isInRoom(p)) ?? []);
	/** Guests knocking on the locked room; only hosts can let them in. */
	knocking = $derived(
		this.isHost ? (this.room?.participants.filter((p) => p.presence === 'knocking') ?? []) : []
	);

	readonly #meeting: MeetingId;
	readonly #media: LocalMedia;
	readonly #signaling: SignalingClient;
	readonly #ice: Promise<RTCIceServer[]>;
	#join: JoinRequest;
	#link: Promise<MediaLink> | null = null;
	#releaseWakeLock: () => void = () => {};
	#stopSync: () => void = () => {};
	#disposed = false;
	#checkingMeeting = false;

	constructor(meeting: MeetingId, join: JoinRequest, media: LocalMedia) {
		this.#meeting = meeting;
		this.#join = join;
		this.#media = media;
		this.#signaling = new SignalingClient(meeting, {
			onOpen: () => this.#sendJoin(),
			onMessage: (msg) => this.receive(msg),
			onStatus: (status) => this.#onStatus(status)
		});
		this.layers = new LayerControl((track, rid) =>
			this.#signaling.send({ type: 'setLayer', track, rid })
		);
		// Fetched once, while the socket connects; TURN credentials last hours.
		this.#ice = getIceServers().then((result) => {
			if (result.ok) return result.value;
			console.warn('ICE servers unavailable, using STUN only', result.error);
			return FALLBACK_ICE;
		});
	}

	start(): void {
		this.#releaseWakeLock = holdWakeLock();
		this.#signaling.connect();
		// Publishing follows the local tracks: device switches, camera on/off, screen share.
		this.#stopSync = $effect.root(() => {
			$effect(() => {
				const outgoing = this.#media.outgoing;
				void this.#link?.then(({ publisher }) => publisher.sync(outgoing));
			});
		});
	}

	/** Sends a command; tells the user when it cannot go out. */
	send(msg: ClientMsg): void {
		if (!this.#signaling.send(msg)) {
			this.toasts.show('Not connected to the meeting. Try again in a moment.', 'alert');
		}
	}

	acceptRecording(): void {
		this.#join = { ...this.#join, consent: true };
		this.state = { ...this.state, consentAsked: false };
		this.send({ type: 'setConsent', consent: true });
	}

	leave(): void {
		this.#signaling.send({ type: 'leave' });
		this.#finish({ kind: 'left' });
	}

	/** Handles one server message. */
	receive(msg: ServerMsg): void {
		const [next, commands] = reduce(this.state, msg);
		this.state = next;
		for (const command of commands) this.#run(command);
		if (next.stage.kind === 'in') {
			const owners = next.stage.room.participants.map((p) => p.id);
			void this.#link?.then(({ subscriber }) => subscriber.retain(owners));
		}
	}

	/** Closes the socket and the peer connections. Local media stays with its owner. */
	dispose(): void {
		if (this.#disposed) return;
		this.#disposed = true;
		this.#stopSync();
		this.#signaling.close();
		this.#closeLink();
		this.layers.dispose();
		this.#releaseWakeLock();
	}

	#run(command: Command): void {
		switch (command.kind) {
			case 'toast':
				this.toasts.show(command.text, command.tone);
				return;
			case 'warn':
				console.warn(command.text);
				return;
			case 'welcomed':
				if (command.fresh || this.#link === null) this.#openLink();
				else void this.#link.then(({ publisher }) => publisher.retryFailed());
				return;
			case 'publish-answer':
				void this.#link?.then(({ publisher }) => publisher.answer(command.sdp));
				return;
			case 'publish-rejected':
				void this.#link?.then(({ publisher }) => publisher.rejected());
				return;
			case 'subscribe-offer':
				void this.#link?.then(({ subscriber }) => subscriber.offer(command.sdp, command.tracks));
				return;
			case 'mute-mic':
				this.#media.mute();
				return;
			case 'rejoin-as-guest':
				forgetHostToken(this.#meeting);
				this.#join = { ...this.#join, hostToken: null };
				this.#sendJoin();
				return;
			case 'teardown':
				this.#over();
				return;
		}
	}

	/** New SFU sessions on the room side: new peer connections here, everything republished. */
	#openLink(): void {
		this.#closeLink();
		const link: Promise<MediaLink> = this.#ice.then((iceServers) => {
			const publisher = new Publisher(
				peerConnection(iceServers),
				(msg) => this.#signaling.send(msg),
				(kinds, reason) => this.#onPublishFailed(kinds, reason)
			);
			const subscriber = new Subscriber(
				peerConnection(iceServers),
				(sdp) => this.#signaling.send({ type: 'subscribeAnswer', sdp }),
				(tracks, delivered) => {
					// A late answer from a replaced link must not overwrite the current tracks.
					if (this.#link !== link) return;
					this.remote = tracks;
					this.layers.received(delivered);
				}
			);
			publisher.sync(this.#media.outgoing);
			return { publisher, subscriber };
		});
		this.#link = link;
	}

	#closeLink(): void {
		void this.#link?.then(({ publisher, subscriber }) => {
			publisher.close();
			subscriber.close();
		});
		this.#link = null;
		this.remote = NO_REMOTE_TRACKS;
	}

	#onPublishFailed(kinds: TrackKind[], reason: PublishFailure): void {
		// The room's rejection already produced a notice from its error code.
		if (reason === 'rejected') return;
		const what = kinds.map((k) => KIND_LABEL[k]).join(' and ');
		this.toasts.show(
			reason === 'not-sent'
				? `Your ${what} is shared once the connection is back.`
				: `Your ${what} could not be shared. Turn it off and on to try again.`,
			'alert'
		);
	}

	#sendJoin(): void {
		const { name, consent, hostToken } = this.#join;
		const { resume } = this.state;
		this.#signaling.send({
			type: 'join',
			name,
			consent,
			...(hostToken !== null ? { hostToken } : {}),
			...(resume !== null ? { resume } : {})
		});
	}

	#onStatus(status: ConnectionStatus): void {
		this.connection = status;
		if (status.kind === 'waiting') void this.#checkMeeting();
		if (status.kind === 'closed' && status.code !== 1000) {
			this.#finish({
				kind: 'refused',
				reason: status.reason || 'The meeting closed the connection.'
			});
		}
	}

	/**
	 * The room answers a socket for an ended or unknown meeting with HTTP 410 or 404, which a
	 * browser reports only as a lost connection. So before each retry, ask whether the meeting
	 * still runs, and stop retrying if it does not.
	 */
	async #checkMeeting(): Promise<void> {
		if (this.#checkingMeeting) return;
		this.#checkingMeeting = true;
		const result = await getMeeting(this.#meeting);
		this.#checkingMeeting = false;
		if (result.ok && result.value.kind !== 'running') {
			this.#finish({ kind: 'ended', transcript: null });
		}
	}

	#finish(end: SessionEnd): void {
		if (this.end !== null) return;
		this.state = { ...this.state, stage: { kind: 'over', end }, consentAsked: false };
		this.#over();
	}

	/** Releases camera and mic as soon as the stay is over, not only when the page closes. */
	#over(): void {
		this.dispose();
		this.#media.dispose();
	}
}
