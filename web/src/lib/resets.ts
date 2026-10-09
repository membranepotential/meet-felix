/**
 * Requests for a new SFU session after a peer connection failed (D33 in docs/media). A failed
 * connection reports its failure only once, so the request for a kind goes out again every
 * `RESET_RETRY_MS` until the room's `PublishReset` or `SubscribeReset` arrives, and never
 * more often. Each request names the epoch of the session whose connection failed, so the room
 * ignores it once that session is replaced.
 */
import type { SessionEpoch, SessionKind } from './protocol.ts';

/** How long to wait for the room's reset before asking again. */
export const RESET_RETRY_MS = 15_000;

export class SessionResets {
	readonly #request: (kind: SessionKind, epoch: SessionEpoch) => void;
	readonly #timers = new Map<SessionKind, ReturnType<typeof setTimeout>>();

	/** `request` sends `ClientMsg::ResetSession` for `kind` and `epoch`. */
	constructor(request: (kind: SessionKind, epoch: SessionEpoch) => void) {
		this.#request = request;
	}

	/**
	 * The peer connection for the session of `kind` and `epoch` failed: asks the room for a new
	 * session unless a request is waiting for its reset. Returns whether this started a new
	 * request.
	 */
	failed(kind: SessionKind, epoch: SessionEpoch): boolean {
		if (this.#timers.has(kind)) return false;
		this.#send(kind, epoch);
		return true;
	}

	/** The room's reset for `kind` arrived, or the connection was replaced anyway. */
	done(kind: SessionKind): void {
		clearTimeout(this.#timers.get(kind));
		this.#timers.delete(kind);
	}

	cancel(): void {
		for (const kind of [...this.#timers.keys()]) this.done(kind);
	}

	#send(kind: SessionKind, epoch: SessionEpoch): void {
		this.#request(kind, epoch);
		this.#timers.set(
			kind,
			setTimeout(() => this.#send(kind, epoch), RESET_RETRY_MS)
		);
	}
}
