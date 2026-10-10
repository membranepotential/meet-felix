/** A typed client for the mock SFU's control API (see `../mock-sfu.ts`). */
import type {
	AdaptersResponse,
	AdapterView,
	CallsResponse,
	ControlCommand,
	ControlCommands,
	SfuCall
} from '../mock-sfu.ts';

export type { AdapterView, SfuCall };

export class MockSfu {
	readonly #url: string;
	/** Calls up to this sequence number belong to earlier tests. */
	#since = 0;

	constructor(url: string) {
		this.#url = url;
	}

	async #command<C extends ControlCommand>(command: C, body: ControlCommands[C]): Promise<void> {
		const response = await fetch(`${this.#url}/__control/${command}`, {
			method: 'POST',
			body: JSON.stringify(body)
		});
		if (response.status !== 204) {
			throw new Error(`mock SFU ${command}: ${response.status} ${await response.text()}`);
		}
	}

	/**
	 * Clears every fault and the adapter audio, and starts a new call log view: `calls()` shows
	 * only later calls.
	 */
	async reset(): Promise<void> {
		await this.#command('reset', {});
		const all = await this.#fetchCalls(0);
		this.#since = all.at(-1)?.seq ?? 0;
	}

	setDown(down: boolean): Promise<void> {
		return this.#command('down', { down });
	}

	setCloseDelay(ms: number): Promise<void> {
		return this.#command('close-delay', { ms });
	}

	/** The next pull's session is lost: that call and every later one on it get HTTP 410. */
	loseNextPull(): Promise<void> {
		return this.#command('lose-session', { next: 'pull' });
	}

	loseSession(session: string): Promise<void> {
		return this.#command('lose-session', { session });
	}

	/**
	 * Adapters created from now on stream `file` (raw 48 kHz stereo s16le PCM) in a loop;
	 * `null`: they connect and send nothing. `reset` sets `null`.
	 */
	setAdapterAudio(file: string | null): Promise<void> {
		return this.#command('adapter-audio', { file });
	}

	/** Every WebSocket adapter the mock started, oldest first, including other tests'. */
	async adapters(): Promise<AdapterView[]> {
		const response = await fetch(`${this.#url}/__control/adapters`);
		return ((await response.json()) as AdaptersResponse).adapters;
	}

	async #fetchCalls(since: number): Promise<SfuCall[]> {
		const response = await fetch(`${this.#url}/__control/calls?since=${since}`);
		const { calls } = (await response.json()) as CallsResponse;
		return calls;
	}

	/** The sequence number of the latest call; pass it as `after` to see only later calls. */
	async lastSeq(): Promise<number> {
		return (await this.#fetchCalls(this.#since)).at(-1)?.seq ?? this.#since;
	}

	/** The calls the room made after `after` (by default: since the last `reset`), oldest first. */
	calls(after = this.#since): Promise<SfuCall[]> {
		return this.#fetchCalls(after);
	}

	/** The first call after `after` that `matches`; throws after `timeoutMs`. */
	async waitForCall<C extends SfuCall>(
		matches: (call: SfuCall) => call is C,
		options?: WaitOptions
	): Promise<C>;
	async waitForCall(matches: (call: SfuCall) => boolean, options?: WaitOptions): Promise<SfuCall>;
	async waitForCall(
		matches: (call: SfuCall) => boolean,
		{ after = this.#since, timeoutMs = 5_000 }: WaitOptions = {}
	): Promise<SfuCall> {
		const deadline = Date.now() + timeoutMs;
		for (;;) {
			const call = (await this.calls(after)).find(matches);
			if (call) return call;
			if (Date.now() > deadline) throw new Error(`no matching SFU call within ${timeoutMs} ms`);
			await new Promise((resolve) => setTimeout(resolve, 50));
		}
	}
}

export interface WaitOptions {
	/** Only calls after this sequence number count; default: since the last `reset`. */
	after?: number;
	timeoutMs?: number;
}

/** Narrowing predicates for `waitForCall` and filters. */
export const isOp =
	<Op extends SfuCall['op']>(op: Op) =>
	(call: SfuCall): call is Extract<SfuCall, { op: Op }> =>
		call.op === op;

/** A `tracks/new` that publishes the client's tracks (local) or pulls others' (remote). */
export const isPublish = (call: SfuCall): call is Extract<SfuCall, { op: 'tracks/new' }> =>
	call.op === 'tracks/new' && call.body.tracks[0]?.location === 'local';
export const isPull = (call: SfuCall): call is Extract<SfuCall, { op: 'tracks/new' }> =>
	call.op === 'tracks/new' && call.body.tracks[0]?.location === 'remote';
