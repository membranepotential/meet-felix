/**
 * The transcript page's API (`GET /api/transcript/:token`): the job's status and, once ready,
 * the transcript itself (D17, D31). Every response is checked here, at the boundary.
 *
 * The shapes mirror `crates/meeting-core/src/transcript.rs` (`Transcript`) and
 * `crates/worker/src/recorder/state.rs` (`Status`); they are not generated, so a change there
 * must change them here too.
 */
import type { ApiError, ApiResult } from './api.ts';
import type { MeetingId, Millis, ParticipantId, Role, TranscriptRef } from './protocol.ts';

export interface Speaker {
	who: ParticipantId;
	/** Unique among the speakers: equal names are numbered, hosts are marked. */
	label: string;
	role: Role;
}

/** A participant's consent to recording, as the transcript records it. */
export interface Consent {
	who: ParticipantId;
	name: string;
	/** When they first consented. */
	at: Millis;
	noticeVersion: number;
	/** When they withdrew consent, if they did and did not give it again. */
	withdrawnAt: Millis | null;
}

/** Audio the transcript lacks: it never arrived, or the speech model failed on it. */
export interface Gap {
	who: ParticipantId;
	start: Millis;
	end: Millis;
	reason: 'missing' | 'untranscribed';
}

/** One speaker's turn. */
export interface Paragraph {
	who: ParticipantId;
	label: string;
	startMs: Millis;
	endMs: Millis;
	text: string;
}

/** `transcript.json`. */
export interface Transcript {
	meeting: MeetingId;
	startedAt: Millis;
	endedAt: Millis;
	speakers: Speaker[];
	consents: Consent[];
	gaps: Gap[];
	paragraphs: Paragraph[];
}

/** Milliseconds of recorded audio the job has finished, of all recorded audio. */
export interface Progress {
	done: number;
	total: number;
}

/** What every known transcript reports: the meeting and its times. */
interface Known {
	meeting: MeetingId;
	startedAt: Millis;
	/** `null` while the meeting runs. */
	endedAt: Millis | null;
}

export type TranscriptStatus =
	| ({ status: 'recording'; progress: Progress } & Known)
	| ({
			status: 'processing';
			progress: Progress;
			/** Set while the speech model's daily allowance is used up: the job continues then. */
			retryAt: Millis | null;
	  } & Known)
	| ({ status: 'ready'; transcript: Transcript; audioUntil: Millis | null } & Known)
	| ({ status: 'failed' } & Known)
	| { status: 'not-found' };

const isObject = (value: unknown): value is Record<string, unknown> =>
	typeof value === 'object' && value !== null;

const isNumber = (value: unknown): value is number =>
	typeof value === 'number' && Number.isFinite(value);

const isRole = (value: unknown): value is Role => value === 'host' || value === 'guest';

/** Checks every element with `parse`; `null` if any fails. */
function all<T>(value: unknown, parse: (item: unknown) => T | null): T[] | null {
	if (!Array.isArray(value)) return null;
	const items = value.map(parse);
	return items.every((item): item is T => item !== null) ? items : null;
}

const parseSpeaker = (v: unknown): Speaker | null =>
	isObject(v) && isNumber(v.who) && typeof v.label === 'string' && isRole(v.role)
		? { who: v.who, label: v.label, role: v.role }
		: null;

function parseConsent(v: unknown): Consent | null {
	if (!isObject(v) || !isNumber(v.who) || typeof v.name !== 'string') return null;
	if (!isNumber(v.at) || !isNumber(v.noticeVersion)) return null;
	const withdrawnAt = v.withdrawnAt === undefined ? null : v.withdrawnAt;
	if (withdrawnAt !== null && !isNumber(withdrawnAt)) return null;
	return { who: v.who, name: v.name, at: v.at, noticeVersion: v.noticeVersion, withdrawnAt };
}

