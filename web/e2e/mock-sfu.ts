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
 * Its WebSocket adapters (`adapters/websocket/new` and `close`) behave like Cloudflare's: each
 * one opens a WebSocket to the recorder's endpoint and streams the audio set through
 * `/__control/adapter-audio` in 20 ms protobuf packets, looping, until it is closed. Without
 * audio, an adapter connects and sends nothing. It connects only to `ws://` endpoints, so a
 * Worker configured with the production `RECORDER_ORIGIN` never gets a stream from it.
 *
 * Run it on its own with `node e2e/mock-sfu.ts <port> <app-id> <app-secret>` (port 0: any free one); the `stack`
 * fixture does that and stops it by PID.
 */
import { randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
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

/** One entry of `adapters/websocket/new` (`AdapterTrack` in `wire.rs`): stream a track there. */
export interface AdapterTrack {
	location: 'remote';
	sessionId: string;
	trackName: string;
	endpoint: string;
	outputCodec: 'pcm';
}

/** One call the Worker made, numbered in arrival order from 1. Adapter calls have no session. */
export type SfuCall = { seq: number } & (
	| { op: 'new'; session: null }
	| { op: 'adapters/websocket/new'; session: null; body: { tracks: AdapterTrack[] } }
	| { op: 'adapters/websocket/close'; session: null; body: { tracks: { adapterId: string }[] } }
	| { op: 'tracks/new'; session: string; body: TracksRequest }
	| { op: 'renegotiate'; session: string; body: { sessionDescription: SessionDescription } }
	| { op: 'tracks/close'; session: string; body: { tracks: { mid: string }[]; force: boolean } }
	| { op: 'tracks/update'; session: string; body: { tracks: TrackObject[] } }
);

// ---- The control API

/**
 * `POST /__control/{command}` bodies. `GET /__control/calls?since=n` lists calls after `n`;
 * `GET /__control/adapters` lists the adapters.
 */
export interface ControlCommands {
	/** Clears every fault below and the adapter audio. Sessions, adapters and the call log stay. */
	reset: Record<string, never>;
	/** While down, every SFU call answers 503. */
	down: { down: boolean };
	/** Holds every `tracks/close` this long before answering. */
	'close-delay': { ms: number };
	/** Loses the session of the next pull (`tracks/new` with remote tracks), or a given session. */
	'lose-session': { next: 'pull' } | { session: string };
	/**
	 * Every adapter created afterwards streams this file, raw 48 kHz stereo s16le PCM as the
	 * adapter delivers it, in a loop; `null`: adapters connect and send nothing (the default).
	 */
	'adapter-audio': { file: string | null };
}

export type ControlCommand = keyof ControlCommands;

export interface CallsResponse {
	calls: SfuCall[];
}

/** A WebSocket adapter of the mock: where it streams, how far its socket got, what it sent. */
export interface AdapterView {
	adapterId: string;
	trackName: string;
	endpoint: string;
	/** `refused`: the endpoint closed or failed the socket before it opened. */
	state: 'connecting' | 'open' | 'closed' | 'refused';
	/** Packets sent so far, 20 ms of audio each. */
	packets: number;
}

export interface AdaptersResponse {
	adapters: AdapterView[];
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

// ---- WebSocket adapters

/** One packet's payload: 20 ms of 48 kHz stereo signed 16-bit PCM. */
const FRAME_BYTES = 960 * 4;

/** Protobuf `Packet { uint32 sequenceNumber = 1; uint32 timestamp = 2; bytes payload = 5; }`. */
export function adapterPacket(
	sequenceNumber: number,
	timestamp: number,
	payload: Uint8Array
): Uint8Array<ArrayBuffer> {
	const header: number[] = [];
	const varint = (value: number) => {
		let rest = value;
		while (rest >= 0x80) {
			header.push((rest % 0x80) | 0x80);
			rest = Math.floor(rest / 0x80);
		}
		header.push(rest);
	};
	varint(1 << 3);
	varint(sequenceNumber >>> 0);
	varint(2 << 3);
	varint(timestamp >>> 0);
	varint((5 << 3) | 2);
	varint(payload.length);
	return Buffer.concat([Uint8Array.from(header), payload]);
}

interface Adapter {
	view: AdapterView;
	socket: WebSocket;
}

/** Connects to `track.endpoint` and streams `audio` (if any) in 20 ms packets until closed. */
function startAdapter(adapterId: string, track: AdapterTrack, audio: Uint8Array | null): Adapter {
	const view: AdapterView = {
		adapterId,
		trackName: track.trackName,
		endpoint: track.endpoint,
		state: 'connecting',
		packets: 0
	};
	const socket = new WebSocket(track.endpoint);
	let timer: ReturnType<typeof setInterval> | undefined;
	socket.addEventListener('open', () => {
		view.state = 'open';
		if (audio === null) return;
		let sequence = 1000;
		let offset = 0;
		timer = setInterval(() => {
			if (socket.readyState !== WebSocket.OPEN) return;
			if (offset + FRAME_BYTES > audio.length) offset = 0;
			socket.send(
				adapterPacket(sequence, sequence * 960, audio.subarray(offset, offset + FRAME_BYTES))
			);
			offset += FRAME_BYTES;
			sequence += 1;
			view.packets += 1;
		}, 20);
	});
	socket.addEventListener('close', () => {
		clearInterval(timer);
		view.state = view.state === 'connecting' ? 'refused' : 'closed';
	});
	// A refused upgrade fires `error`, then `close`; the state above records it.
	socket.addEventListener('error', () => {});
	return { view, socket };
}

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

const isAdapterTrack = (value: unknown): value is AdapterTrack =>
	isObject(value) &&
	value.location === 'remote' &&
	isString(value.sessionId) &&
	isString(value.trackName) &&
	isString(value.endpoint) &&
	value.outputCodec === 'pcm';

/** 1 to 4 entries, as the API takes them. */
const isAdapterBatch = <T>(value: unknown, isEntry: (v: unknown) => v is T): value is T[] =>
	Array.isArray(value) && value.length >= 1 && value.length <= 4 && value.every(isEntry);

const isAdapterRef = (value: unknown): value is { adapterId: string } =>
	isObject(value) && isString(value.adapterId);

/** The call `op` on `session` with `body`, or `null` if the body is not what the API takes. */
function toCall(seq: number, op: string, session: string | null, body: unknown): SfuCall | null {
	if (op === 'new') return { seq, op, session: null };
	if (!isObject(body)) return null;
	if (op === 'adapters/websocket/new') {
		return isAdapterBatch(body.tracks, isAdapterTrack)
			? { seq, op, session: null, body: { tracks: body.tracks } }
			: null;
	}
	if (op === 'adapters/websocket/close') {
		return isAdapterBatch(body.tracks, isAdapterRef)
			? {
					seq,
					op,
					session: null,
					body: { tracks: body.tracks.map(({ adapterId }) => ({ adapterId })) }
				}
			: null;
	}
	if (session === null) return null;
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

function parseControl(
	command: string,
	body: unknown
): Partial<Faults> | { lose: string } | { audioFile: string | null } | null {
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
		case 'adapter-audio':
			return body.file === null || isString(body.file) ? { audioFile: body.file } : null;
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

/** The PCM in `file`, whole frames only; at least one frame. */
function audioFrom(file: string): Uint8Array {
	const pcm = new Uint8Array(readFileSync(file));
	const usable = pcm.length - (pcm.length % FRAME_BYTES);
	if (usable === 0) throw new Error(`${file} holds less than 20 ms of audio`);
	return pcm.subarray(0, usable);
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
	const adapters = new Map<string, Adapter>();
	/** What adapters created from now on stream; see `ControlCommands['adapter-audio']`. */
	let adapterAudio: Uint8Array | null = null;

	const sessionPath = new RegExp(`^/v1/apps/${appId}/sessions/(?:new|([0-9a-f]+)/(.+))$`);
	const adapterPath = new RegExp(`^/v1/apps/${appId}/(adapters/websocket/(?:new|close))$`);

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

	/** Starts one adapter per entry; HTTP 503 with the entries' errors if every entry failed. */
	function createAdapters(res: ServerResponse, tracks: AdapterTrack[]): void {
		const results = tracks.map((track) => {
			if (!track.endpoint.startsWith('ws://')) {
				return {
					trackName: track.trackName,
					errorCode: 'invalid_endpoint',
					errorDescription: 'the mock SFU streams only to ws:// endpoints (a local Worker)'
				};
			}
			const adapterId = Buffer.from(random(8)).toString('hex');
			adapters.set(adapterId, startAdapter(adapterId, track, adapterAudio));
			return { adapterId, trackName: track.trackName, endpoint: track.endpoint };
		});
		const failed = results.every((r) => 'errorCode' in r);
		reply(res, failed ? 503 : 200, { tracks: results });
	}

	/** Closes the adapters' sockets; closing an unknown or closed adapter succeeds. */
	function closeAdapters(res: ServerResponse, tracks: { adapterId: string }[]): void {
		for (const { adapterId } of tracks) adapters.get(adapterId)?.socket.close();
		reply(res, 200, { tracks: tracks.map(({ adapterId }) => ({ adapterId })) });
	}

	async function sfuCall(req: IncomingMessage, res: ServerResponse, body: unknown, path: string) {
		const adapterOp = adapterPath.exec(path)?.[1];
		const match = sessionPath.exec(path);
		if (!adapterOp && !match) {
			return reply(res, 404, { errorCode: 'not_found', errorDescription: path });
		}
		const [, sessionId, rest] = match ?? [];
		const call = toCall(
			calls.length + 1,
			adapterOp ?? (sessionId ? (rest ?? '') : 'new'),
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
		if (call.op === 'adapters/websocket/new') return createAdapters(res, call.body.tracks);
		if (call.op === 'adapters/websocket/close') return closeAdapters(res, call.body.tracks);
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
		if (req.method === 'GET' && command === 'adapters') {
			const response: AdaptersResponse = { adapters: [...adapters.values()].map((a) => a.view) };
			return reply(res, 200, response);
		}
		const change = req.method === 'POST' ? parseControl(command, body ?? {}) : null;
		if (change === null) return reply(res, 400, { error: `bad control request ${command}` });
		if ('audioFile' in change) {
			try {
				adapterAudio = change.audioFile === null ? null : audioFrom(change.audioFile);
			} catch (error) {
				return reply(res, 400, { error: String(error) });
			}
		} else if ('lose' in change) {
			const session = sessions.get(change.lose);
			if (!session) return reply(res, 404, { error: `no session ${change.lose}` });
			session.gone = true;
		} else {
			faults = { ...faults, ...change };
			if (command === 'reset') adapterAudio = null;
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
