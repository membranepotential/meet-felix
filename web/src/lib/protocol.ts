/**
 * Signaling protocol between the browser and the Room Durable Object.
 *
 * GENERATED LATER: these types will be generated from `crates/meeting-core` via `tsify`
 * and replace this hand-written file. Until then they follow docs/core/README.md "State, events and effects".
 *
 * Wire conventions assumed here (meeting-core must match, or this file changes with it):
 * - enums are internally tagged, `#[serde(tag = "type")]`, with Rust variant names as tags;
 * - field names stay snake_case as in Rust;
 * - `Option<T>` serializes as `T | null`.
 */

/** Branded string so ids of different kinds cannot be swapped by accident. */
type Brand<T, B extends string> = T & { readonly __brand: B };

/** Three words joined by hyphens, e.g. `amber-otter-lamp`. Also the URL path. */
export type MeetingId = Brand<string, 'MeetingId'>;
export type ParticipantId = Brand<string, 'ParticipantId'>;
/** SFU track name, unique within a meeting. */
export type TrackId = Brand<string, 'TrackId'>;
/** Unix time in milliseconds. */
export type Millis = number;

export type Role = 'Host' | 'Guest';
export type TrackKind = 'audio' | 'video';
export type TrackSource = 'microphone' | 'camera' | 'screen';
/** Simulcast layers published per video track, lowest to highest resolution. */
export type Rid = 'q' | 'h' | 'f';

export interface Track {
	id: TrackId;
	kind: TrackKind;
	source: TrackSource;
	/** The publisher paused this track (mic or camera off). */
	muted: boolean;
}

export interface Participant {
	id: ParticipantId;
	name: string;
	role: Role;
	tracks: Track[];
}

export interface Recording {
	since: Millis;
	by: ParticipantId;
}

export interface RoomState {
	meeting: MeetingId;
	participants: Participant[];
	/** Locked rooms make late joiners knock; the host admits them. */
	locked: boolean;
	/** Present while recording runs; every client renders the badge from this. */
	recording: Recording | null;
}

/** Media sections the client publishes in one offer, keyed by SDP `mid`. */
export interface PublishedTrack {
	mid: string;
	kind: TrackKind;
	source: TrackSource;
}

export type SfuSession = 'publish' | 'subscribe';

export type ClientMsg =
	| { type: 'Join'; name: string; consent: boolean; host_token: string | null }
	| { type: 'PublishOffer'; sdp: string; tracks: PublishedTrack[] }
	| { type: 'SubscribeAnswer'; sdp: string }
	| { type: 'Ice'; session: SfuSession; candidate: RTCIceCandidateInit }
	| { type: 'SetLayer'; track: TrackId; rid: Rid }
	| { type: 'Lock'; locked: boolean }
	| { type: 'Admit'; who: ParticipantId }
	| { type: 'Kick'; who: ParticipantId }
	| { type: 'Mute'; who: ParticipantId }
	| { type: 'SetRecording'; on: boolean }
	| { type: 'End' }
	| { type: 'Leave' };

export type ServerMsg =
	| { type: 'Welcome'; you: ParticipantId; state: RoomState }
	| { type: 'State'; state: RoomState }
	| { type: 'PublishAnswer'; sdp: string }
	| { type: 'SubscribeOffer'; sdp: string }
	| { type: 'Knock'; who: { id: ParticipantId; name: string } }
	| { type: 'Ended'; transcript_url: string | null }
	| { type: 'Error'; message: string };

export type ServerMsgType = ServerMsg['type'];

const SERVER_MSG_TYPES: ReadonlySet<string> = new Set<ServerMsgType>([
	'Welcome',
	'State',
	'PublishAnswer',
	'SubscribeOffer',
	'Knock',
	'Ended',
	'Error'
]);

/**
 * Parses one WebSocket frame. Checks only the tag; full structural validation arrives with
 * the generated decoder from meeting-core (serde in WASM), which this function will call.
 */
export function parseServerMsg(raw: string): ServerMsg | null {
	let value: unknown;
	try {
		value = JSON.parse(raw);
	} catch {
		return null;
	}
	if (typeof value !== 'object' || value === null || !('type' in value)) return null;
	return typeof value.type === 'string' && SERVER_MSG_TYPES.has(value.type)
		? (value as ServerMsg)
		: null;
}

const MEETING_ID = /^[a-z]+-[a-z]+-[a-z]+$/;

/** Validates a URL segment as a meeting id (three lowercase words). */
export function parseMeetingId(segment: string): MeetingId | null {
	return MEETING_ID.test(segment) ? (segment as MeetingId) : null;
}
