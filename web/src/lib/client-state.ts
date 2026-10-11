/**
 * Pure dispatch of server messages: what the client shows, and which side effects the
 * session must run. The room decides everything about the meeting (docs/core/README.md,
 * D20); this module only tracks where the client stands and turns messages into commands.
 */
import type {
	ErrorCode,
	IncomingTrack,
	ParticipantId,
	ParticipantView,
	Presence,
	ResumeToken,
	RoomView,
	Sdp,
	ServerMsg,
	SessionEpoch,
	SessionEpochs,
	TrackKind,
	TranscriptRef
} from './protocol.ts';
import type { ToastTone } from './toasts.svelte.ts';

/** Why the stay is over. */
export type SessionEnd =
	| { kind: 'left' }
	| { kind: 'kicked' }
	| { kind: 'ended'; transcript: TranscriptRef | null }
	| { kind: 'refused'; reason: string };

/** Where the client stands. */
export type Stage =
	/** The socket is connecting, or `Join` is sent and the room has not answered. */
	| { kind: 'joining' }
	/** Outside the room: no host has arrived yet, or the room is locked and a host must admit us. */
	| { kind: 'waiting' }
	/** In the meeting. */
	| { kind: 'in'; you: ParticipantId; room: RoomView }
	| { kind: 'over'; end: SessionEnd };

export interface ClientState {
	stage: Stage;
	/** From the last `Welcome`; sent in `Join` after a reconnect to keep our place. */
	resume: ResumeToken | null;
	/** The room asked for consent to recording and we have not answered yet. */
	consentAsked: boolean;
}

export const INITIAL: ClientState = {
	stage: { kind: 'joining' },
	resume: null,
	consentAsked: false
};

/** A side effect for the session to run, in order. */
export type Command =
	| { kind: 'toast'; text: string; tone: ToastTone }
	/** Harmless for the user, but worth a line in the console. */
	| { kind: 'warn'; text: string }
	/** Switch the local microphone off. */
	| { kind: 'mute-mic' }
	/** The stay is over: release media and close the socket. */
	| { kind: 'teardown' }
	/** This page speaks a protocol the room no longer serves: load it again. */
	| { kind: 'reload' }
	| MediaCommand;

/** A side effect for the SFU peer connections (`MediaLink`). */
export type MediaCommand =
	/**
	 * We are in. `fresh` means a new participant: open new SFU peer connections. `epochs` names
	 * the room's current sessions; a connection made for an older one must be replaced. The
	 * session also makes the mic switch and the room's mute flag agree (`syncMute`).
	 */
	| { kind: 'welcomed'; fresh: boolean; epochs: SessionEpochs }
	/** The answer to our publish offer; the SFU did not publish the `rejected` kinds. */
	| { kind: 'publish-answer'; sdp: Sdp; rejected: TrackKind[] }
	| { kind: 'publish-rejected' }
	| { kind: 'subscribe-offer'; sdp: Sdp; tracks: IncomingTrack[] }
	/** The room replaced our publish session: publish everything again on a new connection. */
	| { kind: 'publish-reset'; epoch: SessionEpoch }
	/** The room replaced our subscribe session: receive on a new connection. */
	| { kind: 'subscribe-reset'; epoch: SessionEpoch }
	/**
	 * A subscribe failed and the room retries it. Often the first pull only raced the
	 * publisher's connection, so the session shows `notice` only if no `subscribe-offer`
	 * follows within a grace period.
	 */
	| { kind: 'subscribe-retrying'; notice: string }
	/** The room refused a `SetLayer` for a track it does not deliver yet: ask again later. */
	| { kind: 'layer-refused' };

/** Errors that answer a `PublishOffer`: the publish in flight failed. */
const PUBLISH_ERRORS: ReadonlySet<ErrorCode> = new Set<ErrorCode>([
	'invalidOffer',
	'negotiationInProgress',
	'publishFailed'
]);

/** User-facing text per error code; `null` for errors the user can do nothing about. */
export function describeError(code: ErrorCode): string | null {
	switch (code) {
		case 'malformedMessage':
			return 'The meeting did not understand this page. Reload the page to continue.';
		case 'notJoined':
			return 'You are not in the meeting yet. Wait a moment, then try again.';
		case 'notHost':
			return 'Only hosts can do that.';
		case 'invalidTarget':
			return 'That person is no longer waiting or in the meeting.';
		case 'invalidOffer':
		case 'negotiationInProgress':
		case 'publishFailed':
			return 'Your camera, microphone or screen could not be shared. Turn it off and on to try again.';
		case 'sfuUnavailable':
			return 'The media server is not reachable right now. The meeting keeps trying on its own.';
		case 'subscribeFailed':
			return 'Some video or audio did not arrive. The meeting retries on its own.';
		case 'roomFull':
			return 'This meeting is full. Try again later.';
		case 'outdatedClient':
			return 'This page is out of date. Reload it to join the meeting.';
		case 'notPublished':
		case 'unexpectedAnswer':
		case 'notSubscribed':
			return null;
	}
}

/** What makes the mic switch and the room's mute flag agree: switch the mic off, or report it. */
export type MuteSync = 'none' | 'mute-mic' | 'report';

