/**
 * A mock of the Cloudflare Realtime SFU's HTTPS API (`/v1/apps/{appId}/sessions/...`): just
 * enough for the room's calls, with real SDP (see `sdp.ts`) so browsers negotiate against it,
 * and a control API under `/__control/` that tests use to inject faults and read the calls the
 * room made.
 *
 * Like the live SFU when a publisher is not connected yet (docs/spikes/02-first-call.md), the
 * first pull of each published track fails with `not_found_track_error` and still returns an
 * offer; later pulls of that track succeed. A lost session answers that call and every later
 * one with HTTP 410 `session_error`, as the live SFU did on 2026-10-09.
 *
 * Run it on its own with `node e2e/mock-sfu.ts <port> <app-id> <app-secret>` (port 0: any free one); the `stack`
 * fixture does that and stops it by PID.
 */
import { randomBytes } from 'node:crypto';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import {
	answerSdp,
	attribute,
	newTransport,
	offerSdp,
	parseSdp,
	type OfferedTrack,
	type Origin,
	type SdpMediaKind,
	type Transport
} from './sdp.ts';

// ---- The SFU API's request bodies, as `crates/sfu-api/src/wire.rs` sends them

export interface SessionDescription {
	type: 'offer' | 'answer';
	sdp: string;
}

export type TrackObject =
	| { location: 'local'; mid: string; trackName: string }
	| {
			location: 'remote';
			sessionId: string;
			trackName: string;
			mid?: string;
			simulcast?: { preferredRid: string; priorityOrdering?: string; ridNotAvailable?: string };
	  };

export interface TracksRequest {
	sessionDescription?: SessionDescription;
	tracks: TrackObject[];
}

/** One call the room made, numbered in arrival order from 1. */
export type SfuCall = { seq: number } & (
	| { op: 'new'; session: null }
	| { op: 'tracks/new'; session: string; body: TracksRequest }
	| { op: 'renegotiate'; session: string; body: { sessionDescription: SessionDescription } }
	| { op: 'tracks/close'; session: string; body: { tracks: { mid: string }[]; force: boolean } }
	| { op: 'tracks/update'; session: string; body: { tracks: TrackObject[] } }
);

// ---- The control API

/** `POST /__control/{command}` bodies. `GET /__control/calls?since=n` lists calls after `n`. */
export interface ControlCommands {
	/** Clears every fault below. Sessions and the call log stay. */
	reset: Record<string, never>;
	/** While down, every SFU call answers 503. */
	down: { down: boolean };
	/** Holds every `tracks/close` this long before answering. */
	'close-delay': { ms: number };
	/** Loses the session of the next pull (`tracks/new` with remote tracks), or a given session. */
	'lose-session': { next: 'pull' } | { session: string };
}

export type ControlCommand = keyof ControlCommands;

export interface CallsResponse {
	calls: SfuCall[];
}

// ---- State

interface Session {
	transport: Transport;
	origin: Origin;
	/** Kinds of the tracks this session published, by track name. */
	published: Map<string, SdpMediaKind>;
	/** Sections of the offers this session received, in order; they only ever grow. */
	offered: OfferedTrack[];
	gone: boolean;
}

interface Faults {
	down: boolean;
	closeDelayMs: number;
	loseNextPull: boolean;
}

const NO_FAULTS: Faults = { down: false, closeDelayMs: 0, loseNextPull: false };

const NOT_FOUND = {
	errorCode: 'not_found_track_error',
	errorDescription:
		'Track not found on remote peer. Make sure the publisher peer is connected and sending packets for this track'
} as const;

const SESSION_GONE = {
	errorCode: 'session_error',
	errorDescription:
		'Session appears to be disconnected. Please check if the PeerConnection is connected.'
} as const;

const random = (bytes: number) => new Uint8Array(randomBytes(bytes));

// ---- Validation of what arrives: the room's JSON, and the tests' control commands

type Json = Record<string, unknown>;
const isObject = (value: unknown): value is Json =>
	typeof value === 'object' && value !== null && !Array.isArray(value);
const isString = (value: unknown): value is string => typeof value === 'string';

function isSessionDescription(value: unknown): value is SessionDescription {
	return (
		isObject(value) && (value.type === 'offer' || value.type === 'answer') && isString(value.sdp)
	);
}

function isTrackObject(value: unknown): value is TrackObject {
	if (!isObject(value) || !isString(value.trackName)) return false;
	if (value.location === 'local') return isString(value.mid);
	return value.location === 'remote' && isString(value.sessionId);
}

const isTrackList = (value: unknown): value is TrackObject[] =>
	Array.isArray(value) && value.every(isTrackObject);

