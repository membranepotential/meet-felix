/**
 * HTTP API of the Worker (`/api/*`). The request and response bodies are generated from
 * `crates/meeting-core/src/http.rs` by tsify, and every response is read here with the core's
 * serde codec (`parse…`), which throws on anything else; the rest of the app works with the
 * typed results.
 */
import {
	parseErrorBody,
	parseListing,
	parseMeetingLookup,
	parseNewMeeting,
	parseOwner,
	parseTranscriptStatus,
	type ApiErrorCode,
	type LoginRequest,
	type MeetingLookup,
	type NewMeetingRequest,
	type Owner,
	type PastMeeting,
	type TranscriptStatus
} from './wasm/meeting_core.js';
import type { MeetingId, Phase, RecordingPolicy, TranscriptRef } from './protocol.ts';

export type { ApiErrorCode, Owner, PastMeeting, TranscriptStatus };

/** What `GET /api/me` finds in this browser: the owner's valid session, or none. */
export type SessionCheck = { kind: 'owner'; owner: Owner } | { kind: 'none' };

/** A meeting that can still be joined: waiting for the host, or running. */
export type OpenPhase = Exclude<Phase, 'ended'>;

/** What the pre-join screen needs to know about a meeting. */
export type MeetingInfo =
	| { kind: 'open'; recordingPolicy: RecordingPolicy; phase: OpenPhase }
	/** No transcript link: only the participants get it when the meeting ends (D19). */
	| { kind: 'ended' }
	| { kind: 'not-found' };

/** A transcript link's status, or `not-found` for a token the Worker does not know. */
export type TranscriptLookup = TranscriptStatus | { status: 'not-found' };

export type ApiError =
	| { kind: 'network' }
	| { kind: 'unauthorized' }
	/** `retryAfterMs`: when to try again, from the Worker's `Retry-After`; `null` without one. */
	| { kind: 'rate-limited'; retryAfterMs: number | null }
	/**
	 * Any other refusal or failure. `code` is the Worker's error code; `null` if the answer is
	 * not the Worker's error body: another server answered, or a Worker without the route.
	 */
	| { kind: 'server'; status: number; code: ApiErrorCode | null }
	| { kind: 'malformed' };

export type ApiResult<T> = { ok: true; value: T } | { ok: false; error: ApiError };

const fail = (error: ApiError): ApiResult<never> => ({ ok: false, error });

/** Reads a successful response; `null` means the reply is not what the endpoint promises. */
type Reader<T> = (response: Response) => Promise<T | null>;

/** A JSON body, read by `parse`, which throws if it is not what the endpoint promises. */
const json =
	<T>(parse: (json: string) => T): Reader<T> =>
	async (response) => {
		try {
			return parse(await response.text());
		} catch {
			return null;
		}
	};

/**
 * No body: only 204 counts. A 200 here is not this endpoint answering, for example the
 * static app's fallback page served for an `/api` route the Worker does not have yet.
 */
const noContent: Reader<void> = async (response) => (response.status === 204 ? undefined : null);

/** `Retry-After` in milliseconds, if it is a number of seconds, as the Worker sends it. */
function retryAfter(response: Response): number | null {
	const value = response.headers.get('Retry-After')?.trim() ?? '';
	return /^\d+$/.test(value) ? Number(value) * 1000 : null;
}

/** The code in the Worker's error body, or `null` if the body is not one. */
async function errorCode(response: Response): Promise<ApiErrorCode | null> {
	try {
		return parseErrorBody(await response.text()).error;
	} catch {
		return null;
	}
}

/**
 * Fetches `path` and reads a 2xx response with `read`. `onError` may turn one of the Worker's
 * error codes into a value (such as `notFound` → not found) before it counts as an error.
 * Requests stay same-origin (in development through Vite's proxy), so the browser sends the
 * owner's session cookie with the default `credentials: 'same-origin'`.
 */
async function request<T>(
	path: string,
	init: RequestInit,
	read: Reader<T>,
	onError: (code: ApiErrorCode) => T | null = () => null
): Promise<ApiResult<T>> {
	let response: Response;
	try {
		response = await fetch(path, init);
	} catch {
		return fail({ kind: 'network' });
	}
	if (!response.ok) {
		const code = await errorCode(response);
		const value = code === null ? null : onError(code);
		if (value !== null) return { ok: true, value };
		switch (code) {
			case 'unauthorized':
			case 'crossOrigin':
				return fail({ kind: 'unauthorized' });
			case 'rateLimited':
				return fail({ kind: 'rate-limited', retryAfterMs: retryAfter(response) });
			default:
				return fail({ kind: 'server', status: response.status, code });
		}
	}
	const value = await read(response);
	return value === null ? fail({ kind: 'malformed' }) : { ok: true, value };
}

