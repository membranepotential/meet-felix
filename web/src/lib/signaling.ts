/**
 * WebSocket client for the Room Durable Object at `/ws/{meeting}`.
 *
 * Reconnects with exponential backoff and jitter after unexpected closes, and retries at
 * once when the page becomes visible or the network returns (iOS drops sockets of
 * backgrounded tabs). After every (re)connect `onOpen` runs, where the caller sends `Join`;
 * the server answers with `Welcome` and a full state, so no message replay is needed.
 */
import { parseServerMsg, type ClientMsg, type MeetingId, type ServerMsg } from './protocol.ts';

export type ConnectionStatus =
	| { kind: 'connecting'; attempt: number }
	| { kind: 'open' }
	/** Waiting `retryInMs` before the next attempt. */
	| { kind: 'waiting'; attempt: number; retryInMs: number }
	/** Closed for good: by us, or by the server with a terminal close code. */
	| { kind: 'closed'; code: number; reason: string };

export interface SignalingHandlers {
	onOpen: () => void;
	onMessage: (msg: ServerMsg) => void;
	onStatus: (status: ConnectionStatus) => void;
}

/**
 * Close codes 4000–4099 mean the server decided (meeting ended, kicked, refused);
 * reconnecting would not help. Other codes, including 1006 (connection lost), retry.
 */
const isTerminalClose = (code: number): boolean => code >= 4000 && code < 4100;

const BASE_DELAY_MS = 500;
const MAX_DELAY_MS = 10_000;

/** Pure: delay before attempt `attempt` (1-based), with "equal jitter" from `random()`. */
export function backoffDelay(attempt: number, random: () => number = Math.random): number {
	const ceiling = Math.min(MAX_DELAY_MS, BASE_DELAY_MS * 2 ** Math.max(0, attempt - 1));
	return Math.round(ceiling / 2 + random() * (ceiling / 2));
}

export function signalingUrl(meeting: MeetingId, location: Location = window.location): string {
	const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${scheme}//${location.host}/ws/${encodeURIComponent(meeting)}`;
}

export class SignalingClient {
	readonly #url: string;
	readonly #handlers: SignalingHandlers;
	#socket: WebSocket | null = null;
	#attempt = 0;
	#retryTimer: ReturnType<typeof setTimeout> | undefined;
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

	/** Sends if connected. Returns `false` otherwise; callers decide whether that matters. */
	send(msg: ClientMsg): boolean {
		if (this.#socket?.readyState !== WebSocket.OPEN) return false;
		this.#socket.send(JSON.stringify(msg));
		return true;
	}

	close(): void {
		if (this.#stopped) return;
		this.#stopped = true;
		clearTimeout(this.#retryTimer);
		document.removeEventListener('visibilitychange', this.#retryNow);
		window.removeEventListener('online', this.#retryNow);
		this.#socket?.close(1000, 'client closed');
		this.#socket = null;
		this.#handlers.onStatus({ kind: 'closed', code: 1000, reason: 'client closed' });
	}

	#open(): void {
		if (this.#stopped) return;
		this.#attempt += 1;
		this.#handlers.onStatus({ kind: 'connecting', attempt: this.#attempt });
		const socket = new WebSocket(this.#url);
		this.#socket = socket;

		socket.addEventListener('open', () => {
			if (socket !== this.#socket) return;
			this.#attempt = 0;
			this.#handlers.onStatus({ kind: 'open' });
			this.#handlers.onOpen();
		});
		socket.addEventListener('message', (event) => {
			if (socket !== this.#socket || typeof event.data !== 'string') return;
			const msg = parseServerMsg(event.data);
			if (msg) this.#handlers.onMessage(msg);
		});
		socket.addEventListener('close', (event) => {
			if (socket !== this.#socket || this.#stopped) return;
			this.#socket = null;
			if (isTerminalClose(event.code)) {
				this.#stopped = true;
				this.#handlers.onStatus({ kind: 'closed', code: event.code, reason: event.reason });
				return;
			}
			this.#scheduleRetry();
		});
		// `error` is always followed by `close`, which handles the retry.
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
