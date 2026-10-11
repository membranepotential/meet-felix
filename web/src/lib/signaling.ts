/**
 * WebSocket client for the Room Durable Object at `/ws/{meeting}`.
 *
 * Reconnects with exponential backoff and jitter after unexpected closes, and retries at
 * once when the page becomes visible or the network returns (iOS drops sockets of
 * backgrounded tabs). After every (re)connect `onOpen` runs, where the caller sends `Join`;
 * the server answers with `Welcome` and a full state, so no message replay is needed. The
 * backoff starts over once the room placed us, in the meeting (`Welcome`) or outside it
 * (`Waiting`), or once the socket stayed open for `STABLE_MS` without an error from the room
 * (a guest who waits at the consent question gets only `ConsentRequired`). So a room that
 * refuses the join and closes the socket (the SFU is down) is asked less and less often. The
 * room's close code says whether to come back at all (`afterClose`).
 *
 * While open, the client sends `{"type":"ping"}` every 30 s: the room closes a socket with code
 * 1001 after about 60 s without one, and answers `{"type":"pong"}` without waking its Durable
 * Object. Neither frame is part of the protocol: they are matched as exact strings.
 *
 * Every other frame goes through the core's codec (`parseServerMsg`, `encodeClientMsg`), so
 * the room and the client agree on one definition of each message.
 */
import {
	afterClose,
	encodeClientMsg,
	parseServerMsg,
	type AfterClose,
	type ClientMsg,
	type MeetingId,
	type ServerMsg
} from './protocol.ts';

export type ConnectionStatus =
	| { kind: 'connecting'; attempt: number }
	| { kind: 'open' }
	/** Waiting `retryInMs` before the next attempt. */
	| { kind: 'waiting'; attempt: number; retryInMs: number }
	/** Closed by us (`close()`). */
	| { kind: 'closed' }
	/**
	 * The room closed the socket for good: the stay is over (`stop`), or this page is out of
	 * date (`reload`). `reason` is the close frame's, meant for the user.
	 */
	| { kind: 'dismissed'; after: Exclude<AfterClose, 'reconnect'>; reason: string };

export interface SignalingHandlers {
	onOpen: () => void;
	onMessage: (msg: ServerMsg) => void;
	onStatus: (status: ConnectionStatus) => void;
}

const KEEPALIVE_MS = 30_000;
/** The exact frame the room's auto-response matches; not part of `ClientMsg`. */
const PING = '{"type":"ping"}';
/** The room's exact answer to `PING`; not part of `ServerMsg`. */
const PONG = '{"type":"pong"}';

/** How much of an invalid frame a warning quotes. */
const LOGGED_FRAME_CHARS = 200;

/**
 * Decodes one frame from the room. Returns `null` for the keepalive answer and for a frame
 * the core rejects; the latter is logged and ignored, because one bad message must not end
 * the meeting.
 */
export function decodeFrame(raw: string): ServerMsg | null {
	if (raw === PONG) return null;
	try {
		return parseServerMsg(raw);
	} catch (error) {
		console.warn(
			'ignored an invalid message from the room:',
			error,
			raw.slice(0, LOGGED_FRAME_CHARS)
		);
		return null;
	}
}

const BASE_DELAY_MS = 500;
const MAX_DELAY_MS = 10_000;
/** A socket open this long without an error from the room has a join that holds. */
const STABLE_MS = 5_000;

/** Pure: delay before attempt `attempt` (1-based), with "equal jitter" from `random()`. */
export function backoffDelay(attempt: number, random: () => number = Math.random): number {
	const ceiling = Math.min(MAX_DELAY_MS, BASE_DELAY_MS * 2 ** Math.max(0, attempt - 1));
	return Math.round(ceiling / 2 + random() * (ceiling / 2));
}

function signalingUrl(meeting: MeetingId, location: Location = window.location): string {
	const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${scheme}//${location.host}/ws/${encodeURIComponent(meeting)}`;
}

export class SignalingClient {
	readonly #url: string;
	readonly #handlers: SignalingHandlers;
	#socket: WebSocket | null = null;
	/** The socket `reconnect()` replaced, open until the room answers on its successor. */
	#replaced: WebSocket | null = null;
	#attempt = 0;
	#retryTimer: ReturnType<typeof setTimeout> | undefined;
	#keepalive: ReturnType<typeof setInterval> | undefined;
	/** Starts the backoff over once the current socket has held for `STABLE_MS`. */
	#stableTimer: ReturnType<typeof setTimeout> | undefined;
	#stopped = false;

	constructor(meeting: MeetingId, handlers: SignalingHandlers) {
		this.#url = signalingUrl(meeting);
		this.#handlers = handlers;
	}

