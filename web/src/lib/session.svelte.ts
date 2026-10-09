/**
 * One stay in a meeting: the signaling connection, the room mirror, notices, and the
 * screen wake lock. Created when the user presses Join, disposed when they leave.
 */
import { holdWakeLock } from './media/wakelock.ts';
import type { ClientMsg, MeetingId, ParticipantId, ServerMsg } from './protocol.ts';
import { Room } from './room.svelte.ts';
import { SignalingClient, type ConnectionStatus } from './signaling.ts';
import { Toasts } from './toasts.svelte.ts';

export interface JoinRequest {
	name: string;
	consent: boolean;
	hostToken: string | null;
}

/** Why the session is over; `null` while it runs. */
export type SessionEnd =
	| { kind: 'left' }
	| { kind: 'ended'; transcriptUrl: string | null }
	| { kind: 'refused'; reason: string };

export interface Knock {
	id: ParticipantId;
	name: string;
}

export class Session {
	readonly room = new Room();
	readonly toasts = new Toasts();
	connection: ConnectionStatus = $state.raw({ kind: 'connecting', attempt: 1 });
	end = $state.raw<SessionEnd | null>(null);
	/** People waiting to enter a locked room; the host admits them. */
	knocks = $state.raw<Knock[]>([]);
	/** Who is talking. Set from SFU audio levels in phase 2. */
	activeSpeaker = $state.raw<ParticipantId | null>(null);

	readonly #join: JoinRequest;
	readonly #signaling: SignalingClient | null;
	readonly #onEnd: (end: SessionEnd) => void;
	#releaseWakeLock: () => void = () => {};

	/**
	 * `onEnd` runs once when the stay is over (left, ended, refused).
	 * `offline` skips the server; the dev demo feeds the room directly.
	 */
	constructor(
		meeting: MeetingId,
		join: JoinRequest,
		{ onEnd, offline = false }: { onEnd: (end: SessionEnd) => void; offline?: boolean }
	) {
		this.#join = join;
		this.#onEnd = onEnd;
		if (offline) this.connection = { kind: 'open' };
		this.#signaling = offline
			? null
			: new SignalingClient(meeting, {
					onOpen: () => this.#sendJoin(),
					onMessage: (msg) => this.receive(msg),
					onStatus: (status) => this.#onStatus(status)
				});
	}

	start(): void {
		this.#releaseWakeLock = holdWakeLock();
		this.#signaling?.connect();
	}

	/** Sends a command; tells the user when it cannot go out. */
	send(msg: ClientMsg): void {
		if (!this.#signaling?.send(msg)) {
			this.toasts.show('Not connected to the meeting. Try again in a moment.', 'alert');
		}
	}

	admit(id: ParticipantId): void {
		this.send({ type: 'Admit', who: id });
		this.knocks = this.knocks.filter((k) => k.id !== id);
	}

	leave(): void {
		this.#signaling?.send({ type: 'Leave' });
		this.#finish({ kind: 'left' });
	}

	dispose(): void {
		this.#signaling?.close();
		this.#releaseWakeLock();
	}

	/** Handles one server message. Public so the dev demo can inject state. */
	receive(msg: ServerMsg): void {
		switch (msg.type) {
			case 'Welcome':
			case 'State': {
				const wasRecording = this.room.recording !== null;
				this.room.apply(msg);
				const isRecording = this.room.recording !== null;
				if (msg.type === 'State' && wasRecording !== isRecording) {
					this.toasts.show(isRecording ? 'Recording started' : 'Recording stopped', 'alert');
				}
				return;
			}
			case 'Knock':
				if (!this.knocks.some((k) => k.id === msg.who.id)) this.knocks = [...this.knocks, msg.who];
				return;
			case 'Ended':
				this.#finish({ kind: 'ended', transcriptUrl: msg.transcript_url });
				return;
			case 'Error':
				this.toasts.show(msg.message, 'alert');
				return;
			case 'PublishAnswer':
			case 'SubscribeOffer':
				// Phase 2: hand to `sfu.ts` (applyPublishAnswer / handleSubscribeOffer).
				return;
		}
	}

	#sendJoin(): void {
		const { name, consent, hostToken } = this.#join;
		this.#signaling?.send({ type: 'Join', name, consent, host_token: hostToken });
	}

	#onStatus(status: ConnectionStatus): void {
		this.connection = status;
		if (status.kind === 'closed' && status.code !== 1000 && this.end === null) {
			this.#finish({
				kind: 'refused',
				reason: status.reason || 'The meeting closed the connection.'
			});
		}
	}

	#finish(end: SessionEnd): void {
		if (this.end !== null) return;
		this.end = end;
		this.dispose();
		this.#onEnd(end);
	}
}
