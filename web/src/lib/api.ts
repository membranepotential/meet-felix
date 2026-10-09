/**
 * HTTP API of the Worker (`/api/*`). Every response is checked here, at the boundary; the
 * rest of the app works with the typed results.
 */
import {
	parseMeetingId,
	type MeetingId,
	type Millis,
	type Phase,
	type RecordingPolicy,
	type TranscriptRef
} from './protocol.ts';

/** One of the owner's meetings, as `GET /api/meetings` lists it. */
export interface PastMeeting {
	meeting: MeetingId;
	/** When the meeting was created, in milliseconds since the Unix epoch. */
	createdAt: Millis;
	phase: Phase;
	/** The transcript link token; `null` if the meeting was not recorded. */
	transcript: TranscriptRef | null;
}

/** A meeting that can still be joined: waiting for the host, or running. */
export type OpenPhase = Exclude<Phase, 'ended'>;

/** What the pre-join screen needs to know about a meeting. */
export type MeetingInfo =
	| { kind: 'open'; recordingPolicy: RecordingPolicy; phase: OpenPhase }
	/** `transcript`: the link token of a recorded meeting, if the Worker gives it out. */
	| { kind: 'ended'; transcript: TranscriptRef | null }
	| { kind: 'not-found' };

export type ApiError =
	| { kind: 'network' }
	| { kind: 'unauthorized' }
	| { kind: 'rate-limited' }
	| { kind: 'server'; status: number }
	| { kind: 'malformed' };

export type ApiResult<T> = { ok: true; value: T } | { ok: false; error: ApiError };

const fail = (error: ApiError): ApiResult<never> => ({ ok: false, error });

/** Reads a successful response; `null` means the reply is not what the endpoint promises. */
type Reader<T> = (response: Response) => Promise<T | null>;

/** A JSON body, checked by `parse`. */
const json =
	<T>(parse: (body: unknown) => T | null): Reader<T> =>
	async (response) =>
		parse(await response.json().catch(() => null));

/**
 * No body: only 204 counts. A 200 here is not this endpoint answering, for example the
 * static app's fallback page served for an `/api` route the Worker does not have yet.
 */
const noContent: Reader<void> = async (response) => (response.status === 204 ? undefined : null);

/**
 * Fetches `path` and reads a 2xx response with `read`. `onStatus` may turn a non-2xx status
 * into a value (such as 404 → not found) before it counts as an error. Requests stay
 * same-origin (in development through Vite's proxy), so the browser sends the owner's session
 * cookie with the default `credentials: 'same-origin'`.
 */
async function request<T>(
	path: string,
	init: RequestInit,
	read: Reader<T>,
	onStatus: (status: number) => T | null = () => null
): Promise<ApiResult<T>> {
	let response: Response;
	try {
		response = await fetch(path, init);
	} catch {
		return fail({ kind: 'network' });
	}
	if (!response.ok) {
		const value = onStatus(response.status);
		if (value !== null) return { ok: true, value };
		if (response.status === 401 || response.status === 403) return fail({ kind: 'unauthorized' });
		if (response.status === 429) return fail({ kind: 'rate-limited' });
		return fail({ kind: 'server', status: response.status });
	}
	const value = await read(response);
	return value === null ? fail({ kind: 'malformed' }) : { ok: true, value };
}

const isObject = (value: unknown): value is Record<string, unknown> =>
	typeof value === 'object' && value !== null;

const isRecordingPolicy = (value: unknown): value is RecordingPolicy =>
	value === 'fromStart' || value === 'hostDecides';

const isOpenPhase = (value: unknown): value is OpenPhase =>
	value === 'scheduled' || value === 'live';

const isPhase = (value: unknown): value is Phase => value === 'ended' || isOpenPhase(value);

const postJson = (body: unknown): RequestInit => ({
	method: 'POST',
	headers: { 'Content-Type': 'application/json' },
	body: JSON.stringify(body)
});

/**
 * Logs the owner in: the Worker checks `password` and sets the session cookie. A wrong
 * password is `unauthorized`.
 */
export function login(password: string): Promise<ApiResult<void>> {
	return request('/api/login', postJson({ password }), noContent);
}

/** Ends the owner's session on this device: the Worker clears the cookie. */
export function logout(): Promise<ApiResult<void>> {
	return request('/api/logout', { method: 'POST' }, noContent);
}

