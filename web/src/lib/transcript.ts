/**
 * What the transcript page (D31) shows of a transcript. The shapes are generated from the core
 * (`meeting_core::http::TranscriptStatus`, `meeting_core::transcript::Transcript`); `api.ts`
 * fetches and checks them.
 */
import type { Millis, ParticipantId } from './protocol.ts';
import type { Gap, LabelledParagraph, Transcript } from './wasm/meeting_core.js';

export type { Gap, LabelledParagraph, Transcript };

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