/** The call `op` on `session` with `body`, or `null` if the body is not what the API takes. */
function toCall(seq: number, op: string, session: string | null, body: unknown): SfuCall | null {
	if (op === 'new') return { seq, op, session: null };
	if (session === null || !isObject(body)) return null;
	switch (op) {
		case 'tracks/new':
			return isTrackList(body.tracks) &&
				(body.sessionDescription === undefined || isSessionDescription(body.sessionDescription))
				? {
						seq,
						op,
						session,
						body: {
							tracks: body.tracks,
							...(body.sessionDescription ? { sessionDescription: body.sessionDescription } : {})
						}
					}
				: null;
		case 'renegotiate':
			return isSessionDescription(body.sessionDescription)
				? { seq, op, session, body: { sessionDescription: body.sessionDescription } }
				: null;
		case 'tracks/close': {
			const { tracks, force } = body;
			return Array.isArray(tracks) &&
				tracks.every((t) => isObject(t) && isString(t.mid)) &&
				typeof force === 'boolean'
				? {
						seq,
						op,
						session,
						body: { tracks: tracks.map((t: Json) => ({ mid: String(t.mid) })), force }
					}
				: null;
		}
		case 'tracks/update':
			return isTrackList(body.tracks) ? { seq, op, session, body: { tracks: body.tracks } } : null;
		default:
			return null;
	}
}

function parseControl(command: string, body: unknown): Partial<Faults> | { lose: string } | null {
	if (!isObject(body)) return null;
	switch (command) {
		case 'reset':
			return NO_FAULTS;
		case 'down':
			return typeof body.down === 'boolean' ? { down: body.down } : null;
		case 'close-delay':
			return typeof body.ms === 'number' && body.ms >= 0 ? { closeDelayMs: body.ms } : null;
		case 'lose-session':
			if (body.next === 'pull') return { loseNextPull: true };
			return isString(body.session) ? { lose: body.session } : null;
		default:
			return null;
	}
}

/** The JSON in `text`: `null` for an empty body, `undefined` if it is not JSON. */
function parseJson(text: string): unknown {
	if (text === '') return null;
	try {
		return JSON.parse(text);
	} catch {
		return undefined;
	}
}

// ---- The server

export interface MockSfuOptions {
	appId: string;
	appSecret: string;
}

