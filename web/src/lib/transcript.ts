/**
 * What the transcript page (D31) shows of a transcript. `api.ts` fetches and checks it with the
 * core's serde types (`meeting_core::http::TranscriptStatus`).
 */
import type { DisplayName, MeetingId, Millis, ParticipantId, Role } from './protocol.ts';

// The core does not yet generate TypeScript for `meeting_core::transcript::Transcript` and its
// parts (`transcript.rs`, `recording.rs`), so these restate them, under the generated names.
// Once it does, re-export those instead. Only the types are restated: the core's parser
// checks every transcript.

export interface Speaker {
	who: ParticipantId;
	/** Unique among the speakers: equal names are numbered, hosts are marked. */
	label: string;
	role: Role;
}

/** A participant's consent to recording, as the transcript records it. */
export interface ConsentRecord {
	who: ParticipantId;
	name: DisplayName;
	/** When they consented. */
	at: Millis;
	noticeVersion: number;
	/** When they withdrew consent, if they did and did not give it again. */
	withdrawnAt?: Millis;
}

export type GapReason = 'missing' | 'untranscribed';

/** Audio the transcript lacks: it never arrived, or the speech model failed on it. */
export interface Gap {
	who: ParticipantId;
	start: Millis;
	end: Millis;
	reason: GapReason;
}

/** One speaker's turn. */
export interface LabelledParagraph {
	who: ParticipantId;
	label: string;
	startMs: Millis;
	endMs: Millis;
	text: string;
}

/** `transcript.json`. */
export interface Transcript {
	version: number;
	meeting: MeetingId;
	startedAt: Millis;
	endedAt: Millis;
	speakers: Speaker[];
	consents: ConsentRecord[];
	gaps: Gap[];
	paragraphs: LabelledParagraph[];
}

/** What the page lists: the paragraphs with the gaps between them, in time order. */
export type Entry =
	| { kind: 'paragraph'; at: Millis; paragraph: LabelledParagraph }
	| { kind: 'gap'; at: Millis; gap: Gap; label: string };

/** Merges paragraphs and gaps by start time; a gap comes before a paragraph that starts with it. */
export function timeline(transcript: Transcript): Entry[] {
	const label = (who: ParticipantId) =>
		transcript.speakers.find((s) => s.who === who)?.label ?? `Speaker ${who}`;
	const entries: Entry[] = [
		...transcript.gaps.map((gap): Entry => ({
			kind: 'gap',
			at: gap.start,
			gap,
			label: label(gap.who)
		})),
		...transcript.paragraphs.map((paragraph): Entry => ({
			kind: 'paragraph',
			at: paragraph.startMs,
			paragraph
		}))
	];
	// A stable sort keeps gaps ahead of paragraphs at the same time.
	return entries.toSorted((a, b) => a.at - b.at);
}

/** Whole minutes, rounded up, so a short recording never shows as 0. */
export const minutes = (ms: number): number => Math.ceil(ms / 60_000);

/** A length of time in words: "45 s", "12 min", "1 h 5 min". */
export function duration(ms: number): string {
	const minutes = Math.round(ms / 60_000);
	if (ms < 60_000) return `${Math.max(1, Math.round(ms / 1000))} s`;
	if (minutes < 60) return `${minutes} min`;
	const hours = Math.floor(minutes / 60);
	const rest = minutes % 60;
	return rest === 0 ? `${hours} h` : `${hours} h ${rest} min`;
}
