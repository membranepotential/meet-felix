/**
 * A signalling client for protocol tests: one WebSocket to a room, typed `ClientMsg` out and
 * `ServerMsg` in. Every server message waits in an inbox until a test takes it, so a test can
 * wait for a message that arrived before it asked.
 */
import { WebSocket as NodeWebSocket } from 'node:http';
import type { ClientMsg, MeetingId, ServerMsg } from '../../src/lib/protocol.ts';
import type { Auth } from './api.ts';

type ServerMsgType = ServerMsg['type'];
export type ServerMsgOf<T extends ServerMsgType> = Extract<ServerMsg, { type: T }>;

/** The keepalive frame the room answers without waking up; not part of `ClientMsg`. */
export const PING = '{"type":"ping"}';
/** The room's answer to `PING`. */
export const PONG = '{"type":"pong"}';

export const DEFAULT_TIMEOUT_MS = 5_000;

/**
 * Node's WebSocket (undici) takes request headers in a second argument, which the DOM typings
 * do not know; the owner's sockets need it for the session cookie. `node:http` exports it with
 * those typings.
 */
type Socket = InstanceType<typeof NodeWebSocket>;

interface Close {
	code: number;
	reason: string;
}

/** What arrived: a protocol message, or the keepalive answer. */
type Inbound = ServerMsg | { type: 'pong' };

export class RoomClient {
	readonly #socket: Socket;
	readonly #inbox: Inbound[] = [];
	#waiters: (() => boolean)[] = [];
	/** Resolves once the socket closed, by either side. */
	readonly closed: Promise<Close>;

	private constructor(socket: Socket) {
		this.#socket = socket;
		this.closed = new Promise((resolve) =>
			socket.addEventListener('close', (e) => resolve({ code: e.code, reason: e.reason }), {
				once: true
			})
		);
		socket.addEventListener('message', (event) => {
			this.#inbox.push(
				event.data === PONG ? { type: 'pong' } : (JSON.parse(event.data) as ServerMsg)
			);
			this.#waiters = this.#waiters.filter((waiter) => !waiter());
		});
	}

	/** Opens a socket to `meeting`; `headers` go on the upgrade, e.g. the owner's cookie. */
	static open(baseUrl: string, meeting: MeetingId, headers: Auth = {}): Promise<RoomClient> {
		const url = `${baseUrl.replace(/^http/, 'ws')}/ws/${meeting}`;
		return new Promise((resolve, reject) => {
			const socket = new NodeWebSocket(url, { headers });
			const timer = setTimeout(
				() => reject(new Error(`timeout opening ${url}`)),
				DEFAULT_TIMEOUT_MS
			);
			socket.addEventListener(
				'open',
				() => {
					clearTimeout(timer);
					resolve(new RoomClient(socket));
				},
				{ once: true }
			);
			socket.addEventListener('error', () => reject(new Error(`cannot open ${url}`)), {
				once: true
			});
		});
	}

	get isOpen(): boolean {
		return this.#socket.readyState === NodeWebSocket.OPEN;
	}

	send(msg: ClientMsg): void {
		this.#socket.send(JSON.stringify(msg));
	}

	/** Sends a frame as it is: the keepalive ping, or a malformed message. */
	sendRaw(frame: string): void {
		this.#socket.send(frame);
	}

	/** Sends `Join` with consent, unless `fields` say otherwise. */
	join(
		fields: Omit<Extract<ClientMsg, { type: 'join' }>, 'type' | 'consent'> & { consent?: boolean }
	): void {
		this.send({ type: 'join', consent: true, ...fields });
	}

	/** Resolves with (and removes) the first message of `type` that satisfies `matches`. */
	take<T extends ServerMsgType | 'pong'>(
		type: T,
		matches: (msg: Extract<Inbound, { type: T }>) => boolean = () => true,
		timeoutMs = DEFAULT_TIMEOUT_MS
	): Promise<Extract<Inbound, { type: T }>> {
		const isIt = (msg: Inbound): msg is Extract<Inbound, { type: T }> => msg.type === type;
		return new Promise((resolve, reject) => {
			const attempt = () => {
				const index = this.#inbox.findIndex((msg) => isIt(msg) && matches(msg));
				const msg = this.#inbox[index];
				if (index < 0 || msg === undefined || !isIt(msg)) return false;
				this.#inbox.splice(index, 1);
				clearTimeout(timer);
				resolve(msg);
				return true;
			};
			const timer = setTimeout(() => {
				this.#waiters = this.#waiters.filter((waiter) => waiter !== attempt);
				reject(new Error(`no ${type} message within ${timeoutMs} ms`));
			}, timeoutMs);
			if (!attempt()) this.#waiters.push(attempt);
		});
	}

	/** Whether a matching message of `type` arrives within `timeoutMs`; takes it if so. */
	gets<T extends ServerMsgType | 'pong'>(
		type: T,
		matches?: (msg: Extract<Inbound, { type: T }>) => boolean,
		timeoutMs = DEFAULT_TIMEOUT_MS
	): Promise<boolean> {
		return this.take(type, matches, timeoutMs).then(
			() => true,
			() => false
		);
	}

	/** Waits for the close, or fails after `timeoutMs`. */
	closes(timeoutMs = DEFAULT_TIMEOUT_MS): Promise<Close> {
		return Promise.race([
			this.closed,
			new Promise<never>((_, reject) =>
				setTimeout(() => reject(new Error(`socket still open after ${timeoutMs} ms`)), timeoutMs)
			)
		]);
	}

	close(code = 1000, reason = 'bye'): void {
		if (this.#socket.readyState <= NodeWebSocket.OPEN) this.#socket.close(code, reason);
	}
}