export function createMockSfu({ appId, appSecret }: MockSfuOptions) {
	const sessions = new Map<string, Session>();
	const calls: SfuCall[] = [];
	/** Published tracks pulled at least once, as `publisherSession/trackName`. */
	const pulledOnce = new Set<string>();
	let faults: Faults = NO_FAULTS;

	const sfuPath = new RegExp(`^/v1/apps/${appId}/sessions/(?:new|([0-9a-f]+)/(.+))$`);

	function reply(res: ServerResponse, status: number, value: unknown): void {
		res.writeHead(status, { 'content-type': 'application/json' });
		res.end(JSON.stringify(value));
	}

	function describe(session: Session, tracks: OfferedTrack[]): string {
		session.origin = { ...session.origin, version: session.origin.version + 1 };
		return offerSdp(tracks, session.transport, session.origin);
	}

	/** Publishes the offer's tracks and answers it. */
	function publish(res: ServerResponse, session: Session, request: TracksRequest): void {
		const offer = request.sessionDescription && parseSdp(request.sessionDescription.sdp);
		if (!offer || request.sessionDescription?.type !== 'offer') {
			return reply(res, 400, {
				errorCode: 'invalid_session_description',
				errorDescription: 'tracks/new with local tracks needs an SDP offer'
			});
		}
		const results = request.tracks.map((track) => {
			const section = offer.media.find((m) => attribute(m.attributes, 'mid') === track.mid);
			if (
				track.location !== 'local' ||
				!section ||
				(section.media !== 'audio' && section.media !== 'video')
			) {
				return {
					...track,
					errorCode: 'invalid_track',
					errorDescription: 'no such mid in the offer'
				};
			}
			session.published.set(track.trackName, section.media);
			return { mid: track.mid, trackName: track.trackName };
		});
		session.origin = { ...session.origin, version: session.origin.version + 1 };
		reply(res, 200, {
			requiresImmediateRenegotiation: false,
			tracks: results,
			sessionDescription: {
				type: 'answer',
				sdp: answerSdp(offer, session.transport, session.origin)
			}
		});
	}

	/** Adds the pulled tracks to the session's next offer; a track's first pull fails. */
	function pull(res: ServerResponse, session: Session, request: TracksRequest): void {
		const results = request.tracks.map((track) => {
			if (track.location !== 'remote') {
				return { ...track, errorCode: 'invalid_track', errorDescription: 'mixed local and remote' };
			}
			const key = `${track.sessionId}/${track.trackName}`;
			const kind = sessions.get(track.sessionId)?.published.get(track.trackName);
			const found = kind !== undefined && pulledOnce.has(key);
			pulledOnce.add(key);
			// A track that is not found still gets an inactive section, like on the live SFU: an
			// offer needs at least one, as Chromium refuses an offer without a BUNDLE group.
			const mid = String(session.offered.length);
			session.offered.push({
				mid,
				kind: kind ?? 'audio',
				direction: found ? 'sendonly' : 'inactive',
				ssrc: random(4).reduce((n, b) => n * 256 + b, 0) || 1,
				stream: `${track.sessionId}-${track.trackName}`,
				// Unique per section: a browser refuses two sections with the same `a=msid`.
				track: `${track.trackName}-${mid}`
			});
			return found
				? { sessionId: track.sessionId, trackName: track.trackName, mid }
				: { sessionId: track.sessionId, trackName: track.trackName, ...NOT_FOUND };
		});
		reply(res, 200, {
			requiresImmediateRenegotiation: true,
			tracks: results,
			sessionDescription: { type: 'offer', sdp: describe(session, session.offered) }
		});
	}

	async function sfuCall(req: IncomingMessage, res: ServerResponse, body: unknown, path: string) {
		const match = sfuPath.exec(path);
		if (!match) return reply(res, 404, { errorCode: 'not_found', errorDescription: path });
		const [, sessionId, rest] = match;
		const call = toCall(
			calls.length + 1,
			sessionId ? (rest ?? '') : 'new',
			sessionId ?? null,
			body
		);
		if (!call) {
			return reply(res, 400, {
				errorCode: 'bad_request',
				errorDescription: `${req.method} ${path}`
			});
		}
		calls.push(call);
		if (req.headers.authorization !== `Bearer ${appSecret}`) {
			return reply(res, 401, { errorCode: 'unauthorized', errorDescription: 'bad app secret' });
		}
		if (faults.down) {
			return reply(res, 503, {
				errorCode: 'temporarily_unavailable_error',
				errorDescription: 'mock SFU is down'
			});
		}
		if (call.op === 'new') {
			const id = Buffer.from(random(16)).toString('hex');
			sessions.set(id, {
				transport: newTransport(random),
				origin: { id: String(random(4).reduce((n, b) => n * 256 + b, 0)), version: 1 },
				published: new Map(),
				offered: [],
				gone: false
			});
			return reply(res, 201, { sessionId: id });
		}
		const session = sessions.get(call.session);
		if (!session)
			return reply(res, 404, { errorCode: 'not_found', errorDescription: 'no such session' });
		if (
			faults.loseNextPull &&
			call.op === 'tracks/new' &&
			call.body.tracks[0]?.location === 'remote'
		) {
			faults = { ...faults, loseNextPull: false };
			session.gone = true;
		}
		if (session.gone) return reply(res, 410, SESSION_GONE);
		switch (call.op) {
			case 'tracks/new':
				return call.body.tracks[0]?.location === 'local'
					? publish(res, session, call.body)
					: pull(res, session, call.body);
			case 'renegotiate':
				return reply(res, 200, {});
			case 'tracks/close': {
				await new Promise((resolve) => setTimeout(resolve, faults.closeDelayMs));
				const closed = new Set(call.body.tracks.map((t) => t.mid));
				session.offered = session.offered.map((t) =>
					closed.has(t.mid) ? { ...t, direction: 'inactive' } : t
				);
				return reply(res, 200, { requiresImmediateRenegotiation: false, tracks: call.body.tracks });
			}
			case 'tracks/update':
				return reply(res, 200, { requiresImmediateRenegotiation: false, tracks: call.body.tracks });
		}
	}

	function control(req: IncomingMessage, res: ServerResponse, body: unknown, url: URL) {
		const command = url.pathname.slice('/__control/'.length);
		if (req.method === 'GET' && command === 'calls') {
			const since = Number(url.searchParams.get('since') ?? 0);
			const response: CallsResponse = { calls: calls.filter((c) => c.seq > since) };
			return reply(res, 200, response);
		}
		const change = req.method === 'POST' ? parseControl(command, body ?? {}) : null;
		if (change === null) return reply(res, 400, { error: `bad control request ${command}` });
		if ('lose' in change) {
			const session = sessions.get(change.lose);
			if (!session) return reply(res, 404, { error: `no session ${change.lose}` });
			session.gone = true;
		} else {
			faults = { ...faults, ...change };
		}
		res.writeHead(204).end();
	}

	return createServer(async (req, res) => {
		let text = '';
		for await (const chunk of req) text += String(chunk);
		const body = parseJson(text);
		if (body === undefined) {
			return reply(res, 400, { errorCode: 'bad_request', errorDescription: 'body is not JSON' });
		}
		const url = new URL(req.url ?? '/', 'http://mock-sfu');
		if (url.pathname.startsWith('/__control/')) return control(req, res, body, url);
		return sfuCall(req, res, body, url.pathname);
	});
}

if (import.meta.main) {
	const [port, appId, appSecret] = process.argv.slice(2);
	if (!port || !appId || !appSecret) {
		console.error('usage: node e2e/mock-sfu.ts <port> <app-id> <app-secret>');
		process.exit(2);
	}
	// Port 0 picks a free port; the line below tells the caller which.
	const server = createMockSfu({ appId, appSecret }).listen(Number(port), '127.0.0.1', () => {
		const address = server.address();
		const bound = typeof address === 'object' && address !== null ? address.port : port;
		console.log(`mock SFU listening on http://127.0.0.1:${bound}`);
	});
}
