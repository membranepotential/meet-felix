/**
 * The Worker's HTTP API as the tests use it: thin wrappers that return the raw `Response`, so
 * tests can check statuses and headers, plus `loginAsOwner` and `createMeeting` for setup.
 * The wire types below are what the Worker sends; tests assert them field by field.
 */
import { randomBytes } from 'node:crypto';
import { request } from 'node:http';
import type {
	MeetingId,
	Millis,
	Phase,
	RecordingPolicy,
	TranscriptRef
} from '../../src/lib/protocol.ts';
import {
	parseTranscriptStatus,
	type Progress,
	type TranscriptStatus
} from '../../src/lib/transcript.ts';

/** Request headers that authenticate the owner: the session cookie or the bearer secret. */
export type Auth = Readonly<Record<string, string>>;

export const NO_AUTH: Auth = {};
export const bearer = (secret: string): Auth => ({ authorization: `Bearer ${secret}` });

/** The owner's session: the `Cookie` header and the cookie's value. */
export interface OwnerSession {
	headers: Auth;
	value: string;
}

/** `GET /api/meeting/{words}`. */
export interface MeetingInfoWire {
	recordingPolicy: RecordingPolicy;
	phase: Phase;
	ended: boolean;
	transcript?: TranscriptRef;
}

/** One entry of `GET /api/meetings`. */
export interface ListedMeetingWire {
	meeting: MeetingId;
	createdAt: Millis;
	phase: Phase;
	ended: boolean;
	transcript?: TranscriptRef;
}

/**
 * `GET /api/transcript/{token}` for a known token: the transcript page's `TranscriptStatus`, and
 * the progress the Worker sends with every status (the page reads it only while the job runs).
 * Read it with `transcriptStatusOf`.
 */
export type TranscriptStatusWire = Exclude<TranscriptStatus, { status: 'not-found' }> & {
	progress: Progress;
};

/** The downloads of a ready transcript. */
export type TranscriptFile = 'transcript.md' | 'transcript.json';

export interface IceWire {
	iceServers: { urls: string[]; username?: string; credential?: string }[];
}

/** The body of a JSON response, typed as the endpoint promises; the caller asserts it. */
export const body = async <T>(response: Response): Promise<T> => (await response.json()) as T;

/** A transcript status, checked by the transcript page's own parser; throws if the page could not read it. */
export async function transcriptStatusOf(response: Response): Promise<TranscriptStatusWire> {
	const raw: unknown = await response.json();
	const status = parseTranscriptStatus(raw);
	if (status === null || status.status === 'not-found') {
		throw new Error(`the transcript page cannot read this status: ${JSON.stringify(raw)}`);
	}
	return { ...status, progress: (raw as { progress: Progress }).progress };
}

/** The `Cookie` request header for a response's first `Set-Cookie`, or "" if there is none. */
export const cookieOf = (response: Response): string =>
	response.headers.getSetCookie()[0]?.split(';')[0] ?? '';

export class Api {
	readonly baseUrl: string;

	constructor(baseUrl: string) {
		this.baseUrl = baseUrl;
	}

	#fetch(path: string, init?: RequestInit): Promise<Response> {
		return fetch(`${this.baseUrl}${path}`, init);
	}

	login(password: string): Promise<Response> {
		return this.loginWith(JSON.stringify({ password }));
	}

	loginWith(rawBody: string): Promise<Response> {
		return this.#fetch('/api/login', { method: 'POST', body: rawBody });
	}

	me(auth: Auth): Promise<Response> {
		return this.#fetch('/api/me', { headers: auth });
	}

	logout(): Promise<Response> {
		return this.#fetch('/api/logout', { method: 'POST' });
	}

	newMeeting(auth: Auth, settings?: { recording: RecordingPolicy }): Promise<Response> {
		return this.#fetch('/api/new', {
			method: 'POST',
			headers: auth,
			...(settings ? { body: JSON.stringify(settings) } : {})
		});
	}

	meeting(words: string): Promise<Response> {
		return this.#fetch(`/api/meeting/${words}`);
	}

	meetings(auth: Auth): Promise<Response> {
		return this.#fetch('/api/meetings', { headers: auth });
	}

	/** `GET /api/ice`, with `?meeting=` unless `meeting` is `null`. */
	ice(meeting: string | null): Promise<Response> {
		return this.#fetch(meeting === null ? '/api/ice' : `/api/ice?meeting=${meeting}`);
	}

	transcript(token: TranscriptRef): Promise<Response> {
		return this.#fetch(`/api/transcript/${token}`);
	}

	transcriptFile(token: TranscriptRef, file: TranscriptFile): Promise<Response> {
		return this.#fetch(`/api/transcript/${token}/${file}`);
	}

	/** The transcript page `/t/{token}`, a static asset of the web app. */
	transcriptPage(token: TranscriptRef): Promise<Response> {
		return this.#fetch(`/t/${token}`);
	}

	/** A plain GET of the socket path, without the upgrade headers. */
	socketWithoutUpgrade(meeting: MeetingId): Promise<Response> {
		return this.#fetch(`/ws/${meeting}`);
	}

	/** The status of a WebSocket upgrade request to the room: 101 if accepted. */
	upgradeStatus(meeting: MeetingId): Promise<number> {
		return upgradeStatus(`${this.baseUrl}/ws/${meeting}`);
	}
}

/**
 * The status of a WebSocket upgrade request to `url` (`http:` or `ws:`): 101 if accepted.
 * (`fetch` forbids `Upgrade`.)
 */
export function upgradeStatus(url: string): Promise<number> {
	return new Promise((resolve, reject) => {
		const req = request(url.replace(/^ws/, 'http'), {
			headers: {
				connection: 'Upgrade',
				upgrade: 'websocket',
				'sec-websocket-version': '13',
				'sec-websocket-key': randomBytes(16).toString('base64')
			}
		});
		req.on('upgrade', (_response, socket) => {
			socket.destroy();
			resolve(101);
		});
		req.on('response', (response) => {
			response.resume();
			resolve(response.statusCode ?? 0);
		});
		req.on('error', reject);
		req.setTimeout(5_000, () => req.destroy(new Error('upgrade request timed out')));
		req.end();
	});
}

/** Logs in through `POST /api/login` and returns the owner's session cookie. */
export async function loginAsOwner(api: Api, password: string): Promise<OwnerSession> {
	const response = await api.login(password);
	const cookie = cookieOf(response);
	if (response.status !== 200 || !cookie.startsWith('owner=')) {
		throw new Error(`owner login failed: ${response.status}`);
	}
	return { headers: { cookie }, value: cookie.slice('owner='.length) };
}

/** Creates a meeting through `POST /api/new` and returns its words. */
export async function createMeeting(
	api: Api,
	auth: Auth,
	recording: RecordingPolicy = 'fromStart'
): Promise<MeetingId> {
	const response = await api.newMeeting(auth, { recording });
	if (response.status !== 201) throw new Error(`creating a meeting failed: ${response.status}`);
	return (await body<{ meeting: MeetingId }>(response)).meeting;
}
