/**
 * One stay in a meeting: the signaling socket, the client state from `client-state.ts`, the
 * two SFU peer connections, the others' voices, notices and the screen wake lock. Created
 * inside the Join tap, disposed when the stay is over.
 */
import { untrack } from 'svelte';
import { on } from 'svelte/events';
import { getIceServers, getMeeting } from './api.ts';
import {
	awaitingConsent,
	INITIAL,
	reduce,
	syncMute,
	type ClientState,
	type Command,
	type SessionEnd
} from './client-state.ts';
import { GraceNotice } from './grace.ts';
import { LayerControl } from './layers.ts';
import type { LocalMedia } from './local-media.svelte.ts';
import { RemoteAudio } from './media/remote-audio.svelte.ts';
import { holdWakeLock } from './media/wakelock.ts';
import type {
	ClientMsg,
	DisplayName,
	MeetingId,
	ParticipantId,
	ParticipantView,
	ServerMsg,
	SessionEpoch,
	SessionEpochs,
	SessionKind,
	TrackKind
} from './protocol.ts';
import { SessionResets } from './resets.ts';
import {
	NO_REMOTE_TRACKS,
	peerConnection,
	Publisher,
	Subscriber,
	type PublishFailure,
	type RemoteTracks
} from './sfu.ts';
import { SignalingClient, type ConnectionStatus } from './signaling.ts';
import { Toasts } from './toasts.svelte.ts';

export interface JoinRequest {
	name: DisplayName;
	/** Whether the user accepted recording on the pre-join screen. */
	consent: boolean;
}

/** Used when `/api/ice` fails: direct and STUN-only paths still work on most networks. */
const FALLBACK_ICE: RTCIceServer[] = [{ urls: 'stun:stun.cloudflare.com:3478' }];

/** How long a failed subscribe may take to recover before the user hears about it. */
const SUBSCRIBE_GRACE_MS = 10_000;

const KIND_LABEL: Record<TrackKind, string> = {
	audio: 'microphone',
	video: 'camera',
	screen: 'screen'
};

/** The quiet notice while the room replaces a session whose connection failed. */
const RECONNECTING: Record<SessionKind, string> = {
	publish: 'Reconnecting your camera and microphone…',
	subscribe: 'Reconnecting to the others…'
};

/** Shown once each time the mic stalls (`LocalMedia.micStalled`); the button then restarts it. */
const MIC_STALLED = 'Microphone was interrupted. Tap the microphone button to turn it on again.';

/** Participants who get a tile: admitted, not waiting outside. */
const isInRoom = (p: ParticipantView) => p.presence === 'joined' || p.presence === 'connecting';

export class Session {
	readonly toasts = new Toasts();
	/** The others' voices and the `AudioContext` for the level meters. */
	readonly audio: RemoteAudio;
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
	/** Others the running recording leaves out until they accept it; only hosts see them. */
	awaitingConsent: ParticipantView[] = $derived(
		this.isHost && this.room && this.you !== null ? awaitingConsent(this.room, this.you) : []
	);