	connect(): void {
		document.addEventListener('visibilitychange', this.#retryNow);
		window.addEventListener('online', this.#retryNow);
		this.#open();
	}

	/**
	 * Sends if connected. Returns `false` otherwise, or if the core rejects `msg` (a bug in
	 * the caller, logged); callers decide whether that matters.
	 */
	send(msg: ClientMsg): boolean {
		if (this.#socket?.readyState !== WebSocket.OPEN) return false;
		let frame: string;
		try {
			frame = encodeClientMsg(msg);
		} catch (error) {
			console.error(`did not send an invalid ${msg.type} message:`, error);
			return false;
		}
		this.#socket.send(frame);
		return true;
	}

	/**
	 * Replaces the socket with a new one at once, as after a lost connection: `onOpen` runs
	 * again and the caller's `Join` carries the resume token. The new upgrade request carries
	 * the cookies the browser holds now, such as an owner session from a login since.
	 *
	 * The old socket stays open until the room has answered on the new one. The room counts a
	 * closed socket as its participant leaving, and it would often see that close before the
	 * new socket's `Join`, which then could not resume and would join as someone new. A
	 * resuming `Join` makes the room close the old socket itself; the client closes it too,
	 * once the room's first answer arrives. A second call while a replacement is under way
	 * does nothing: the new socket already carries the cookies.
	 */
	reconnect(): void {
		if (this.#stopped || this.#replaced !== null) return;
		clearTimeout(this.#retryTimer);
		clearTimeout(this.#stableTimer);
		clearInterval(this.#keepalive);
		// Detached first, so the old socket's messages and `close` event are ignored.
		this.#replaced = this.#socket;
		this.#socket = null;
		this.#attempt = 0;
		this.#open();
	}

	close(): void {
		if (this.#stopped) return;
		this.#stopped = true;
		clearTimeout(this.#retryTimer);
		clearTimeout(this.#stableTimer);
		clearInterval(this.#keepalive);
		document.removeEventListener('visibilitychange', this.#retryNow);
		window.removeEventListener('online', this.#retryNow);
		this.#socket?.close(1000, 'client closed');
		this.#socket = null;
		this.#closeReplaced();
		this.#handlers.onStatus({ kind: 'closed' });
	}

	#open(): void {
		if (this.#stopped) return;
		this.#attempt += 1;
		this.#handlers.onStatus({ kind: 'connecting', attempt: this.#attempt });
		const socket = new WebSocket(this.#url);
		this.#socket = socket;

		socket.addEventListener('open', () => {
			if (socket !== this.#socket) return;
			clearInterval(this.#keepalive);
			this.#keepalive = setInterval(() => {
				if (socket.readyState === WebSocket.OPEN) socket.send(PING);
			}, KEEPALIVE_MS);
			clearTimeout(this.#stableTimer);
			this.#stableTimer = setTimeout(() => (this.#attempt = 0), STABLE_MS);
			this.#handlers.onStatus({ kind: 'open' });
			this.#handlers.onOpen();
		});
		socket.addEventListener('message', (event) => {
			if (socket !== this.#socket || typeof event.data !== 'string') return;
			this.#closeReplaced();
			const msg = decodeFrame(event.data);
			if (msg === null) return;
			// The room placed us: the next lost connection starts the backoff over. Not on
			// `ConsentRequired`: a host gets it before the room asks the SFU for sessions, so it
			// does not mean the join holds. An error means it may not: wait for the next socket.
			if (msg.type === 'welcome' || msg.type === 'waiting') this.#attempt = 0;
			if (msg.type === 'error') clearTimeout(this.#stableTimer);
			this.#handlers.onMessage(msg);
		});
		socket.addEventListener('close', (event) => {
			if (socket !== this.#socket || this.#stopped) return;
			this.#socket = null;
			clearTimeout(this.#stableTimer);
			clearInterval(this.#keepalive);
			const after = afterClose(event.code);
			if (after === 'reconnect') {
				this.#scheduleRetry();
				return;
			}
			this.#stopped = true;
			this.#closeReplaced();
			this.#handlers.onStatus({ kind: 'dismissed', after, reason: event.reason });
		});
		// `error` is always followed by `close`, which handles the retry.
	}

	#closeReplaced(): void {
		this.#replaced?.close(1000, 'replaced');
		this.#replaced = null;
	}

	#scheduleRetry(): void {
		const retryInMs = backoffDelay(this.#attempt);
		this.#handlers.onStatus({ kind: 'waiting', attempt: this.#attempt, retryInMs });
		clearTimeout(this.#retryTimer);
		this.#retryTimer = setTimeout(() => this.#open(), retryInMs);
	}

	/** Skips the remaining backoff when the page is visible again or the network is back. */
	#retryNow = (): void => {
		if (this.#stopped || this.#socket !== null || document.visibilityState !== 'visible') return;
		clearTimeout(this.#retryTimer);
		this.#open();
	};
}
