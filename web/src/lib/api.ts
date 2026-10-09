/**
 * HTTP API of the Worker (`/api/*`). Every response is checked here, at the boundary; the
 * rest of the app works with the typed results.
 */
import {
	parseMeetingId,
	type HostToken,
	type MeetingId,
	type Millis,
	type RecordingPolicy,
	type TranscriptRef
} from './protocol.ts';

export interface NewMeeting {
	meeting: MeetingId;
	/** Makes the creator a host; sent as `hostToken` in `ClientMsg::Join`. */
	hostToken: HostToken;
}

/** One of the owner's meetings, as `GET /api/meetings` lists it. */
export interface PastMeeting {
	meeting: MeetingId;
	/** When the meeting was created, in milliseconds since the Unix epoch. */
	createdAt: Millis;
	ended: boolean;
	/** The transcript link token; `null` if the meeting was not recorded. */
	transcript: TranscriptRef | null;
}

/** What the pre-join screen needs to know about a meeting. */
export type MeetingInfo =
	{ kind: 'running'; recordingPolicy: RecordingPolicy } | { kind: 'ended' } | { kind: 'not-found' };

export type ApiError =
	| { kind: 'network' }
	| { kind: 'unauthorized' }
	| { kind: 'rate-limited' }
	| { kind: 'server'; status: number }
	| { kind: 'malformed' };

export type ApiResult<T> = { ok: true; value: T } | { ok: false; error: ApiError };

const fail = (error: ApiError): ApiResult<never> => ({ ok: false, error });

/**
 * Fetches `path` and parses a JSON body with `parse`. `onStatus` may turn a non-2xx status
 * into a value (such as 404 → not found) before it counts as an error.
 */
async function request<T>(
	path: string,
	init: RequestInit,
	parse: (body: unknown) => T | null,
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
	const body: unknown = await response.json().catch(() => null);
	const value = parse(body);
	return value === null ? fail({ kind: 'malformed' }) : { ok: true, value };
}

const isObject = (value: unknown): value is Record<string, unknown> =>
	typeof value === 'object' && value !== null;

const isRecordingPolicy = (value: unknown): value is RecordingPolicy =>
	value === 'fromStart' || value === 'hostDecides';

/** Creates a meeting. Only the owner can: `secret` is the Worker's create secret. */
export function createMeeting(
	secret: string,
	recording: RecordingPolicy
): Promise<ApiResult<NewMeeting>> {
	return request(
		'/api/new',
		{
			method: 'POST',
			headers: { ...bearer(secret), 'Content-Type': 'application/json' },
			body: JSON.stringify({ recording })
		},
		(body) => {
			if (!isObject(body)) return null;
			const { meeting, hostToken } = body;
			if (typeof meeting !== 'string' || typeof hostToken !== 'string' || hostToken === '') {
				return null;
			}
			const id = parseMeetingId(meeting);
			return id ? { meeting: id, hostToken } : null;
		}
	);
}

const bearer = (secret: string): HeadersInit => ({ Authorization: `Bearer ${secret}` });

function parsePastMeeting(value: unknown): PastMeeting | null {
	if (!isObject(value)) return null;
	const { meeting, createdAt, ended, transcript } = value;
	const id = typeof meeting === 'string' ? parseMeetingId(meeting) : null;
	if (id === null || typeof createdAt !== 'number' || typeof ended !== 'boolean') return null;
	if (transcript !== undefined && (typeof transcript !== 'string' || transcript === '')) {
		return null;
	}
	return { meeting: id, createdAt, ended, transcript: transcript ?? null };
}

/**
 * Lists the owner's meetings, newest first. Only the owner can: `secret` is the Worker's
 * create secret.
 */
export function listMeetings(secret: string): Promise<ApiResult<PastMeeting[]>> {
	return request('/api/meetings', { headers: bearer(secret) }, (body) => {
		if (!isObject(body) || !Array.isArray(body.meetings)) return null;
		const meetings = body.meetings.map(parsePastMeeting);
		return meetings.every((m): m is PastMeeting => m !== null)
			? meetings.toSorted((a, b) => b.createdAt - a.createdAt)
			: null;
	});
}

/** Looks a meeting up for the pre-join screen. */
export function getMeeting(meeting: MeetingId): Promise<ApiResult<MeetingInfo>> {
	return request<MeetingInfo>(
		`/api/meeting/${encodeURIComponent(meeting)}`,
		{},
		(body) => {
			if (!isObject(body) || typeof body.ended !== 'boolean') return null;
			if (body.ended) return { kind: 'ended' };
			return isRecordingPolicy(body.recordingPolicy)
				? { kind: 'running', recordingPolicy: body.recordingPolicy }
				: null;
		},
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

/** STUN and TURN servers with short-lived TURN credentials. */
export function getIceServers(): Promise<ApiResult<RTCIceServer[]>> {
	return request('/api/ice', {}, (body) =>
		isObject(body) && Array.isArray(body.iceServers) && body.iceServers.every(isIceServer)
			? body.iceServers
			: null
	);
}
