/**
 * One stay in a meeting: the signaling socket, the client state from `client-state.ts`, the
 * microphone's mute flag in the room, the others' voices, notices and the screen wake lock.
 * The SFU peer connections are the `MediaLink`'s. Created inside the Join tap, disposed when the
 * stay is over.
 */
import { untrack } from 'svelte';
import { on } from 'svelte/events';
import { getMeeting } from './api.ts';
import {
	awaitingConsent,
	consentGiven,
	INITIAL,
	over,
	reduce,
	syncMute,
	type ClientState,
	type Command,
	type SessionEnd
} from './client-state.ts';
import type { LocalMedia } from './local-media.svelte.ts';
import { MediaLink } from './media-link.svelte.ts';
import { RemoteAudio } from './media/remote-audio.svelte.ts';
import { holdWakeLock } from './media/wakelock.ts';
import {
	protocolVersion,
	type ClientMsg,
	type DisplayName,
	type MeetingId,
	type ParticipantId,
	type ParticipantView,
	type ServerMsg
} from './protocol.ts';
import { SignalingClient, type ConnectionStatus } from './signaling.ts';
import { Toasts } from './toasts.svelte.ts';

export interface JoinRequest {
	name: DisplayName;
	/** Whether the user accepted recording on the pre-join screen. */
	consent: boolean;
}

/** Shown once each time the mic stalls (`LocalMedia.micStalled`); the button then restarts it. */
const MIC_STALLED = 'Microphone was interrupted. Tap the microphone button to turn it on again.';

/** Participants who get a tile: admitted, not waiting outside. */
const isInRoom = (p: ParticipantView) => p.presence === 'joined' || p.presence === 'connecting';

/** When this tab last reloaded because the room called the page out of date. */
const OUTDATED_RELOAD_KEY = 'meet-felix:outdated-reload';
/** A page still out of date this soon after such a reload came from a cache: do not loop. */
const OUTDATED_RELOAD_PAUSE_MS = 5 * 60 * 1000;

/**
 * Loads the page again for the current app, unless this tab did so moments ago, which means
 * the reload brought the same old app; then the end screen asks the user to reload.
 */
function reloadOutdated(): void {
	try {
		const last = Number(sessionStorage.getItem(OUTDATED_RELOAD_KEY));
		if (Date.now() - last < OUTDATED_RELOAD_PAUSE_MS) return;
		sessionStorage.setItem(OUTDATED_RELOAD_KEY, String(Date.now()));
	} catch {
		// Without storage a loop cannot be ruled out; the end screen asks instead.
		return;
	}
	window.location.reload();
}

export class Session {
	readonly toasts = new Toasts();
	/** The others' voices and the `AudioContext` for the level meters. */
	readonly audio: RemoteAudio;
	/** The SFU peer connections: received tracks (`remote`) and layer requests (`layers`). */
	readonly link: MediaLink;
	connection: ConnectionStatus = $state.raw({ kind: 'connecting', attempt: 1 });
	state: ClientState = $state.raw(INITIAL);

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
	#join: JoinRequest;
	/** The mute flag this client last got through to the room for its participant (`syncMute`). */
	#reportedMuted: boolean | null = null;
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
		this.link = new MediaLink(
			meeting,
			(msg) => this.#signaling.send(msg),
			() => media.outgoing,
			(text, tone) => this.toasts.show(text, tone)
		);
	}

	start(): void {
		this.#releaseWakeLock = holdWakeLock();
		this.#signaling.connect();
		this.#stopEffects = $effect.root(() => {
			// Publishing follows the local tracks: device switches, camera on/off, screen share.
			$effect(() => {
				const outgoing = this.#media.outgoing;
				untrack(() => this.link.publish(outgoing));
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
			// is made on the next welcome (`#syncMute`).
			$effect(() => {
				const muted = !this.#media.micOn;
				untrack(() => {
					if (this.stage.kind === 'in') this.#reportMuted(muted);
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
		this.state = consentGiven(this.state);
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
		this.#apply(reduce(this.state, msg));
		const { stage } = this.state;
		if (stage.kind === 'in') {
			this.link.retain(
				stage.room.participants.flatMap((p) => p.tracks.map((kind) => ({ owner: p.id, kind })))
			);
		}
	}

	/** Closes the socket and the peer connections. Local media stays with its owner. */
	dispose(): void {
		if (this.#disposed) return;
		this.#disposed = true;
		this.#stopEffects();
		this.#signaling.close();
		this.link.dispose();
		this.audio.dispose();
		this.#releaseWakeLock();
	}

	#apply([next, commands]: [ClientState, Command[]]): void {
		this.state = next;
		for (const command of commands) this.#run(command);
	}

	#run(command: Command): void {
		switch (command.kind) {
			case 'toast':
				this.toasts.show(command.text, command.tone);
				return;
			case 'warn':
				console.warn(command.text);
				return;
			case 'mute-mic':
				this.#media.mute();
				return;
			case 'teardown':
				this.#over();
				return;
			case 'reload':
				reloadOutdated();
				return;
			case 'welcomed':
				this.link.run(command);
				this.#syncMute(command.fresh);
				return;
			default:
				this.link.run(command);
		}
	}

	/** After a welcome, makes the mic switch and the room's mute flag agree (`syncMute`). */
	#syncMute(fresh: boolean): void {
		if (fresh) this.#reportedMuted = null;
		const { micOn } = this.#media;
		switch (syncMute(this.me?.muted ?? false, micOn, this.#reportedMuted)) {
			case 'mute-mic':
				this.#media.mute();
				return;
			case 'report':
				this.#reportMuted(!micOn);
				return;
			case 'none':
				return;
		}
	}

	#reportMuted(muted: boolean): void {
		if (this.#signaling.send({ type: 'setMuted', muted })) this.#reportedMuted = muted;
	}

	#sendJoin(): void {
		const { name, consent } = this.#join;
		const { resume } = this.state;
		this.#signaling.send({
			type: 'join',
			name,
			consent,
			protocol: protocolVersion(),
			...(resume !== null ? { resume } : {})
		});
	}

	#onStatus(status: ConnectionStatus): void {
		this.connection = status;
		if (status.kind === 'waiting') void this.#checkMeeting();
		// The room's message about why (kicked, ended, full) usually ended the stay already.
		if (status.kind === 'dismissed') {
			this.#finish({
				kind: 'refused',
				reason: status.reason || 'The meeting closed the connection.'
			});
			if (status.after === 'reload') reloadOutdated();
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
		// The lookup gives out no transcript link; the room's `ended` message carries it.
		this.#finish({ kind: 'ended', transcript: null });
	}

	#finish(end: SessionEnd): void {
		if (this.end !== null) return;
		this.#apply(over(this.state, end));
	}

	/** Releases camera and mic as soon as the stay is over, not only when the page closes. */
	#over(): void {
		this.dispose();
		this.#media.dispose();
	}
}