/** Whether this browser holds a valid owner session cookie. */
export function isOwner(): Promise<ApiResult<boolean>> {
	return request(
		'/api/me',
		{},
		async (response) => ((await noContent(response)) === null ? null : true),
		(status) => (status === 401 ? false : null)
	);
}

/** Creates a meeting. Needs the owner's session; without it the result is `unauthorized`. */
export function createMeeting(recording: RecordingPolicy): Promise<ApiResult<MeetingId>> {
	return request(
		'/api/new',
		postJson({ recording }),
		json((body) =>
			isObject(body) && typeof body.meeting === 'string' ? parseMeetingId(body.meeting) : null
		)
	);
}

/** A Worker from before the phases sends only `ended`; its unended meetings were all live. */
function pastPhase(phase: unknown, ended: unknown): Phase | null {
	if (phase !== undefined) return isPhase(phase) ? phase : null;
	return typeof ended === 'boolean' ? (ended ? 'ended' : 'live') : null;
}

function parsePastMeeting(value: unknown): PastMeeting | null {
	if (!isObject(value)) return null;
	const { meeting, createdAt, phase, ended, transcript } = value;
	const id = typeof meeting === 'string' ? parseMeetingId(meeting) : null;
	if (id === null || typeof createdAt !== 'number') return null;
	if (transcript !== undefined && (typeof transcript !== 'string' || transcript === '')) {
		return null;
	}
	const known = pastPhase(phase, ended);
	return known === null
		? null
		: { meeting: id, createdAt, phase: known, transcript: transcript ?? null };
}

/** Lists the owner's meetings, newest first. Needs the owner's session. */
export function listMeetings(): Promise<ApiResult<PastMeeting[]>> {
	return request(
		'/api/meetings',
		{},
		json((body) => {
			if (!isObject(body) || !Array.isArray(body.meetings)) return null;
			const meetings = body.meetings.map(parsePastMeeting);
			return meetings.every((m): m is PastMeeting => m !== null)
				? meetings.toSorted((a, b) => b.createdAt - a.createdAt)
				: null;
		})
	);
}

/** Looks a meeting up for the pre-join screen. */
export function getMeeting(meeting: MeetingId): Promise<ApiResult<MeetingInfo>> {
	return request<MeetingInfo>(
		`/api/meeting/${encodeURIComponent(meeting)}`,
		{},
		json((body) => {
			if (!isObject(body) || typeof body.ended !== 'boolean') return null;
			const { phase, transcript } = body;
			if (body.ended || phase === 'ended') {
				return {
					kind: 'ended',
					transcript: typeof transcript === 'string' && transcript !== '' ? transcript : null
				};
			}
			// A Worker from before the phases sends none; its meetings were all live.
			const open = phase === undefined ? 'live' : isOpenPhase(phase) ? phase : null;
			return open !== null && isRecordingPolicy(body.recordingPolicy)
				? { kind: 'open', recordingPolicy: body.recordingPolicy, phase: open }
				: null;
		}),
		(status) => (status === 404 ? { kind: 'not-found' } : null)
	);
}

const isIceServer = (value: unknown): value is RTCIceServer => {
	if (!isObject(value)) return false;
	const { urls, username, credential } = value;
	const urlsOk =
		typeof urls === 'string' || (Array.isArray(urls) && urls.every((u) => typeof u === 'string'));
	return (
		urlsOk &&
		(username === undefined || typeof username === 'string') &&
		(credential === undefined || typeof credential === 'string')
	);
};

/**
 * How long to wait for the ICE servers. The peer connections wait for them, so a stalled
 * request must not hold up the call; the caller falls back to STUN.
 */
export const ICE_TIMEOUT_MS = 4_000;

/**
 * STUN and TURN servers with short-lived TURN credentials, for a participant of `meeting`
 * (the Worker hands TURN credentials only to known meetings). Fails after `ICE_TIMEOUT_MS`.
 */
export function getIceServers(meeting: MeetingId): Promise<ApiResult<RTCIceServer[]>> {
	return request(
		`/api/ice?meeting=${encodeURIComponent(meeting)}`,
		{ signal: AbortSignal.timeout(ICE_TIMEOUT_MS) },
		json((body) =>
			isObject(body) && Array.isArray(body.iceServers) && body.iceServers.every(isIceServer)
				? body.iceServers
				: null
		)
	);
}