	readonly #meeting: MeetingId;
	readonly #media: LocalMedia;
	readonly #signaling: SignalingClient;
	readonly #ice: Promise<RTCIceServer[]>;
	#join: JoinRequest;
	/** The two SFU peer connections; `null` until the first welcome. */
	#publisher: Promise<Publisher> | null = null;
	#subscriber: Promise<Subscriber> | null = null;
	/** The epochs of the room's sessions the two connections belong to. */
	#epochs: SessionEpochs = { publish: 0, subscribe: 0 };
	/** The last mic switch was not reported, because the socket was down. */
	#unsentMute = false;
	readonly #resets = new SessionResets((kind, epoch) =>
		this.#signaling.send({ type: 'resetSession', kind, epoch })
	);
	/** The first pull often races the publisher's connection and succeeds on the room's retry. */
	readonly #subscribeNotice = new GraceNotice(SUBSCRIBE_GRACE_MS, (text) =>
		this.toasts.show(text, 'alert')
	);
	#releaseWakeLock: () => void = () => {};
	/** Stops what `start` runs while the session lives: publishing, the mic and mute effects, the `pagehide` leave. */
	#stopEffects: () => void = () => {};
	#disposed = false;
	#checkingMeeting = false;

	/** Call inside the Join tap, before any `await`: it unlocks sound for the stay. */
	constructor(meeting: MeetingId, join: JoinRequest, media: LocalMedia) {
		this.audio = new RemoteAudio();
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
		this.#ice = getIceServers(meeting).then((result) => {
			if (result.ok) return result.value;
			console.warn('ICE servers unavailable, using STUN only', result.error);
			return FALLBACK_ICE;
		});
	}

	start(): void {
		this.#releaseWakeLock = holdWakeLock();
		this.#signaling.connect();
		this.#stopEffects = $effect.root(() => {
			// Publishing follows the local tracks: device switches, camera on/off, screen share.
			$effect(() => {
				const outgoing = this.#media.outgoing;
				void this.#publisher?.then((publisher) => publisher.sync(outgoing));
			});
			$effect(() => {
				if (this.#media.micStalled) untrack(() => this.toasts.show(MIC_STALLED, 'alert'));
			});
			// Closing the tab or reloading leaves at once; otherwise the room would keep our
			// place for its grace period (D35) and show us as reconnecting. A page kept in the
			// back/forward cache (`persisted`) may come back, so it does not leave.
			$effect(() =>
				on(window, 'pagehide', (event) => {
					if (!event.persisted) this.#signaling.send({ type: 'leave' });
				})
			);
			// Everyone sees our mic switch. A report that could not go out (the socket was down)
			// is made on the next welcome instead of adopting the room's view (`#welcomed`).
			$effect(() => {
				const muted = !this.#media.micOn;
				untrack(() => {
					if (this.stage.kind !== 'in') return;
					this.#unsentMute = !this.#signaling.send({ type: 'setMuted', muted });
				});
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

	/**
	 * Connects again with a new socket and the same participant. After the owner logged in,
	 * the new socket carries the session cookie, and the room makes this participant a host.
	 */
	reconnect(): void {
		this.#signaling.reconnect();
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
			const published = next.stage.room.participants.flatMap((p) =>
				p.tracks.map((kind) => ({ owner: p.id, kind }))
			);
			void this.#subscriber?.then((subscriber) => subscriber.retain(published));
		}
	}

	/** Closes the socket and the peer connections. Local media stays with its owner. */
	dispose(): void {
		if (this.#disposed) return;
		this.#disposed = true;
		this.#stopEffects();
		this.#signaling.close();
		this.#closeLink();
		this.#resets.cancel();
		this.#subscribeNotice.cancel();
		this.layers.dispose();
		this.audio.dispose();
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
				this.#welcomed(command.fresh, command.epochs);
				return;
			case 'publish-answer':
				void this.#publisher?.then((publisher) => publisher.answer(command.sdp, command.rejected));
				return;
			case 'publish-rejected':
				void this.#publisher?.then((publisher) => publisher.rejected());
				return;
			case 'publish-reset':
				this.#openPublisher(command.epoch);
				return;
			case 'subscribe-offer':
				this.#subscribeNotice.cancel();
				void this.#subscriber?.then((subscriber) => subscriber.offer(command.sdp, command.tracks));
				return;
			case 'subscribe-reset':
				this.#openSubscriber(command.epoch);
				return;
			case 'subscribe-retrying':
				this.#subscribeNotice.start(command.notice);
				return;
			case 'layer-refused':
				this.layers.notSubscribed();
				return;
			case 'mute-mic':
				this.#media.mute();
				return;
			case 'report-muted':
				this.#signaling.send({ type: 'setMuted', muted: true });
				return;
			case 'teardown':
				this.#over();
				return;
		}
	}

	/**
	 * A new participant opens new peer connections. After a reconnect, the old ones stay,
	 * except a connection whose session the room replaced while the socket was down: its reset
	 * never arrived, but the welcome's epochs tell. Either way the mic switch and the room's mute
	 * flag are made to agree.
	 */
	#welcomed(fresh: boolean, epochs: SessionEpochs): void {
		const current = (kind: SessionKind) => !fresh && this.#epochs[kind] === epochs[kind];
		if (this.#publisher === null || !current('publish')) this.#openPublisher(epochs.publish);
		else void this.#publisher.then((publisher) => publisher.retryFailed());
		if (this.#subscriber === null || !current('subscribe')) this.#openSubscriber(epochs.subscribe);
		if (this.#unsentMute) {
			this.#unsentMute = !this.#signaling.send({ type: 'setMuted', muted: !this.#media.micOn });
			return;
		}
		for (const command of syncMute(this.me?.muted ?? false, this.#media.micOn)) this.#run(command);
	}

	/**
	 * The room's publish session of `epoch`: a new peer connection that publishes it all again.
	 */
	#openPublisher(epoch: SessionEpoch): void {
		this.#closePublisher();
		this.#epochs = { ...this.#epochs, publish: epoch };
		const publisher: Promise<Publisher> = this.#ice.then((iceServers) => {
			const created = new Publisher(
				peerConnection(iceServers),
				(msg) => this.#signaling.send(msg),
				(kinds, reason) => this.#onPublishFailed(kinds, reason),
				() => {
					if (this.#publisher === publisher) this.#onConnectionFailed('publish', epoch);
				}
			);
			created.sync(this.#media.outgoing);
			return created;
		});
		this.#publisher = publisher;
	}

	/** The room's subscribe session of `epoch`: a new peer connection that receives nothing yet. */
	#openSubscriber(epoch: SessionEpoch): void {
		this.#closeSubscriber();
		this.#epochs = { ...this.#epochs, subscribe: epoch };
		const subscriber: Promise<Subscriber> = this.#ice.then(
			(iceServers) =>
				new Subscriber(
					peerConnection(iceServers),
					(sdp) => this.#signaling.send({ type: 'subscribeAnswer', sdp }),
					(tracks, delivered) => {
						// A late answer from a replaced connection must not overwrite the current tracks.
						if (this.#subscriber !== subscriber) return;
						this.remote = tracks;
						this.layers.received(delivered);
					},
					() => {
						if (this.#subscriber === subscriber) this.#onConnectionFailed('subscribe', epoch);
					}
				)
		);
		this.#subscriber = subscriber;
	}

	#closeLink(): void {
		this.#closePublisher();
		this.#closeSubscriber();
	}

	#closePublisher(): void {
		void this.#publisher?.then((publisher) => publisher.close());
		this.#publisher = null;
		this.#resets.done('publish');
	}

	#closeSubscriber(): void {
		void this.#subscriber?.then((subscriber) => subscriber.close());
		this.#subscriber = null;
		this.#resets.done('subscribe');
		this.remote = NO_REMOTE_TRACKS;
		this.layers.reset();
	}

	/** The peer connection for a session failed: ask the room for a new one, quietly. */
	#onConnectionFailed(kind: SessionKind, epoch: SessionEpoch): void {
		if (this.#resets.failed(kind, epoch)) this.toasts.show(RECONNECTING[kind], 'info');
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
		const { name, consent } = this.#join;
		const { resume } = this.state;
		this.#signaling.send({ type: 'join', name, consent, ...(resume !== null ? { resume } : {}) });
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
		if (!result.ok || result.value.kind === 'open') return;
		this.#finish({
			kind: 'ended',
			transcript: result.value.kind === 'ended' ? result.value.transcript : null
		});
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
