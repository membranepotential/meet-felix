/** HTTP API of the Worker (`/api/*`). The host route sits behind Cloudflare Access. */
import { parseMeetingId, type MeetingId } from './protocol.ts';

export interface NewMeeting {
	meeting: MeetingId;
	/** Proves host rights when joining; sent as `host_token` in `ClientMsg::Join`. */
	hostToken: string;
}

export type ApiError =
	| { kind: 'network' }
	| { kind: 'unauthorized' }
	| { kind: 'rate-limited' }
	| { kind: 'server'; status: number }
	| { kind: 'malformed' };

export type ApiResult<T> = { ok: true; value: T } | { ok: false; error: ApiError };

/**
 * Creates a meeting. Expects `{ "meeting": "a-b-c", "host_token": "..." }` from the Worker.
 */
export async function createMeeting(): Promise<ApiResult<NewMeeting>> {
	let response: Response;
	try {
		response = await fetch('/api/new', { method: 'POST' });
	} catch {
		return { ok: false, error: { kind: 'network' } };
	}
	if (response.status === 401 || response.status === 403) {
		return { ok: false, error: { kind: 'unauthorized' } };
	}
	if (response.status === 429) return { ok: false, error: { kind: 'rate-limited' } };
	if (!response.ok) return { ok: false, error: { kind: 'server', status: response.status } };

	const body: unknown = await response.json().catch(() => null);
	const parsed = parseNewMeeting(body);
	return parsed ? { ok: true, value: parsed } : { ok: false, error: { kind: 'malformed' } };
}

function parseNewMeeting(body: unknown): NewMeeting | null {
	if (typeof body !== 'object' || body === null) return null;
	if (!('meeting' in body) || !('host_token' in body)) return null;
	const { meeting, host_token } = body;
	if (typeof meeting !== 'string' || typeof host_token !== 'string' || host_token === '') {
		return null;
	}
	const id = parseMeetingId(meeting);
	return id ? { meeting: id, hostToken: host_token } : null;
}
