/**
 * The signaling protocol, generated from `crates/meeting-core` (`protocol.rs`, `ids.rs`) by
 * tsify. This module re-exports the generated types and adds the runtime helpers the shell
 * needs; it restates none of the types.
 */
import { validateMeetingId } from './core.ts';
import type { MeetingId, ServerMsg, TrackRef } from './wasm/meeting_core.js';

export type {
	ClientMsg,
	DisplayName,
	ErrorCode,
	HostToken,
	IncomingTrack,
	LocalTrack,
	MeetingId,
	Mid,
	Millis,
	ParticipantId,
	ParticipantView,
	Presence,
	Recording,
	RecordingPolicy,
	ResumeToken,
	Rid,
	Role,
	RoomView,
	Sdp,
	ServerMsg,
	TrackKind,
	TrackName,
	TrackRef,
	TranscriptRef
} from './wasm/meeting_core.js';

/** Every `ServerMsg` tag; `satisfies` fails the build when the core adds or renames one. */
const SERVER_MSG_TYPES = {
	welcome: true,
	state: true,
	waiting: true,
	publishAnswer: true,
	subscribeOffer: true,
	knock: true,
	consentRequired: true,
	muted: true,
	kicked: true,
	ended: true,
	error: true
} as const satisfies Record<ServerMsg['type'], true>;

const isServerMsgType = (tag: string): tag is ServerMsg['type'] =>
	Object.hasOwn(SERVER_MSG_TYPES, tag);

/**
 * Parses one WebSocket frame. Checks the tag only and trusts the room for the rest; the core
 * exports no serde decoder to WASM yet.
 */
export function parseServerMsg(raw: string): ServerMsg | null {
	let value: unknown;
	try {
		value = JSON.parse(raw);
	} catch {
		return null;
	}
	if (typeof value !== 'object' || value === null || !('type' in value)) return null;
	return typeof value.type === 'string' && isServerMsgType(value.type)
		? (value as ServerMsg)
		: null;
}

/**
 * Validates a URL segment as a meeting id with the core's word list. Lowercases first,
 * because phone keyboards capitalise the first letter of a typed link.
 */
export function parseMeetingId(segment: string): MeetingId | null {
	const id = segment.toLowerCase();
	return validateMeetingId(id) ? id : null;
}

/** A map key for a track: `TrackRef` is an object, so it cannot be one itself. */
export const trackKey = ({ owner, kind }: TrackRef): string => `${owner}:${kind}`;
