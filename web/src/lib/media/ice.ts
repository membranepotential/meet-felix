/**
 * The ICE servers of one stay, kept fresh. The Worker hands out TURN credentials with their
 * lifetime (`ttl` from `crates/worker/src/ice.rs`), and every peer connection opened later,
 * after a session reset or a failed connection, needs credentials that still work. So the
 * servers are fetched again before they expire, sooner after a failure, and at once when the
 * signaling socket is back after a failure.
 */
import type { ApiResult, IceConfig } from '../api.ts';

/** Used until `/api/ice` answers: direct and STUN-only paths still work on most networks. */
export const FALLBACK_ICE: RTCIceServer[] = [{ urls: 'stun:stun.cloudflare.com:3478' }];

/** Fetch again this long before the credentials expire, or after half their lifetime if sooner. */
export const REFRESH_MARGIN_MS = 60 * 60 * 1000;
/**
 * After TURN credentials of unknown lifetime: a Worker from before `ttl` gave 6 hours. A web app
 * served apart from the Worker may meet one for a while after a deploy.
 */
export const UNKNOWN_TTL_REFRESH_MS = 5 * 60 * 60 * 1000;
/** After servers without TURN: the Worker gave up on Cloudflare for this request. */
export const STUN_ONLY_RETRY_MS = 5 * 60 * 1000;
/** After the rate limit refused us without a `Retry-After`: the Worker's window. */
export const RATE_LIMITED_RETRY_MS = 60 * 1000;
/** After any other failure. */
export const FAILED_RETRY_MS = 30 * 1000;

const isTurn = (server: RTCIceServer): boolean =>
	(Array.isArray(server.urls) ? server.urls : [server.urls]).some((url) => /^turns?:/.test(url));

/** How long to wait before fetching the servers again after `result`. */
export function refreshDelay(result: ApiResult<IceConfig>): number {
	if (!result.ok) {
		return result.error.kind === 'rate-limited'
			? (result.error.retryAfterMs ?? RATE_LIMITED_RETRY_MS)
			: FAILED_RETRY_MS;
	}
	const { servers, ttlMs } = result.value;
	if (!servers.some(isTurn)) return STUN_ONLY_RETRY_MS;
	if (ttlMs === null) return UNKNOWN_TTL_REFRESH_MS;
	return Math.max(ttlMs - REFRESH_MARGIN_MS, ttlMs / 2);
}

export class IceServers {
	readonly #fetch: () => Promise<ApiResult<IceConfig>>;
	readonly #onChange: (servers: RTCIceServer[]) => void;
	#latest: Promise<RTCIceServer[]>;
	/** The last fetch failed, so the next reconnect fetches at once. */
	#failed = false;
	#timer: ReturnType<typeof setTimeout> | undefined;
	#disposed = false;

	/**
	 * Fetches the servers at once with `fetch`; `onChange` gets every later set, for the peer
	 * connections that are already open.
	 */
	constructor(
		fetch: () => Promise<ApiResult<IceConfig>>,
		onChange: (servers: RTCIceServer[]) => void
	) {
		this.#fetch = fetch;
		this.#onChange = onChange;
		this.#latest = this.#load().then((servers) => servers ?? FALLBACK_ICE);
	}

	/** The servers for a new peer connection: the latest that arrived, STUN only if none did. */
	get latest(): Promise<RTCIceServer[]> {
		return this.#latest;
	}

	/** The connection is back: if the last fetch failed, fetch again now. */
	recover(): void {
		if (!this.#failed || this.#disposed) return;
		this.#failed = false;
		void this.#refresh();
	}

	dispose(): void {
		this.#disposed = true;
		clearTimeout(this.#timer);
	}

	async #refresh(): Promise<void> {
		const servers = await this.#load();
		if (servers === null) return;
		this.#latest = Promise.resolve(servers);
		this.#onChange(servers);
	}

	/** Fetches the servers and schedules the next fetch; `null` if it failed or we are done. */
	async #load(): Promise<RTCIceServer[] | null> {
		const result = await this.#fetch();
		if (this.#disposed) return null;
		this.#failed = !result.ok;
		clearTimeout(this.#timer);
		this.#timer = setTimeout(() => void this.#refresh(), refreshDelay(result));
		if (result.ok) return result.value.servers;
		console.warn('ICE servers unavailable, keeping the last ones', result.error);
		return null;
	}
}