/**
 * After a welcome, how to make the local mic switch and the room's mute flag (`roomMuted`)
 * agree. `known` is the flag the room held for us before, as far as this client knows (the last
 * report that got through, or the last welcome's), `null` for a new participant. A switch the
 * user made since, while the socket was down, is reported. Any other difference was made by the
 * room, and muting wins: a host muted us, perhaps while the socket was down, or the room records
 * without our consent, so the mic goes off; a mic switched off before joining is reported.
 * Later switches are reported as they happen (`MuteReport` in `mute.ts`).
 */
export function syncMute(roomMuted: boolean, micOn: boolean, known: boolean | null): MuteSync {
	const muted = !micOn;
	if (roomMuted === muted) return 'none';
	if (known !== null && known !== muted) return 'report';
	return roomMuted ? 'mute-mic' : 'report';
}

/** Where a participant counts for the recording: in the room, or let in but waiting for consent. */
const ADMITTED_OR_ASKED: ReadonlySet<Presence> = new Set<Presence>([
	'awaitingConsent',
	'connecting',
	'joined'
]);

/**
 * The others a running recording leaves out because they have not accepted it (D15): those in
 * the room are muted and not recorded, those let in wait outside until they accept. Knocking
 * guests are left to their knock request. Empty while nothing records.
 */
export function awaitingConsent(room: RoomView, you: ParticipantId): ParticipantView[] {
	if (room.recording === undefined) return [];
	return room.participants.filter(
		(p) => p.id !== you && !p.consentRecording && ADMITTED_OR_ASKED.has(p.presence)
	);
}

/** The stay is over, because of the room's message or the client's own decision. */
export const over = (state: ClientState, end: SessionEnd): [ClientState, Command[]] => [
	{ ...state, stage: { kind: 'over', end }, consentAsked: false },
	[{ kind: 'teardown' }]
];

/** The user accepted the recording the room asked about. */
export const consentGiven = (state: ClientState): ClientState => ({
	...state,
	consentAsked: false
});

/** Applies one server message. Messages after the stay is over change nothing. */
export function reduce(state: ClientState, msg: ServerMsg): [ClientState, Command[]] {
	const { stage } = state;
	if (stage.kind === 'over') return [state, []];
	switch (msg.type) {
		case 'welcome': {
			const fresh = stage.kind !== 'in' || stage.you !== msg.you;
			return [
				{ ...state, stage: { kind: 'in', you: msg.you, room: msg.state }, resume: msg.resume },
				[{ kind: 'welcomed', fresh, epochs: msg.epochs }]
			];
		}
		case 'state': {
			if (stage.kind !== 'in') return [state, []];
			const was = stage.room.recording !== undefined;
			const is = msg.state.recording !== undefined;
			const toasts: Command[] =
				was === is
					? []
					: [
							{ kind: 'toast', text: is ? 'Recording started' : 'Recording stopped', tone: 'alert' }
						];
			return [{ ...state, stage: { ...stage, room: msg.state } }, toasts];
		}
		case 'waiting':
			return [{ ...state, stage: { kind: 'waiting' } }, []];
		case 'consentRequired':
			return [{ ...state, consentAsked: true }, []];
		case 'publishAnswer':
			return [state, [{ kind: 'publish-answer', sdp: msg.sdp, rejected: msg.rejected ?? [] }]];
		case 'subscribeOffer':
			return [state, [{ kind: 'subscribe-offer', sdp: msg.sdp, tracks: msg.tracks }]];
		case 'publishReset':
			return [state, [{ kind: 'publish-reset', epoch: msg.epoch }]];
		case 'subscribeReset':
			return [state, [{ kind: 'subscribe-reset', epoch: msg.epoch }]];
		case 'knock':
			// Hosts see the knocking guest, with Admit and Deny, from the room view's `presence`;
			// the `State` broadcast that lists them arrives just before this message.
			return [state, []];
		case 'muted':
			return [
				state,
				[{ kind: 'mute-mic' }, { kind: 'toast', text: 'Your microphone was muted.', tone: 'alert' }]
			];
		case 'kicked':
			return over(state, { kind: 'kicked' });
		case 'ended':
			return over(state, { kind: 'ended', transcript: msg.transcript ?? null });
		case 'error': {
			const text = describeError(msg.code);
			// The room took no place for us and closes the socket for good: the stay is over
			// before it began, and the session stops reconnecting. An outdated page reloads.
			if (msg.code === 'roomFull' || msg.code === 'outdatedClient') {
				const [next, commands] = over(state, { kind: 'refused', reason: text ?? '' });
				return msg.code === 'outdatedClient'
					? [next, [...commands, { kind: 'reload' }]]
					: [next, commands];
			}
			const commands: Command[] = [];
			if (PUBLISH_ERRORS.has(msg.code)) commands.push({ kind: 'publish-rejected' });
			if (msg.code === 'notSubscribed') commands.push({ kind: 'layer-refused' });
			// An `Unpublish` that crossed a reconnect: the room no longer had the track.
			if (msg.code === 'notPublished') {
				commands.push({ kind: 'warn', text: 'the room had no track to unpublish' });
			}
			if (text !== null) {
				commands.push(
					msg.code === 'subscribeFailed'
						? { kind: 'subscribe-retrying', notice: text }
						: { kind: 'toast', text, tone: 'alert' }
				);
			}
			return [state, commands];
		}
	}
}
