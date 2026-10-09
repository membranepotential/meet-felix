/**
 * The signaling protocol, generated from `crates/meeting-core` (`protocol.rs`, `ids.rs`) by
 * tsify. This module re-exports the generated types and the core's serde codec, and adds the
 * runtime helpers the shell needs; it restates none of the types.
 */
import { validateMeetingId } from './core.ts';
import type { MeetingId, TrackRef } from './wasm/meeting_core.js';

/**
 * `parseServerMsg(json)` validates a whole frame from the room's socket with the core's serde
 * types; `encodeClientMsg(msg)` validates and serializes one for it. Both throw on an invalid
 * message.
 */
export { encodeClientMsg, parseServerMsg } from './wasm/meeting_core.js';

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