const parseGap = (v: unknown): Gap | null =>
	isObject(v) &&
	isNumber(v.who) &&
	isNumber(v.start) &&
	isNumber(v.end) &&
	(v.reason === 'missing' || v.reason === 'untranscribed')
		? { who: v.who, start: v.start, end: v.end, reason: v.reason }
		: null;

const parseParagraph = (v: unknown): Paragraph | null =>
	isObject(v) &&
	isNumber(v.who) &&
	typeof v.label === 'string' &&
	isNumber(v.startMs) &&
	isNumber(v.endMs) &&
	typeof v.text === 'string'
		? { who: v.who, label: v.label, startMs: v.startMs, endMs: v.endMs, text: v.text }
		: null;

function parseTranscript(v: unknown): Transcript | null {
	if (!isObject(v) || typeof v.meeting !== 'string') return null;
	if (!isNumber(v.startedAt) || !isNumber(v.endedAt)) return null;
	const speakers = all(v.speakers, parseSpeaker);
	const consents = all(v.consents, parseConsent);
	const gaps = all(v.gaps, parseGap);
	const paragraphs = all(v.paragraphs, parseParagraph);
	if (speakers === null || consents === null || gaps === null || paragraphs === null) return null;
	return {
		meeting: v.meeting,
		startedAt: v.startedAt,
		endedAt: v.endedAt,
		speakers,
		consents,
		gaps,
		paragraphs
	};
}

const parseProgress = (v: unknown): Progress | null =>
	isObject(v) && isNumber(v.done) && isNumber(v.total) ? { done: v.done, total: v.total } : null;

/** Checks a status body; `null` if it is not what the Worker promises. */
export function parseTranscriptStatus(body: unknown): TranscriptStatus | null {
	if (!isObject(body) || typeof body.meeting !== 'string' || !isNumber(body.startedAt)) {
		return null;
	}
	const optional = (v: unknown): Millis | null | undefined =>
		v === undefined ? null : isNumber(v) ? v : undefined;
	const endedAt = optional(body.endedAt);
	if (endedAt === undefined) return null;
	const known: Known = { meeting: body.meeting, startedAt: body.startedAt, endedAt };
	switch (body.status) {
		case 'recording':
		case 'processing': {
			const progress = parseProgress(body.progress);
			const retryAt = optional(body.retryAt);
			if (progress === null || retryAt === undefined) return null;
			return body.status === 'recording'
				? { status: 'recording', progress, ...known }
				: { status: 'processing', progress, retryAt, ...known };
		}
		case 'ready': {
			const transcript = parseTranscript(body.transcript);
			const audioUntil = optional(body.audioUntil);
			if (transcript === null || audioUntil === undefined) return null;
			return { status: 'ready', transcript, audioUntil, ...known };
		}
		case 'failed':
			return { status: 'failed', ...known };
		default:
			return null;
	}
}

const path = (token: TranscriptRef) => `/api/transcript/${encodeURIComponent(token)}`;

/** The transcript behind a link token, or where its job stands. An unknown token is `not-found`. */
export async function getTranscript(token: TranscriptRef): Promise<ApiResult<TranscriptStatus>> {
	const fail = (error: ApiError): ApiResult<never> => ({ ok: false, error });
	let response: Response;
	try {
		response = await fetch(path(token));
	} catch {
		return fail({ kind: 'network' });
	}
	if (response.status === 404) return { ok: true, value: { status: 'not-found' } };
	if (response.status === 429) return fail({ kind: 'rate-limited' });
	if (!response.ok) return fail({ kind: 'server', status: response.status });
	const value = parseTranscriptStatus(await response.json().catch(() => null));
	return value === null ? fail({ kind: 'malformed' }) : { ok: true, value };
}

/** The download link of a transcript file. */
export const transcriptFile = (token: TranscriptRef, format: 'md' | 'json'): string =>
	`${path(token)}/transcript.${format}`;

/** What the page lists: the paragraphs with the gaps between them, in time order. */
export type Entry =
	| { kind: 'paragraph'; at: Millis; paragraph: Paragraph }
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