const postJson = <T>(body: T): RequestInit => ({
	method: 'POST',
	headers: { 'Content-Type': 'application/json' },
	body: JSON.stringify(body)
});

/**
 * Logs the owner in: the Worker checks `password`, sets the session cookie and describes the
 * owner. A wrong password is `unauthorized`.
 */
export function login(password: string): Promise<ApiResult<Owner>> {
	return request('/api/login', postJson<LoginRequest>({ password }), json(parseOwner));
}

/** Ends the owner's session on this device: the Worker clears the cookie. */
export function logout(): Promise<ApiResult<void>> {
	return request('/api/logout', { method: 'POST' }, noContent);
}

/** Whether this browser holds a valid owner session cookie, and if so, who the owner is. */
export function checkSession(): Promise<ApiResult<SessionCheck>> {
	return request<SessionCheck>(
		'/api/me',
		{},
		json((body) => ({ kind: 'owner', owner: parseOwner(body) })),
		(code) => (code === 'unauthorized' ? { kind: 'none' } : null)
	);
}

/** Creates a meeting. Needs the owner's session; without it the result is `unauthorized`. */
export function createMeeting(recording: RecordingPolicy): Promise<ApiResult<MeetingId>> {
	return request(
		'/api/new',
		postJson<NewMeetingRequest>({ recording }),
		json((body) => parseNewMeeting(body).meeting)
	);
}

/** Lists the owner's meetings, newest first. Needs the owner's session. */
export function listMeetings(): Promise<ApiResult<PastMeeting[]>> {
	return request(
		'/api/meetings',
		{},
		json((body) => parseListing(body).meetings)
	);
}

const meetingInfo = (lookup: MeetingLookup): MeetingInfo =>
	lookup.phase === 'ended'
		? { kind: 'ended' }
		: { kind: 'open', recordingPolicy: lookup.recordingPolicy, phase: lookup.phase };

/** Looks a meeting up for the pre-join screen. */
export function getMeeting(meeting: MeetingId): Promise<ApiResult<MeetingInfo>> {
	return request<MeetingInfo>(
		`/api/meeting/${encodeURIComponent(meeting)}`,
		{},
		json((body) => meetingInfo(parseMeetingLookup(body))),
		(code) => (code === 'notFound' ? { kind: 'not-found' } : null)
	);
}

const transcriptPath = (token: TranscriptRef) => `/api/transcript/${encodeURIComponent(token)}`;

/** The transcript behind a link token, or where its job stands. */
export function getTranscript(token: TranscriptRef): Promise<ApiResult<TranscriptLookup>> {
	return request<TranscriptLookup>(
		transcriptPath(token),
		{},
		json(parseTranscriptStatus),
		(code) => (code === 'notFound' ? { status: 'not-found' } : null)
	);
}

/** The download link of a transcript file. */
export const transcriptFile = (token: TranscriptRef, format: 'md' | 'json'): string =>
	`${transcriptPath(token)}/transcript.${format}`;

const isObject = (value: unknown): value is Record<string, unknown> =>
	typeof value === 'object' && value !== null;

/** An `RTCIceServer`, the browser's type, which the core does not declare. */
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

/** The ICE servers the Worker hands out. */
export interface IceConfig {
	servers: RTCIceServer[];
	/**
	 * How long the TURN credentials among `servers` stay valid from now, in milliseconds;
	 * `null` with STUN only, or from a Worker that does not say.
	 */
	ttlMs: number | null;
}

/**
 * STUN and TURN servers with short-lived TURN credentials, for a participant of `meeting`
 * (the Worker hands TURN credentials only to known meetings). Fails after `ICE_TIMEOUT_MS`.
 */
export function getIceServers(meeting: MeetingId): Promise<ApiResult<IceConfig>> {
	return request(
		`/api/ice?meeting=${encodeURIComponent(meeting)}`,
		{ signal: AbortSignal.timeout(ICE_TIMEOUT_MS) },
		async (response) => {
			const body: unknown = await response.json().catch(() => null);
			if (!isObject(body)) return null;
			const { iceServers, ttl } = body;
			const servers =
				Array.isArray(iceServers) && iceServers.every(isIceServer) ? iceServers : null;
			if (servers === null) return null;
			if (ttl === undefined) return { servers, ttlMs: null };
			return typeof ttl === 'number' && ttl > 0 ? { servers, ttlMs: ttl * 1000 } : null;
		}
	);
}
